#![allow(dead_code)]

use super::client::{ApiError, parse_retry_after};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const ANILIST_URL: &str = "https://graphql.anilist.co";
const MAX_RELATION_DEPTH: u8 = 3;
const ANILIST_MIN_INTERVAL: Duration = Duration::from_millis(200);

/// Член франшизи, знайдений через AniList.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FranchiseMember {
    pub anilist_id: u32,
    pub is_tv: bool,
}

#[derive(Serialize)]
struct Req {
    query: &'static str,
    variables: serde_json::Value,
}

#[derive(Deserialize)]
struct Resp {
    data: Option<RespData>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Deserialize)]
struct RespData {
    #[serde(rename = "Media")]
    media: Option<Media>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
    #[serde(default)]
    path: Vec<serde_json::Value>,
    #[serde(default)]
    extensions: serde_json::Value,
}

#[derive(Deserialize, Clone)]
struct Media {
    id: u32,
    format: Option<String>,
    relations: Option<Relations>,
}

#[derive(Deserialize, Clone)]
struct Relations {
    edges: Vec<Edge>,
}

#[derive(Deserialize, Clone)]
struct Edge {
    #[serde(rename = "relationType")]
    relation_type: String,
    node: Node,
}

#[derive(Deserialize, Clone)]
struct Node {
    id: u32,
    #[serde(rename = "type")]
    media_type: String,
    format: Option<String>,
}

const SEARCH_Q: &str = "query($s:String){Media(search:$s,type:ANIME){id format relations{edges{relationType(version:2)node{id type format}}}}}";
const ID_Q: &str = "query($i:Int){Media(id:$i,type:ANIME){id format relations{edges{relationType(version:2)node{id type format}}}}}";

/// Injectable AniList GraphQL client.  Its request gate guarantees one
/// request at a time and at least 200 ms between starts by default; the
/// resource worker adds the same policy at its scheduling boundary.
#[derive(Clone)]
pub struct AniListClient {
    client: Client,
    base_url: String,
    min_request_interval: Duration,
    last_request_start: Arc<Mutex<Option<Instant>>>,
}

impl AniListClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent("anihub-cli/0.3")
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .context("Failed to build AniList HTTP client")?;
        Self::from_client(client, ANILIST_URL)
    }

    pub fn with_base_url(base_url: impl AsRef<str>) -> Result<Self> {
        let client = Client::builder()
            .user_agent("anihub-cli/0.3")
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .context("Failed to build AniList HTTP client")?;
        Self::from_client(client, base_url)
    }

    pub fn from_client(client: Client, base_url: impl AsRef<str>) -> Result<Self> {
        let base_url = base_url.as_ref().trim_end_matches('/');
        if base_url.is_empty() {
            anyhow::bail!("AniList base URL must not be empty");
        }
        Ok(Self {
            client,
            base_url: base_url.to_string(),
            min_request_interval: ANILIST_MIN_INTERVAL,
            last_request_start: Arc::new(Mutex::new(None)),
        })
    }

    pub fn with_min_request_interval(mut self, interval: Duration) -> Self {
        self.min_request_interval = interval;
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn http_client(&self) -> &Client {
        &self.client
    }

    pub async fn get_franchise_members(
        &self,
        title_original: &str,
    ) -> Result<Vec<FranchiseMember>> {
        let root = self
            .gql(SEARCH_Q, serde_json::json!({ "s": title_original }))
            .await
            .context("AniList title search failed")?;
        bfs_from_root(self, root).await
    }

    pub async fn get_franchise_members_by_id(
        &self,
        anilist_id: u32,
    ) -> Result<Vec<FranchiseMember>> {
        let root = self
            .gql(ID_Q, serde_json::json!({ "i": anilist_id }))
            .await
            .with_context(|| format!("AniList id lookup failed for {anilist_id}"))?;
        bfs_from_root(self, root).await
    }

    async fn gql(&self, query: &'static str, vars: serde_json::Value) -> Result<Media> {
        self.wait_for_request_slot().await;
        let response = self
            .client
            .post(&self.base_url)
            .json(&Req {
                query,
                variables: vars,
            })
            .send()
            .await
            .map_err(|source| {
                anyhow::Error::new(ApiError::Transport {
                    operation: "AniList GraphQL request".to_string(),
                    source,
                })
            })
            .context("AniList GraphQL request failed")?;

        if !response.status().is_success() {
            return Err(anyhow::Error::new(ApiError::Http {
                operation: "AniList GraphQL request".to_string(),
                status: response.status().as_u16(),
                retry_after: parse_retry_after(response.headers()),
            }));
        }

        let response: Resp = response.json().await.map_err(|source| {
            anyhow::Error::new(ApiError::Parse {
                operation: "AniList GraphQL response".to_string(),
                message: source.to_string(),
            })
        })?;

        if !response.errors.is_empty() {
            let messages = response
                .errors
                .iter()
                .map(format_graphql_error)
                .collect::<Vec<_>>()
                .join("; ");
            anyhow::bail!("AniList GraphQL errors: {messages}");
        }

        response
            .data
            .and_then(|data| data.media)
            .ok_or_else(|| anyhow::anyhow!("AniList: response contained no Media result"))
    }

    async fn wait_for_request_slot(&self) {
        let mut last_request_start = self.last_request_start.lock().await;
        if let Some(last_start) = *last_request_start {
            let elapsed = last_start.elapsed();
            if elapsed < self.min_request_interval {
                tokio::time::sleep(self.min_request_interval - elapsed).await;
            }
        }
        *last_request_start = Some(Instant::now());
    }
}

/// Fallible API used by the redesigned resource layer.
pub async fn get_franchise_members_result(
    client: &Client,
    title_original: &str,
) -> Result<Vec<FranchiseMember>> {
    AniListClient::from_client(client.clone(), ANILIST_URL)?
        .get_franchise_members(title_original)
        .await
}

/// Compatibility wrapper.  Existing UI code treats AniList as an optional
/// enrichment, so it retains the old empty-vector fallback.  New code should
/// use `AniListClient` or the fallible `_result` functions to surface errors.
pub async fn get_franchise_members(client: &Client, title_original: &str) -> Vec<FranchiseMember> {
    get_franchise_members_result(client, title_original)
        .await
        .unwrap_or_default()
}

pub async fn get_franchise_members_by_id_result(
    client: &Client,
    anilist_id: u32,
) -> Result<Vec<FranchiseMember>> {
    AniListClient::from_client(client.clone(), ANILIST_URL)?
        .get_franchise_members_by_id(anilist_id)
        .await
}

/// Compatibility wrapper for the old optional-enrichment call sites.
pub async fn get_franchise_members_by_id(client: &Client, anilist_id: u32) -> Vec<FranchiseMember> {
    get_franchise_members_by_id_result(client, anilist_id)
        .await
        .unwrap_or_default()
}

async fn bfs_from_root(client: &AniListClient, root: Media) -> Result<Vec<FranchiseMember>> {
    let mut members = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::<(Media, u8)>::new();

    visited.insert(root.id);
    queue.push_back((root, 0));

    while let Some((media, depth)) = queue.pop_front() {
        let format = media.format.as_deref().unwrap_or("");
        if matches!(format, "TV" | "MOVIE" | "ONA") {
            members.push(FranchiseMember {
                anilist_id: media.id,
                is_tv: format == "TV",
            });
        }

        if depth >= MAX_RELATION_DEPTH {
            continue;
        }

        let mut edges = media
            .relations
            .map(|relations| relations.edges)
            .unwrap_or_default();
        edges.sort_by(|left, right| {
            left.node
                .id
                .cmp(&right.node.id)
                .then_with(|| left.relation_type.cmp(&right.relation_type))
        });

        for edge in edges {
            if edge.node.media_type != "ANIME" || visited.contains(&edge.node.id) {
                continue;
            }
            if !matches!(
                edge.relation_type.as_str(),
                "SEQUEL" | "PREQUEL" | "PARENT" | "SUMMARY"
            ) {
                continue;
            }
            let node_format = edge.node.format.as_deref().unwrap_or("");
            if !matches!(node_format, "TV" | "MOVIE" | "ONA") {
                continue;
            }

            visited.insert(edge.node.id);
            let full = client
                .gql(ID_Q, serde_json::json!({ "i": edge.node.id }))
                .await
                .with_context(|| format!("AniList relation lookup failed for {}", edge.node.id))?;
            queue.push_back((full, depth + 1));
        }
    }

    members.sort_by_key(|member| member.anilist_id);
    members.dedup_by_key(|member| member.anilist_id);
    Ok(members)
}

fn format_graphql_error(error: &GraphQlError) -> String {
    let path = if error.path.is_empty() {
        String::new()
    } else {
        format!(
            " at {}",
            serde_json::to_string(&error.path).unwrap_or_default()
        )
    };
    let extensions = if error.extensions.is_null() {
        String::new()
    } else {
        format!(" ({})", error.extensions)
    };
    format!("{}{}{}", error.message, path, extensions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn server(body: &'static str) -> (String, tokio::sync::oneshot::Sender<()>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            tokio::select! {
                accepted = listener.accept() => {
                    if let Ok((mut stream, _)) = accepted {
                        let mut request = [0u8; 4096];
                        let _ = stream.read(&mut request).await;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(), body
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                    }
                }
                _ = &mut rx => {}
            }
        });
        (address, tx)
    }

    #[tokio::test]
    async fn graphql_top_level_errors_are_not_treated_as_empty_data() {
        let (url, shutdown) = server(
            r#"{"errors":[{"message":"rate limited","extensions":{"code":"RATE_LIMITED"}}]}"#,
        )
        .await;
        let client = AniListClient::with_base_url(url)
            .unwrap()
            .with_min_request_interval(Duration::ZERO);
        let error = client.get_franchise_members_by_id(1).await.unwrap_err();
        assert!(format!("{error:#}").contains("AniList GraphQL errors"));
        let _ = shutdown.send(());
    }
}
