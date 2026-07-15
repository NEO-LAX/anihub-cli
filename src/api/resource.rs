//! Actor-owned resource loading.
//!
//! The UI can submit requests without owning semaphores, caches, or retry
//! state.  Every result carries the generation that requested it, so a view
//! can discard an old result without racing a newer view.

use super::client::{ApiClient, ApiError};
use super::franchise::AniListMedia;
use super::models::{AnimeDetails, AnimeItem, EpisodeSourcesKey, EpisodeSourcesResponse};
use image::DynamicImage;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;

const DEFAULT_COMMAND_CAPACITY: usize = 64;
const DEFAULT_EVENT_CAPACITY: usize = 64;
const DEFAULT_ANIHUB_CONCURRENCY: usize = 3;
const DEFAULT_ANIHUB_MAX_STARTS: usize = 40;
const DEFAULT_ANIHUB_WINDOW: Duration = Duration::from_secs(60);
const DEFAULT_RETRY_LIMIT: usize = 2;
const DEFAULT_RETRY_BACKOFF: Duration = Duration::from_millis(100);
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(300);
const DEFAULT_NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(30);

/// Monotonic id assigned to one submitted request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

impl RequestId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

impl From<u64> for RequestId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

/// Generation of the view that owns a request.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewGeneration(pub u64);

impl ViewGeneration {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for ViewGeneration {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

/// Resources supported by the worker.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceKey {
    Search(String),
    AniHubByAniList(u32),
    Details(u32),
    /// Episode sources requested explicitly for the currently opened view.
    /// This variant is intentionally absent from [`PrefetchHandle`].
    Sources(EpisodeSourcesKey),
    Poster(String),
}

impl ResourceKey {
    pub fn search(query: impl Into<String>) -> Self {
        Self::Search(query.into())
    }

    pub const fn details(anime_id: u32) -> Self {
        Self::Details(anime_id)
    }

    pub const fn anihub_by_anilist(anilist_id: u32) -> Self {
        Self::AniHubByAniList(anilist_id)
    }

    pub const fn sources(anime_id: u32, season: u32) -> Self {
        Self::Sources(EpisodeSourcesKey::new(anime_id, season))
    }

    pub fn poster(url: impl Into<String>) -> Self {
        Self::Poster(url.into())
    }

    fn uses_anihub(&self) -> bool {
        true
    }
}

/// Typed values carried by completion events and stored in the completed
/// cache.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum ResourceValue {
    Search(SearchResultBundle),
    AniHubId(Option<u32>),
    Details(AnimeDetails),
    Sources(EpisodeSourcesResponse),
    Poster(DynamicImage),
}

#[derive(Debug, Clone)]
pub struct SearchResultBundle {
    pub items: Vec<AnimeItem>,
    pub anilist_media: Vec<AniListMedia>,
}

/// Worker failure model.  HTTP status and retry-after are retained for
/// callers that want to display or instrument failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LoadError {
    #[error("resource not found")]
    NotFound,
    #[error("HTTP {status}: {message}")]
    Http {
        status: u16,
        message: String,
        retry_after: Option<Duration>,
    },
    #[error("network error: {0}")]
    Network(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("unsupported resource: {0}")]
    Unsupported(String),
    #[error("resource worker is shutting down")]
    Shutdown,
}

impl LoadError {
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Http { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    pub fn is_transient(&self) -> bool {
        match self {
            Self::Network(_) => true,
            Self::Http { status, .. } => *status == 429 || (500..=599).contains(status),
            _ => false,
        }
    }
}

#[derive(Debug, Error)]
pub enum ResourceCommandError {
    #[error("resource command channel is closed")]
    Closed,
}

/// Bounded actor command contract.
#[derive(Debug, Clone)]
pub enum ResourceCommand {
    Load {
        request_id: RequestId,
        generation: ViewGeneration,
        key: ResourceKey,
    },
    CancelGeneration {
        generation: ViewGeneration,
    },
    Shutdown,
}

/// Bounded actor event contract.  Both success and failure carry the request
/// generation and key, including cache hits.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum ResourceEvent {
    Completed {
        request_id: RequestId,
        generation: ViewGeneration,
        key: ResourceKey,
        value: ResourceValue,
    },
    Failed {
        request_id: RequestId,
        generation: ViewGeneration,
        key: ResourceKey,
        error: LoadError,
    },
}

#[derive(Debug, Clone)]
pub struct ResourceWorkerConfig {
    pub command_capacity: usize,
    pub event_capacity: usize,
    pub anihub_max_concurrency: usize,
    pub anihub_max_starts: usize,
    pub anihub_window: Duration,
    pub retry_limit: usize,
    pub retry_backoff: Duration,
    pub completed_cache_ttl: Duration,
    pub negative_cache_ttl: Duration,
}

impl Default for ResourceWorkerConfig {
    fn default() -> Self {
        Self {
            command_capacity: DEFAULT_COMMAND_CAPACITY,
            event_capacity: DEFAULT_EVENT_CAPACITY,
            anihub_max_concurrency: DEFAULT_ANIHUB_CONCURRENCY,
            anihub_max_starts: DEFAULT_ANIHUB_MAX_STARTS,
            anihub_window: DEFAULT_ANIHUB_WINDOW,
            retry_limit: DEFAULT_RETRY_LIMIT,
            retry_backoff: DEFAULT_RETRY_BACKOFF,
            completed_cache_ttl: DEFAULT_CACHE_TTL,
            negative_cache_ttl: DEFAULT_NEGATIVE_CACHE_TTL,
        }
    }
}

/// Cloneable command-side handle.  The event receiver deliberately remains
/// single-owner so event ordering is unambiguous.
#[derive(Clone)]
pub struct ResourceHandle {
    command_tx: mpsc::Sender<ResourceCommand>,
    next_request_id: Arc<AtomicU64>,
}

impl ResourceHandle {
    pub async fn load(
        &self,
        generation: ViewGeneration,
        key: ResourceKey,
    ) -> Result<RequestId, ResourceCommandError> {
        let request_id = self.allocate_request_id();
        self.command_tx
            .send(ResourceCommand::Load {
                request_id,
                generation,
                key,
            })
            .await
            .map_err(|_| ResourceCommandError::Closed)?;
        Ok(request_id)
    }

    pub async fn cancel_generation(
        &self,
        generation: ViewGeneration,
    ) -> Result<(), ResourceCommandError> {
        self.command_tx
            .send(ResourceCommand::CancelGeneration { generation })
            .await
            .map_err(|_| ResourceCommandError::Closed)
    }

    pub async fn shutdown(&self) -> Result<(), ResourceCommandError> {
        self.command_tx
            .send(ResourceCommand::Shutdown)
            .await
            .map_err(|_| ResourceCommandError::Closed)
    }

    fn allocate_request_id(&self) -> RequestId {
        RequestId::new(self.next_request_id.fetch_add(1, Ordering::Relaxed))
    }
}

pub struct ResourceWorkerRuntime {
    pub handle: ResourceHandle,
    pub events: mpsc::Receiver<ResourceEvent>,
    pub join_handle: tokio::task::JoinHandle<()>,
}

impl ResourceWorkerRuntime {
    pub async fn shutdown(self) -> Result<(), ResourceCommandError> {
        self.handle.shutdown().await?;
        let _ = self.join_handle.await;
        Ok(())
    }
}

pub struct ResourceWorker;

impl ResourceWorker {
    pub fn spawn(api_client: ApiClient) -> ResourceWorkerRuntime {
        Self::spawn_with_config(api_client, ResourceWorkerConfig::default())
    }

    pub fn spawn_with_config(
        api_client: ApiClient,
        config: ResourceWorkerConfig,
    ) -> ResourceWorkerRuntime {
        let command_capacity = config.command_capacity.max(1);
        let event_capacity = config.event_capacity.max(1);
        let (command_tx, command_rx) = mpsc::channel(command_capacity);
        let (event_tx, event_rx) = mpsc::channel(event_capacity);
        let handle = ResourceHandle {
            command_tx,
            next_request_id: Arc::new(AtomicU64::new(1)),
        };
        let actor = Actor::new(api_client, command_rx, event_tx, config);
        let join_handle = tokio::spawn(actor.run());
        ResourceWorkerRuntime {
            handle,
            events: event_rx,
            join_handle,
        }
    }
}

#[derive(Clone)]
struct Work {
    key: ResourceKey,
}

#[derive(Clone, Copy)]
struct Waiter {
    request_id: RequestId,
    generation: ViewGeneration,
}

struct InFlight {
    work: Work,
    waiters: Vec<Waiter>,
}

struct CacheEntry<T> {
    inserted_at: Instant,
    value: T,
}

struct TaskOutcome {
    key: ResourceKey,
    result: Result<ResourceValue, LoadError>,
}

struct HubLimiter {
    max_starts: usize,
    window: Duration,
    starts: Mutex<VecDeque<Instant>>,
}

impl HubLimiter {
    fn new(config: &ResourceWorkerConfig) -> Self {
        Self {
            max_starts: config.anihub_max_starts.max(1),
            window: config.anihub_window,
            starts: Mutex::new(VecDeque::new()),
        }
    }

    async fn acquire_start(&self) {
        loop {
            let wait = {
                let mut starts = self.starts.lock().await;
                let now = Instant::now();
                while starts
                    .front()
                    .is_some_and(|started| now.duration_since(*started) >= self.window)
                {
                    starts.pop_front();
                }

                if starts.len() < self.max_starts {
                    starts.push_back(now);
                    None
                } else {
                    starts
                        .front()
                        .map(|started| self.window.saturating_sub(now.duration_since(*started)))
                }
            };

            match wait {
                None => return,
                Some(duration) if duration.is_zero() => tokio::task::yield_now().await,
                Some(duration) => tokio::time::sleep(duration).await,
            }
        }
    }
}

struct Actor {
    api_client: ApiClient,
    command_rx: mpsc::Receiver<ResourceCommand>,
    event_tx: mpsc::Sender<ResourceEvent>,
    config: ResourceWorkerConfig,
    pending: VecDeque<ResourceKey>,
    in_flight: HashMap<ResourceKey, InFlight>,
    completed: HashMap<ResourceKey, CacheEntry<ResourceValue>>,
    negative: HashMap<ResourceKey, CacheEntry<LoadError>>,
    canceled_generations: HashSet<ViewGeneration>,
    tasks: JoinSet<TaskOutcome>,
    running_anihub: usize,
    shutting_down: bool,
    hub_limiter: Arc<HubLimiter>,
}

impl Actor {
    fn new(
        api_client: ApiClient,
        command_rx: mpsc::Receiver<ResourceCommand>,
        event_tx: mpsc::Sender<ResourceEvent>,
        config: ResourceWorkerConfig,
    ) -> Self {
        Self {
            api_client,
            command_rx,
            event_tx,
            hub_limiter: Arc::new(HubLimiter::new(&config)),
            config,
            pending: VecDeque::new(),
            in_flight: HashMap::new(),
            completed: HashMap::new(),
            negative: HashMap::new(),
            canceled_generations: HashSet::new(),
            tasks: JoinSet::new(),
            running_anihub: 0,
            shutting_down: false,
        }
    }

    async fn run(mut self) {
        loop {
            self.schedule_ready();

            if self.shutting_down && self.tasks.is_empty() {
                break;
            }

            let have_tasks = !self.tasks.is_empty();
            tokio::select! {
                command = self.command_rx.recv() => {
                    match command {
                        Some(command) => self.handle_command(command).await,
                        None => self.begin_shutdown().await,
                    }
                }
                outcome = self.tasks.join_next(), if have_tasks => {
                    if let Some(Ok(outcome)) = outcome {
                        self.handle_outcome(outcome).await;
                    }
                }
            }
        }
    }

    fn schedule_ready(&mut self) {
        if self.shutting_down {
            return;
        }

        while self.running_anihub < self.config.anihub_max_concurrency.max(1) {
            let Some(key) = self.pending.pop_front() else {
                break;
            };
            let Some(in_flight) = self.in_flight.get(&key) else {
                continue;
            };
            if in_flight.waiters.is_empty() {
                self.in_flight.remove(&key);
                continue;
            }

            let work = in_flight.work.clone();
            if work.key.uses_anihub() {
                self.running_anihub += 1;
            }
            let api_client = self.api_client.clone();
            let config = self.config.clone();
            let hub_limiter = self.hub_limiter.clone();
            self.tasks.spawn(async move {
                let result = load_with_retries(api_client, work.clone(), config, hub_limiter).await;
                TaskOutcome {
                    key: work.key,
                    result,
                }
            });
        }
    }

    async fn handle_command(&mut self, command: ResourceCommand) {
        match command {
            ResourceCommand::Load {
                request_id,
                generation,
                key,
            } => {
                self.enqueue(
                    Waiter {
                        request_id,
                        generation,
                    },
                    Work { key },
                )
                .await;
            }
            ResourceCommand::CancelGeneration { generation } => {
                self.cancel_generation(generation);
            }
            ResourceCommand::Shutdown => self.begin_shutdown().await,
        }
    }

    async fn enqueue(&mut self, waiter: Waiter, work: Work) {
        if self.shutting_down {
            self.emit_failed(waiter, work.key, LoadError::Shutdown)
                .await;
            return;
        }
        if self.canceled_generations.contains(&waiter.generation) {
            return;
        }

        let key = work.key.clone();
        if let Some(value) = self.cached_completed(&key) {
            self.emit_completed(waiter, key, value).await;
            return;
        }
        if let Some(error) = self.cached_negative(&key) {
            self.emit_failed(waiter, key, error).await;
            return;
        }

        if let Some(in_flight) = self.in_flight.get_mut(&key) {
            in_flight.waiters.push(waiter);
            return;
        }

        self.in_flight.insert(
            key.clone(),
            InFlight {
                work,
                waiters: vec![waiter],
            },
        );
        self.pending.push_back(key);
    }

    fn cancel_generation(&mut self, generation: ViewGeneration) {
        self.canceled_generations.insert(generation);
        for in_flight in self.in_flight.values_mut() {
            in_flight
                .waiters
                .retain(|waiter| waiter.generation != generation);
        }
        self.pending.retain(|key| {
            self.in_flight
                .get(key)
                .is_some_and(|in_flight| !in_flight.waiters.is_empty())
        });
    }

    async fn begin_shutdown(&mut self) {
        if self.shutting_down {
            return;
        }
        self.shutting_down = true;
        let pending = std::mem::take(&mut self.pending);
        for key in pending {
            if let Some(in_flight) = self.in_flight.remove(&key) {
                for waiter in in_flight.waiters {
                    self.emit_failed(waiter, key.clone(), LoadError::Shutdown)
                        .await;
                }
            }
        }
    }

    async fn handle_outcome(&mut self, outcome: TaskOutcome) {
        if outcome.key.uses_anihub() {
            self.running_anihub = self.running_anihub.saturating_sub(1);
        }
        let Some(in_flight) = self.in_flight.remove(&outcome.key) else {
            return;
        };

        match outcome.result {
            Ok(value) => {
                self.completed.insert(
                    outcome.key.clone(),
                    CacheEntry {
                        inserted_at: Instant::now(),
                        value: value.clone(),
                    },
                );
                for waiter in in_flight.waiters {
                    if !self.canceled_generations.contains(&waiter.generation) {
                        self.emit_completed(waiter, outcome.key.clone(), value.clone())
                            .await;
                    }
                }
            }
            Err(error) => {
                self.negative.insert(
                    outcome.key.clone(),
                    CacheEntry {
                        inserted_at: Instant::now(),
                        value: error.clone(),
                    },
                );
                for waiter in in_flight.waiters {
                    if !self.canceled_generations.contains(&waiter.generation) {
                        self.emit_failed(waiter, outcome.key.clone(), error.clone())
                            .await;
                    }
                }
            }
        }
    }

    fn cached_completed(&mut self, key: &ResourceKey) -> Option<ResourceValue> {
        if self.config.completed_cache_ttl.is_zero() {
            return None;
        }
        let expired = self
            .completed
            .get(key)
            .is_some_and(|entry| entry.inserted_at.elapsed() >= self.config.completed_cache_ttl);
        if expired {
            self.completed.remove(key);
            None
        } else {
            self.completed.get(key).map(|entry| entry.value.clone())
        }
    }

    fn cached_negative(&mut self, key: &ResourceKey) -> Option<LoadError> {
        if self.config.negative_cache_ttl.is_zero() {
            return None;
        }
        let expired = self
            .negative
            .get(key)
            .is_some_and(|entry| entry.inserted_at.elapsed() >= self.config.negative_cache_ttl);
        if expired {
            self.negative.remove(key);
            None
        } else {
            self.negative.get(key).map(|entry| entry.value.clone())
        }
    }

    async fn emit_completed(&self, waiter: Waiter, key: ResourceKey, value: ResourceValue) {
        let _ = self
            .event_tx
            .send(ResourceEvent::Completed {
                request_id: waiter.request_id,
                generation: waiter.generation,
                key,
                value,
            })
            .await;
    }

    async fn emit_failed(&self, waiter: Waiter, key: ResourceKey, error: LoadError) {
        let _ = self
            .event_tx
            .send(ResourceEvent::Failed {
                request_id: waiter.request_id,
                generation: waiter.generation,
                key,
                error,
            })
            .await;
    }
}

async fn load_with_retries(
    api_client: ApiClient,
    work: Work,
    config: ResourceWorkerConfig,
    hub_limiter: Arc<HubLimiter>,
) -> Result<ResourceValue, LoadError> {
    let mut retry_number = 0usize;
    loop {
        if work.key.uses_anihub() {
            hub_limiter.acquire_start().await;
        }
        match load_once(&api_client, &work).await {
            Ok(value) => return Ok(value),
            Err(error) if error.is_transient() && retry_number < config.retry_limit => {
                let exponential = config
                    .retry_backoff
                    .checked_mul(2u32.saturating_pow(retry_number as u32))
                    .unwrap_or(config.retry_backoff);
                let delay = error.retry_after().unwrap_or(exponential);
                tokio::time::sleep(delay).await;
                retry_number += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn load_once(api_client: &ApiClient, work: &Work) -> Result<ResourceValue, LoadError> {
    match &work.key {
        ResourceKey::Search(query) => {
            let items = api_client
                .search_anime(query)
                .await
                .map_err(classify_error)?;
            let anilist_ids = items
                .iter()
                .filter_map(|item| item.anilist_id)
                .collect::<Vec<_>>();
            // Franchise enrichment is optional: AniHub search remains useful
            // during an AniList outage, and the UI falls back to conservative
            // one-release catalogs instead of failing the entire search.
            let anilist_media = api_client
                .get_anilist_media_batch(&anilist_ids)
                .await
                .unwrap_or_default();
            Ok(ResourceValue::Search(SearchResultBundle {
                items,
                anilist_media,
            }))
        }
        ResourceKey::AniHubByAniList(anilist_id) => api_client
            .get_anime_by_anilist_id(*anilist_id)
            .await
            .map(ResourceValue::AniHubId)
            .map_err(classify_error),
        ResourceKey::Details(anime_id) => api_client
            .get_anime_details(*anime_id)
            .await
            .map(ResourceValue::Details)
            .map_err(classify_error),
        ResourceKey::Sources(key) => api_client
            .get_release_sources(key.anime_id, key.season)
            .await
            .map(ResourceValue::Sources)
            .map_err(classify_error),
        ResourceKey::Poster(url) => api_client
            .fetch_poster(url)
            .await
            .map(ResourceValue::Poster)
            .map_err(classify_error),
    }
}

fn classify_error(error: anyhow::Error) -> LoadError {
    for cause in error.chain() {
        if let Some(api_error) = cause.downcast_ref::<ApiError>() {
            if api_error.is_not_found() {
                return LoadError::NotFound;
            }
            if let Some(status) = api_error.status() {
                return LoadError::Http {
                    status,
                    message: error.to_string(),
                    retry_after: api_error.retry_after(),
                };
            }
            return match api_error {
                ApiError::Transport { .. } => LoadError::Network(error.to_string()),
                ApiError::Parse { .. } => LoadError::Parse(error.to_string()),
                ApiError::Decode { .. } => LoadError::Decode(error.to_string()),
                ApiError::Http { .. } => LoadError::Http {
                    status: api_error.status().unwrap_or(0),
                    message: error.to_string(),
                    retry_after: api_error.retry_after(),
                },
            };
        }
    }
    LoadError::Unsupported(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::timeout;

    struct TestServer {
        url: String,
        requests: Arc<AtomicUsize>,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
    }

    impl TestServer {
        async fn start(status: u16, delay: Duration) -> Self {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(AtomicUsize::new(0));
            let requests_for_task = requests.clone();
            let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = &mut stop_rx => break,
                        accepted = listener.accept() => {
                            let Ok((mut stream, _)) = accepted else { break };
                            let requests = requests_for_task.clone();
                            tokio::spawn(async move {
                                let mut buffer = [0u8; 8192];
                                let _ = stream.read(&mut buffer).await;
                                requests.fetch_add(1, Ordering::Relaxed);
                                tokio::time::sleep(delay).await;
                                let body = if status == 200 {
                                    json!({
                                        "id": 7,
                                        "anilist_id": null,
                                        "slug": "seven",
                                        "title_ukrainian": "Seven",
                                        "title_original": "Seven",
                                        "title_english": "Seven",
                                        "status": "FINISHED",
                                        "type": "TV",
                                        "year": 2024,
                                        "has_ukrainian_dub": true,
                                        "poster_url": null,
                                        "episodes_count": 1,
                                        "description": null,
                                        "rating": null,
                                        "genres": null,
                                        "dubbing_studios": null
                                    }).to_string()
                                } else {
                                    "{}".to_string()
                                };
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
                url,
                requests,
                stop: Some(stop_tx),
            }
        }

        fn request_count(&self) -> usize {
            self.requests.load(Ordering::Relaxed)
        }

        async fn stop(mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
        }
    }

    fn test_config() -> ResourceWorkerConfig {
        ResourceWorkerConfig {
            command_capacity: 8,
            event_capacity: 8,
            anihub_max_concurrency: 3,
            anihub_max_starts: 40,
            anihub_window: Duration::from_secs(60),
            retry_limit: 0,
            retry_backoff: Duration::from_millis(1),
            completed_cache_ttl: Duration::from_secs(30),
            negative_cache_ttl: Duration::from_secs(30),
        }
    }

    #[tokio::test]
    async fn same_key_is_single_flight_and_completed_cache_is_reused() {
        let server = TestServer::start(200, Duration::from_millis(30)).await;
        let api = ApiClient::with_base_urls(&server.url, &server.url).unwrap();
        let mut runtime = ResourceWorker::spawn_with_config(api, test_config());
        let generation = ViewGeneration::new(1);

        runtime
            .handle
            .load(generation, ResourceKey::Details(7))
            .await
            .unwrap();
        runtime
            .handle
            .load(generation, ResourceKey::Details(7))
            .await
            .unwrap();

        let first = timeout(Duration::from_secs(2), runtime.events.recv())
            .await
            .unwrap()
            .unwrap();
        let second = timeout(Duration::from_secs(2), runtime.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(first, ResourceEvent::Completed { .. }));
        assert!(matches!(second, ResourceEvent::Completed { .. }));
        assert_eq!(server.request_count(), 1);

        runtime
            .handle
            .load(ViewGeneration::new(2), ResourceKey::Details(7))
            .await
            .unwrap();
        let cached = timeout(Duration::from_secs(1), runtime.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            cached,
            ResourceEvent::Completed {
                generation: ViewGeneration(2),
                ..
            }
        ));
        assert_eq!(server.request_count(), 1);

        runtime.shutdown().await.unwrap();
        server.stop().await;
    }

    #[tokio::test]
    async fn canceled_generation_does_not_receive_stale_result() {
        let server = TestServer::start(200, Duration::from_millis(100)).await;
        let api = ApiClient::with_base_urls(&server.url, &server.url).unwrap();
        let mut runtime = ResourceWorker::spawn_with_config(api, test_config());
        let generation = ViewGeneration::new(9);
        runtime
            .handle
            .load(generation, ResourceKey::Details(7))
            .await
            .unwrap();
        runtime.handle.cancel_generation(generation).await.unwrap();

        assert!(
            timeout(Duration::from_millis(250), runtime.events.recv())
                .await
                .is_err()
        );
        runtime.shutdown().await.unwrap();
        server.stop().await;
    }

    #[tokio::test]
    async fn not_found_is_negative_cached() {
        let server = TestServer::start(404, Duration::ZERO).await;
        let api = ApiClient::with_base_urls(&server.url, &server.url).unwrap();
        let mut runtime = ResourceWorker::spawn_with_config(api, test_config());
        let generation = ViewGeneration::new(3);
        runtime
            .handle
            .load(generation, ResourceKey::Details(7))
            .await
            .unwrap();
        let first = timeout(Duration::from_secs(1), runtime.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            first,
            ResourceEvent::Failed {
                error: LoadError::NotFound,
                ..
            }
        ));

        runtime
            .handle
            .load(generation, ResourceKey::Details(7))
            .await
            .unwrap();
        let second = timeout(Duration::from_secs(1), runtime.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            second,
            ResourceEvent::Failed {
                error: LoadError::NotFound,
                ..
            }
        ));
        assert_eq!(server.request_count(), 1);
        runtime.shutdown().await.unwrap();
        server.stop().await;
    }

    #[tokio::test]
    async fn start_gate_uses_a_rolling_window() {
        let config = ResourceWorkerConfig {
            anihub_max_starts: 1,
            anihub_window: Duration::from_millis(25),
            ..test_config()
        };
        let limiter = HubLimiter::new(&config);
        limiter.acquire_start().await;
        let started = Instant::now();
        limiter.acquire_start().await;
        assert!(started.elapsed() >= Duration::from_millis(20));
    }
}
