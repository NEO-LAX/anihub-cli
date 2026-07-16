use super::franchise::AniListMedia;
use super::models::{AnimeDetails, AnimeItem, AnimeSearchResponse, EpisodeSourcesResponse};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::{Client, RequestBuilder, Response, header};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;

const API_BASE_URL: &str = "https://api.anihub.in.ua";
const INTERNAL_API_BASE_URL: &str = "https://anihub.in.ua/api";
const ANILIST_GRAPHQL_URL: &str = "https://graphql.anilist.co";
const ANILIST_BATCH_SIZE: usize = 50;
const SEARCH_PAGE_SIZE: u32 = 20;
const BASIC_SEARCH_RESULT_LIMIT: usize = 20;
const EXTENDED_SEARCH_PAGE_LIMIT: u32 = 5;
const EXTENDED_SEARCH_RESULT_LIMIT: usize = 100;
const MAX_CONCURRENT_REQUESTS: usize = 3;
const MAX_REQUEST_STARTS: usize = 40;
const REQUEST_WINDOW: Duration = Duration::from_secs(60);

const ANILIST_MEDIA_BATCH_QUERY: &str = r#"
query AniHubFranchiseBatch($ids: [Int!]!) {
  Page(page: 1, perPage: 50) {
    media(id_in: $ids, type: ANIME, sort: ID) {
      id
      type
      format
      status
      episodes
      seasonYear
      nextAiringEpisode { episode airingAt }
      title { romaji english native }
      coverImage { large }
      relations {
        edges {
          relationType
          node {
            id
            type
            format
            status
            episodes
            seasonYear
            nextAiringEpisode { episode airingAt }
            title { romaji english native }
            coverImage { large }
          }
        }
      }
    }
  }
}
"#;

#[derive(Debug, serde::Deserialize)]
struct AniListGraphQlResponse {
    #[serde(default)]
    data: Option<AniListGraphQlData>,
    #[serde(default)]
    errors: Vec<AniListGraphQlError>,
}

#[derive(Debug, serde::Deserialize)]
struct AniListGraphQlData {
    #[serde(default, rename = "Page")]
    page: Option<AniListGraphQlPage>,
}

#[derive(Debug, serde::Deserialize)]
struct AniListGraphQlPage {
    #[serde(default)]
    media: Vec<AniListMedia>,
}

#[derive(Debug, serde::Deserialize)]
struct AniListGraphQlError {
    message: String,
}

struct RequestGate {
    concurrency: Arc<tokio::sync::Semaphore>,
    starts: tokio::sync::Mutex<VecDeque<Instant>>,
}

impl RequestGate {
    fn new() -> Self {
        Self {
            concurrency: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_REQUESTS)),
            starts: tokio::sync::Mutex::new(VecDeque::new()),
        }
    }

    async fn acquire(&self) -> tokio::sync::OwnedSemaphorePermit {
        let permit = self
            .concurrency
            .clone()
            .acquire_owned()
            .await
            .expect("AniHub request semaphore is never closed");
        loop {
            let delay = {
                let mut starts = self.starts.lock().await;
                let now = Instant::now();
                while starts
                    .front()
                    .is_some_and(|started| now.duration_since(*started) >= REQUEST_WINDOW)
                {
                    starts.pop_front();
                }
                if starts.len() < MAX_REQUEST_STARTS {
                    starts.push_back(now);
                    None
                } else {
                    starts
                        .front()
                        .map(|started| REQUEST_WINDOW.saturating_sub(now.duration_since(*started)))
                }
            };
            match delay {
                None => return permit,
                Some(delay) => tokio::time::sleep(delay).await,
            }
        }
    }
}

/// Errors produced while talking to an AniHub HTTP endpoint.
///
/// The worker uses the status and retry-after information to decide whether a
/// request is safe to retry.  The public API still returns `anyhow::Result`
/// for compatibility with the existing application code; callers can
/// downcast an error to this type when they need structured information.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("{operation}: HTTP status {status}")]
    Http {
        operation: String,
        status: u16,
        retry_after: Option<Duration>,
    },
    #[error("{operation}: request failed: {source}")]
    Transport {
        operation: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{operation}: response JSON could not be parsed: {message}")]
    Parse { operation: String, message: String },
    #[error("{operation}: response body could not be decoded: {message}")]
    Decode { operation: String, message: String },
}

impl ApiError {
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            Self::Transport { .. } | Self::Parse { .. } | Self::Decode { .. } => None,
        }
    }

    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Http { retry_after, .. } => *retry_after,
            Self::Transport { .. } | Self::Parse { .. } | Self::Decode { .. } => None,
        }
    }

    pub fn is_not_found(&self) -> bool {
        self.status() == Some(404)
    }
}

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
    api_base_url: String,
    internal_api_base_url: String,
    anilist_graphql_url: String,
    request_gate: Arc<RequestGate>,
}

impl ApiClient {
    /// Build a production client using the real AniHub endpoints.
    pub fn new() -> Result<Self> {
        Self::with_base_urls(API_BASE_URL, INTERNAL_API_BASE_URL)
    }

    /// Build a client with injectable public and internal API bases.
    ///
    /// This is intentionally a constructor rather than a mutable setter so a
    /// cloned client cannot accidentally switch endpoints underneath an
    /// in-flight request.
    pub fn with_base_urls(
        api_base_url: impl AsRef<str>,
        internal_api_base_url: impl AsRef<str>,
    ) -> Result<Self> {
        let client = build_http_client().context("Failed to build HTTP client")?;
        Self::from_client(client, api_base_url, internal_api_base_url)
    }

    /// Alias useful in tests and by downstream callers that prefer a `new_*`
    /// constructor name.
    pub fn new_with_base_urls(
        api_base_url: impl AsRef<str>,
        internal_api_base_url: impl AsRef<str>,
    ) -> Result<Self> {
        Self::with_base_urls(api_base_url, internal_api_base_url)
    }

    /// Build a client around an already configured reqwest client.
    pub fn from_client(
        client: Client,
        api_base_url: impl AsRef<str>,
        internal_api_base_url: impl AsRef<str>,
    ) -> Result<Self> {
        Self::from_client_with_endpoints(
            client,
            api_base_url,
            internal_api_base_url,
            ANILIST_GRAPHQL_URL,
        )
    }

    /// Build a client with an injectable AniList GraphQL endpoint.
    pub fn with_endpoints(
        api_base_url: impl AsRef<str>,
        internal_api_base_url: impl AsRef<str>,
        anilist_graphql_url: impl AsRef<str>,
    ) -> Result<Self> {
        let client = build_http_client().context("Failed to build HTTP client")?;
        Self::from_client_with_endpoints(
            client,
            api_base_url,
            internal_api_base_url,
            anilist_graphql_url,
        )
    }

    /// Injectable-endpoint variant for callers that already own a reqwest client.
    pub fn from_client_with_endpoints(
        client: Client,
        api_base_url: impl AsRef<str>,
        internal_api_base_url: impl AsRef<str>,
        anilist_graphql_url: impl AsRef<str>,
    ) -> Result<Self> {
        Ok(Self {
            client,
            api_base_url: normalize_base_url(api_base_url.as_ref())?,
            internal_api_base_url: normalize_base_url(internal_api_base_url.as_ref())?,
            anilist_graphql_url: normalize_base_url(anilist_graphql_url.as_ref())?,
            request_gate: Arc::new(RequestGate::new()),
        })
    }

    pub fn http_client(&self) -> &Client {
        &self.client
    }

    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    pub fn internal_api_base_url(&self) -> &str {
        &self.internal_api_base_url
    }

    pub fn anilist_graphql_url(&self) -> &str {
        &self.anilist_graphql_url
    }

    fn generate_api_key(&self) -> String {
        let date_str = Utc::now().format("%Y-%m-%d").to_string();
        let key_str = format!("Ukr@in1anAn1me-S3curity-Key-2025_{date_str}");

        let mut hasher = Sha256::new();
        hasher.update(key_str.as_bytes());
        hex::encode(hasher.finalize())
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}/{}", self.api_base_url, path.trim_start_matches('/'))
    }

    fn internal_api_url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.internal_api_base_url,
            path.trim_start_matches('/')
        )
    }

    fn authenticated(&self, request: RequestBuilder) -> RequestBuilder {
        request.header("X-API-Key", self.generate_api_key())
    }

    async fn send_request(&self, request: RequestBuilder, operation: &str) -> Result<Response> {
        let _permit = self.request_gate.acquire().await;
        request
            .send()
            .await
            .map_err(|source| request_error(operation, source))
    }

    /// Conservative title search used by the main UI.
    ///
    /// Only the first AniHub page is requested. Broad pagination belongs to a
    /// future explicit advanced-search mode; doing it for every short query
    /// can exhaust the upstream request budget. Results must match the first
    /// first two words of one of the available titles.
    pub async fn search_anime(&self, query: &str) -> Result<Vec<AnimeItem>> {
        self.search_anime_with_mode(query, false).await
    }

    pub async fn search_anime_with_mode(
        &self,
        query: &str,
        extended: bool,
    ) -> Result<Vec<AnimeItem>> {
        let normalized_query = normalize_search_text(query);
        if normalized_query.is_empty() {
            return Ok(Vec::new());
        }

        let first_page = self.search_anime_page(query, 1).await?;
        if extended {
            let page_limit = first_page.total_pages.min(EXTENDED_SEARCH_PAGE_LIMIT);
            let mut items = first_page.items;
            for page in 2..=page_limit {
                items.extend(self.search_anime_page(query, page).await?.items);
            }
            return Ok(deduplicate_anime_by_id(
                items
                    .into_iter()
                    .filter(|item| item.has_ukrainian_dub)
                    .collect(),
            )
            .into_iter()
            .take(EXTENDED_SEARCH_RESULT_LIMIT)
            .collect());
        }

        let matches = first_page
            .items
            .into_iter()
            .filter(|item| item.has_ukrainian_dub)
            .filter(|item| basic_title_match(item, &normalized_query));

        Ok(deduplicate_anime_by_id(matches.collect())
            .into_iter()
            .take(BASIC_SEARCH_RESULT_LIMIT)
            .collect())
    }

    async fn search_anime_page(&self, query: &str, page: u32) -> Result<AnimeSearchResponse> {
        let page = page.to_string();
        let request = self
            .authenticated(self.client.get(self.api_url("/anime/")))
            .query(&[
                ("search", query),
                ("has_ukrainian_dub", "true"),
                ("page_size", "20"),
                ("page", page.as_str()),
            ]);
        let response = self
            .send_request(request, "AniHub search request")
            .await
            .context("Failed to send AniHub search request")?;
        let response = ensure_success(response, "AniHub search")?;
        parse_json(response, "AniHub search")
            .await
            .context("Failed to parse AniHub search response")
    }

    pub async fn get_anime_details(&self, anime_id: u32) -> Result<AnimeDetails> {
        let request = self.authenticated(
            self.client
                .get(self.api_url(&format!("/anime/{anime_id}/"))),
        );
        let response = self
            .send_request(request, "AniHub anime details request")
            .await
            .context("Failed to send anime details request")?;
        let response = ensure_success(response, "AniHub anime details")?;
        parse_json(response, "AniHub anime details")
            .await
            .context("Failed to parse anime details response")
    }

    pub async fn get_episode_sources(
        &self,
        anime_id: u32,
        season: u32,
    ) -> Result<EpisodeSourcesResponse> {
        let request = self.authenticated(
            self.client
                .get(self.internal_api_url(&format!("/anime/{anime_id}/episode-sources")))
                .query(&[("season", season)]),
        );
        let response = self
            .send_request(request, "AniHub episode sources request")
            .await
            .with_context(|| {
                format!(
                    "Failed to send episode sources request for anime {anime_id}, season {season}"
                )
            })?;
        let response = ensure_success(response, "AniHub episode sources")?;
        parse_json(response, "AniHub episode sources")
            .await
            .with_context(|| {
                format!("Failed to parse episode sources for anime {anime_id}, season {season}")
            })
    }

    /// Look up an AniHub id by AniList id.  Only a successful empty response
    /// or HTTP 404 is represented as `None`; rate limits, server failures,
    /// malformed JSON, and all other HTTP errors are returned to the caller.
    pub async fn get_anime_by_anilist_id(&self, anilist_id: u32) -> Result<Option<u32>> {
        let request = self
            .authenticated(self.client.get(self.api_url("/anime/")))
            .query(&[
                ("anilist_id", anilist_id.to_string()),
                ("page_size", SEARCH_PAGE_SIZE.to_string()),
                ("has_ukrainian_dub", "true".to_string()),
                ("page", "1".to_string()),
            ]);
        let response = self
            .send_request(request, "AniHub AniList-id lookup")
            .await
            .context("Failed to send AniHub AniList-id lookup")?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = ensure_success(response, "AniHub AniList-id lookup")?;
        let search_response: AnimeSearchResponse = parse_json(response, "AniHub AniList-id lookup")
            .await
            .context("Failed to parse AniHub AniList-id lookup response")?;
        Ok(search_response
            .items
            .into_iter()
            .next()
            .map(|anime| anime.id))
    }

    /// Fetch AniList media and their direct relation nodes in deterministic
    /// batches. Duplicate and zero ids are ignored, and the result is sorted
    /// by AniList id regardless of response order.
    pub async fn get_anilist_media_batch(&self, anilist_ids: &[u32]) -> Result<Vec<AniListMedia>> {
        let ids = anilist_ids
            .iter()
            .copied()
            .filter(|id| *id != 0)
            .collect::<std::collections::BTreeSet<_>>();
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let ids = ids.into_iter().collect::<Vec<_>>();
        let mut media_by_id = std::collections::BTreeMap::new();
        for batch in ids.chunks(ANILIST_BATCH_SIZE) {
            let request = self
                .client
                .post(&self.anilist_graphql_url)
                .json(&serde_json::json!({
                    "query": ANILIST_MEDIA_BATCH_QUERY,
                    "variables": { "ids": batch },
                }));
            let response = self
                .send_request(request, "AniList media batch request")
                .await
                .context("Failed to send AniList media batch request")?;
            let response = ensure_success(response, "AniList media batch")?;
            let payload: AniListGraphQlResponse =
                parse_json(response, "AniList media batch").await?;
            if !payload.errors.is_empty() {
                let message = payload
                    .errors
                    .into_iter()
                    .map(|error| error.message)
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(anyhow::Error::new(ApiError::Parse {
                    operation: "AniList media batch".to_string(),
                    message,
                }));
            }
            let page = payload.data.and_then(|data| data.page).ok_or_else(|| {
                anyhow::Error::new(ApiError::Parse {
                    operation: "AniList media batch".to_string(),
                    message: "response did not contain data.Page".to_string(),
                })
            })?;
            for media in page.media {
                media_by_id.insert(media.id, media);
            }
        }
        Ok(media_by_id.into_values().collect())
    }

    pub async fn fetch_poster(&self, url: &str) -> Result<(image::DynamicImage, Vec<u8>)> {
        let response = self
            .send_request(self.client.get(url), "AniHub poster request")
            .await
            .context("Failed to send poster request")?;
        let response = ensure_success(response, "AniHub poster")?;
        let bytes = response
            .bytes()
            .await
            .map_err(|source| request_error("AniHub poster body", source))
            .context("Failed to read poster response")?;
        let image = image::load_from_memory(&bytes)
            .map_err(|source| {
                anyhow::Error::new(ApiError::Decode {
                    operation: "AniHub poster".to_string(),
                    message: source.to_string(),
                })
            })
            .context("Failed to decode poster image")?;
        Ok((image, bytes.to_vec()))
    }

    /// Load one AniHub release using the franchise-level season expected by
    /// the episode-sources endpoint. Separate AniHub ids do not make this
    /// parameter release-local: S2 entries still require `season=2`.
    pub async fn get_release_sources(
        &self,
        anime_id: u32,
        season: u32,
    ) -> Result<EpisodeSourcesResponse> {
        let mut sources = self.get_episode_sources(anime_id, season).await?;
        if sources.ashdi.is_empty() && sources.moonanime.is_empty() {
            anyhow::bail!("No episode sources found for anime {anime_id} season {season}");
        }

        sources.ashdi.sort_by(|left, right| {
            left.season_number
                .cmp(&right.season_number)
                .then_with(|| left.studio_name.cmp(&right.studio_name))
                .then_with(|| left.id.cmp(&right.id))
                .then_with(|| right.episodes.len().cmp(&left.episodes.len()))
        });

        Ok(sources)
    }
}

fn build_http_client() -> Result<Client> {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        ),
    );
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static("application/json"),
    );

    Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .pool_idle_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(6)
        .build()
        .context("Failed to build HTTP client")
}

fn normalize_base_url(url: &str) -> Result<String> {
    let normalized = url.trim_end_matches('/');
    if normalized.is_empty() {
        anyhow::bail!("API base URL must not be empty");
    }
    Ok(normalized.to_string())
}

fn request_error(operation: &str, source: reqwest::Error) -> anyhow::Error {
    anyhow::Error::new(ApiError::Transport {
        operation: operation.to_string(),
        source,
    })
}

fn ensure_success(response: Response, operation: &str) -> Result<Response> {
    if response.status().is_success() {
        return Ok(response);
    }

    let retry_after = parse_retry_after(response.headers());
    Err(anyhow::Error::new(ApiError::Http {
        operation: operation.to_string(),
        status: response.status().as_u16(),
        retry_after,
    }))
}

async fn parse_json<T: serde::de::DeserializeOwned>(
    response: Response,
    operation: &str,
) -> Result<T> {
    response.json::<T>().await.map_err(|source| {
        anyhow::Error::new(ApiError::Parse {
            operation: operation.to_string(),
            message: source.to_string(),
        })
    })
}

pub(crate) fn parse_retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    let value = headers.get(header::RETRY_AFTER)?.to_str().ok()?;
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let date = DateTime::parse_from_rfc2822(value).ok()?;
    let until = date.with_timezone(&Utc);
    (until - Utc::now()).to_std().ok()
}

fn normalize_search_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if pending_space && !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.push(character);
            pending_space = false;
        } else {
            pending_space = true;
        }
    }
    normalized
}

fn basic_title_match(item: &AnimeItem, normalized_query: &str) -> bool {
    [
        Some(item.title_ukrainian.as_str()),
        item.title_original.as_deref(),
        item.title_english.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|title| {
        let title = normalize_search_text(title);
        let title_words = title.split_whitespace().collect::<Vec<_>>();
        let query_is_one_word = !normalized_query.contains(' ');

        title_words.iter().take(2).enumerate().any(|(start, word)| {
            if query_is_one_word {
                word.contains(normalized_query)
            } else {
                title_words[start..].join(" ").starts_with(normalized_query)
            }
        })
    })
}

fn deduplicate_anime_by_id(items: Vec<AnimeItem>) -> Vec<AnimeItem> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Mutex;

    #[derive(Clone)]
    struct MockServer {
        address: String,
        requests: Arc<Mutex<Vec<String>>>,
        shutdown: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    }

    impl MockServer {
        async fn start<F>(handler: F) -> Self
        where
            F: Fn(String, String) -> (u16, String) + Send + Sync + 'static,
        {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let address = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
            let requests_for_task = requests.clone();
            let handler = Arc::new(handler);

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = &mut shutdown_rx => break,
                        accepted = listener.accept() => {
                            let Ok((mut stream, _)) = accepted else { break };
                            let requests = requests_for_task.clone();
                            let handler = handler.clone();
                            tokio::spawn(async move {
                                let mut buffer = vec![0u8; 16 * 1024];
                                let count = stream.read(&mut buffer).await.unwrap_or(0);
                                let request = String::from_utf8_lossy(&buffer[..count]).to_string();
                                let first_line = request.lines().next().unwrap_or_default().to_string();
                                let mut parts = first_line.split_whitespace();
                                let _method = parts.next().unwrap_or_default().to_string();
                                let path = parts.next().unwrap_or_default().to_string();
                                requests.lock().await.push(path.clone());
                                let (status, body) = handler(first_line, path);
                                let status_text = if status == 200 { "OK" } else { "ERR" };
                                let response = format!(
                                    "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    body.len(), body
                                );
                                let _ = stream.write_all(response.as_bytes()).await;
                            });
                        }
                    }
                }
            });

            Self {
                address,
                requests,
                shutdown: Arc::new(Mutex::new(Some(shutdown_tx))),
            }
        }

        async fn requests(&self) -> Vec<String> {
            self.requests.lock().await.clone()
        }

        async fn stop(&self) {
            if let Some(sender) = self.shutdown.lock().await.take() {
                let _ = sender.send(());
            }
        }
    }

    fn item(id: u32, title: &str) -> AnimeItem {
        AnimeItem {
            id,
            anilist_id: None,
            slug: format!("slug-{id}"),
            title_ukrainian: title.to_string(),
            title_original: None,
            title_english: None,
            status: "FINISHED".to_string(),
            anime_type: "TV".to_string(),
            year: Some(2024),
            has_ukrainian_dub: true,
            poster_url: None,
            episodes_count: None,
            description: None,
            rating: None,
            genres: None,
            dubbing_studios: None,
        }
    }

    fn page(items: Vec<AnimeItem>, page: u32, total_pages: u32) -> String {
        serde_json::to_string(&AnimeSearchResponse {
            total: items.len() as u32,
            page,
            page_size: 20,
            total_pages,
            items,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn basic_search_requests_one_page_and_filters_by_first_two_words() {
        let server = MockServer::start(|_, path| {
            let url = reqwest::Url::parse(&format!("http://localhost{path}"))
                .expect("mock request path must form a valid URL");
            let params = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
            assert_eq!(params.get("search"), Some(&"дівчина".to_string()));
            assert_eq!(params.get("has_ukrainian_dub"), Some(&"true".to_string()));
            assert_eq!(params.get("page_size"), Some(&"20".to_string()));
            match params.get("page").map(String::as_str) {
                Some("1") => (
                    200,
                    page(
                        vec![
                            item(1, "Дівчина напрокат"),
                            item(2, "Моя дівчина — монстр"),
                            item(3, "Супердівчина з космосу"),
                            item(4, "Та сама дівчина"),
                        ],
                        1,
                        42,
                    ),
                ),
                _ => (500, "{}".to_string()),
            }
        })
        .await;

        let client = ApiClient::with_base_urls(&server.address, &server.address).unwrap();
        let result = client.search_anime("дівчина").await.unwrap();
        assert_eq!(
            result.iter().map(|anime| anime.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        let requests = server.requests().await;
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("page=1"));
        server.stop().await;
    }

    #[tokio::test]
    async fn extended_search_reads_multiple_pages_without_strict_title_filter() {
        let server = MockServer::start(|_, path| {
            let url = reqwest::Url::parse(&format!("http://localhost{path}")).unwrap();
            let params = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
            match params.get("page").map(String::as_str) {
                Some("1") => (200, page(vec![item(1, "Зовсім інша назва")], 1, 3)),
                Some("2") => (200, page(vec![item(2, "Другий результат")], 2, 3)),
                Some("3") => (200, page(vec![item(3, "Третій результат")], 3, 3)),
                _ => (500, "{}".to_string()),
            }
        })
        .await;

        let client = ApiClient::with_base_urls(&server.address, &server.address).unwrap();
        let result = client
            .search_anime_with_mode("дівчина", true)
            .await
            .unwrap();
        assert_eq!(
            result.iter().map(|anime| anime.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(server.requests().await.len(), 3);
        server.stop().await;
    }

    #[test]
    fn basic_search_checks_alternative_titles_and_phrase_prefixes() {
        let mut anime = item(1, "Зовсім інша назва");
        anime.title_english = Some("Kaguya-sama: Love is War".to_string());

        assert!(basic_title_match(&anime, &normalize_search_text("kaguya")));
        assert!(basic_title_match(&anime, &normalize_search_text("sama")));
        assert!(basic_title_match(
            &anime,
            &normalize_search_text("Kaguya sama love")
        ));
        assert!(!basic_title_match(&anime, &normalize_search_text("love")));

        anime.title_ukrainian = "Моя дівчина напрокат".to_string();
        assert!(basic_title_match(
            &anime,
            &normalize_search_text("дівчина напрокат")
        ));
    }

    #[tokio::test]
    async fn lookup_distinguishes_404_from_rate_limit_and_server_error() {
        let server = MockServer::start(|_, path| {
            if path.contains("anilist_id=404") {
                return (404, "{}".to_string());
            }
            if path.contains("anilist_id=429") {
                return (429, "{}".to_string());
            }
            if path.contains("anilist_id=500") {
                return (500, "{}".to_string());
            }
            (200, page(Vec::new(), 1, 1))
        })
        .await;
        let client = ApiClient::with_base_urls(&server.address, &server.address).unwrap();

        assert_eq!(client.get_anime_by_anilist_id(404).await.unwrap(), None);
        let rate_limit = client.get_anime_by_anilist_id(429).await.unwrap_err();
        let server_error = client.get_anime_by_anilist_id(500).await.unwrap_err();
        assert_eq!(
            rate_limit
                .downcast_ref::<ApiError>()
                .and_then(ApiError::status),
            Some(429)
        );
        assert_eq!(
            server_error
                .downcast_ref::<ApiError>()
                .and_then(ApiError::status),
            Some(500)
        );
        server.stop().await;
    }

    #[test]
    fn search_metadata_is_optional_for_older_payloads() {
        let parsed: AnimeItem = serde_json::from_value(serde_json::json!({
            "id": 5048,
            "anilist_id": 108465,
            "slug": "mushoku-tensei",
            "title_ukrainian": "Реінкарнація безробітного",
            "title_original": "Mushoku Tensei",
            "title_english": "Mushoku Tensei",
            "status": "completed",
            "type": "tv",
            "year": 2021,
            "has_ukrainian_dub": true
        }))
        .unwrap();
        assert_eq!(parsed.poster_url, None);
        assert_eq!(parsed.episodes_count, None);
        assert_eq!(parsed.description, None);
        assert_eq!(parsed.rating, None);
        assert_eq!(parsed.genres, None);
        assert_eq!(parsed.dubbing_studios, None);
    }

    #[tokio::test]
    async fn anilist_batch_deduplicates_ids_and_returns_sorted_media() {
        let server = MockServer::start(|_, path| {
            assert_eq!(path, "/graphql");
            (
                200,
                serde_json::json!({
                    "data": {
                        "Page": {
                            "media": [
                                {
                                    "id": 127720,
                                    "type": "ANIME",
                                    "format": "TV",
                                    "seasonYear": 2021,
                                    "title": { "english": "Mushoku Tensei Part 2" },
                                    "coverImage": { "large": "poster-2" },
                                    "relations": { "edges": [] }
                                },
                                {
                                    "id": 108465,
                                    "type": "ANIME",
                                    "format": "TV",
                                    "seasonYear": 2021,
                                    "title": { "english": "Mushoku Tensei" },
                                    "coverImage": { "large": "poster-1" },
                                    "relations": {
                                        "edges": [{
                                            "relationType": "SEQUEL",
                                            "node": {
                                                "id": 127720,
                                                "type": "ANIME",
                                                "format": "TV",
                                                "seasonYear": 2021,
                                                "title": { "english": "Mushoku Tensei Part 2" },
                                                "coverImage": { "large": "poster-2" }
                                            }
                                        }]
                                    }
                                }
                            ]
                        }
                    }
                })
                .to_string(),
            )
        })
        .await;
        let client = ApiClient::with_endpoints(
            &server.address,
            &server.address,
            format!("{}/graphql", server.address),
        )
        .unwrap();
        let media = client
            .get_anilist_media_batch(&[127720, 108465, 127720, 0])
            .await
            .unwrap();
        assert_eq!(
            media.iter().map(|media| media.id).collect::<Vec<_>>(),
            vec![108465, 127720]
        );
        assert_eq!(media[0].relations.edges[0].relation_type, "SEQUEL");
        assert_eq!(server.requests().await.len(), 1);
        server.stop().await;
    }

    #[tokio::test]
    async fn release_source_loader_marks_browser_only_moonanime() {
        let server = MockServer::start(|_, path| {
            let url = reqwest::Url::parse(&format!("http://localhost{path}"))
                .expect("mock request path must form a valid URL");
            let params = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
            assert_eq!(params.get("season"), Some(&"2".to_string()));
            (
                200,
                serde_json::json!({
                    "ashdi": [],
                    "moonanime": [{
                        "id": 17,
                        "studio_name": "Moon Studio",
                        "season_number": 2,
                        "episodes_count": 1,
                        "episodes": [{
                            "episode_number": 1,
                            "display_episode_number": 1.0,
                            "title": "Episode 1",
                            "iframe_url": "https://moonanime.art/episode/1",
                            "poster_url": ""
                        }]
                    }]
                })
                .to_string(),
            )
        })
        .await;

        let client = ApiClient::with_base_urls(&server.address, &server.address).unwrap();
        let sources = client.get_release_sources(42, 2).await.unwrap();

        assert_eq!(server.requests().await.len(), 1);
        assert!(sources.ashdi.is_empty());
        assert!(sources.is_moonanime_only());
        assert_eq!(sources.moonanime[0].studio_name, "Moon Studio");
        assert_eq!(sources.moonanime[0].episodes_count, 1);
        assert_eq!(
            sources.moonanime[0].episodes[0].iframe_url,
            "https://moonanime.art/episode/1"
        );
        server.stop().await;
    }
}
