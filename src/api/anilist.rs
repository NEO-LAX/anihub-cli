use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};

const ANILIST_URL: &str = "https://graphql.anilist.co";

/// Член франшизи, знайдений через AniList.
#[derive(Debug, Clone)]
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
}

#[derive(Deserialize)]
struct RespData {
    #[serde(rename = "Media")]
    media: Option<Media>,
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

async fn gql(client: &Client, query: &'static str, vars: serde_json::Value) -> Result<Media> {
    let res: Resp = client
        .post(ANILIST_URL)
        .json(&Req { query, variables: vars })
        .send().await?
        .error_for_status()?
        .json().await?;
    res.data
        .and_then(|d| d.media)
        .ok_or_else(|| anyhow::anyhow!("AniList: no result"))
}

/// Знаходить усі TV/MOVIE аніме у франшизі через BFS по AniList відносинам.
/// Глибина BFS ≤ 2; затримка 150 мс між запитами.
/// При будь-якій помилці повертає порожній Vec (graceful fallback).
pub async fn get_franchise_members(client: &Client, title_original: &str) -> Vec<FranchiseMember> {
    match get_inner(client, title_original).await {
        Ok(m) => m,
        Err(_) => Vec::new(),
    }
}

/// Публічна функція для BFS починаючи з відомого AniList ID (надійніше ніж пошук за назвою).
pub async fn get_franchise_members_by_id(client: &Client, anilist_id: u32) -> Vec<FranchiseMember> {
    match get_inner_by_id(client, anilist_id).await {
        Ok(m) => m,
        Err(_) => Vec::new(),
    }
}

async fn get_inner(client: &Client, title_original: &str) -> Result<Vec<FranchiseMember>> {
    let root = gql(client, SEARCH_Q, serde_json::json!({ "s": title_original })).await?;
    bfs_from_root(client, root).await
}

async fn get_inner_by_id(client: &Client, anilist_id: u32) -> Result<Vec<FranchiseMember>> {
    let root = gql(client, ID_Q, serde_json::json!({ "i": anilist_id })).await?;
    bfs_from_root(client, root).await
}

/// BFS по графу AniList. Глибина ≤ 3 — достатньо щоб знайти фільми/сезони
/// які є SEQUEL сезону 3 (S1→S2→S3→Фільм = глибина 3 від S1).
async fn bfs_from_root(client: &Client, root: Media) -> Result<Vec<FranchiseMember>> {
    let mut members: Vec<FranchiseMember> = Vec::new();
    let mut visited: HashSet<u32> = HashSet::new();
    let mut queue: VecDeque<(Media, u8)> = VecDeque::new();

    visited.insert(root.id);
    queue.push_back((root, 0));

    while let Some((media, depth)) = queue.pop_front() {
        let fmt = media.format.as_deref().unwrap_or("");
        if matches!(fmt, "TV" | "MOVIE" | "ONA") {
            members.push(FranchiseMember {
                anilist_id: media.id,
                is_tv: fmt == "TV",
            });
        }

        if depth >= 3 {
            continue;
        }

        let edges = match &media.relations {
            Some(r) => r.edges.clone(),
            None => continue,
        };

        for edge in edges {
            if edge.node.media_type != "ANIME" || visited.contains(&edge.node.id) {
                continue;
            }
            let rel = edge.relation_type.as_str();
            if !matches!(rel, "SEQUEL" | "PREQUEL" | "PARENT" | "SUMMARY") {
                continue;
            }
            let nf = edge.node.format.as_deref().unwrap_or("");
            if !matches!(nf, "TV" | "MOVIE" | "ONA") {
                continue;
            }
            visited.insert(edge.node.id);
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            if let Ok(full) = gql(client, ID_Q, serde_json::json!({ "i": edge.node.id })).await {
                queue.push_back((full, depth + 1));
            }
        }
    }

    Ok(members)
}

