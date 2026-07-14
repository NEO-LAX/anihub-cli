mod api;
mod moonanime;
mod platform;
mod playback;
mod player;
mod storage;
mod ui;

use anyhow::Result;

/// Пише debug-повідомлення у файл, не в stderr — щоб не ламати TUI.
fn debug_log(msg: &str) {
    use std::io::Write;
    let mut path = std::env::temp_dir();
    path.push("anihub_debug.log");
    let _ = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .and_then(|mut f| writeln!(f, "{msg}"));
}
use crate::playback::*;
use api::{
    EpisodeSourcesResponse, RequestId, ResourceEvent, ResourceKey, ResourceValue, ResourceWorker,
    ResourceWorkerRuntime, ViewGeneration,
};
use crossterm::{
    ExecutableCommand,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::collections::{HashMap, HashSet};
use std::io::stdout;
use ui::{AppMode, AppState, FocusPanel};

#[derive(Clone, Debug, PartialEq, Eq)]
enum ResourceContext {
    Search(String),
    Content {
        representative_id: u32,
        anime_ids: Vec<u32>,
        details_id: u32,
    },
}

struct CombinedPending {
    generation: ViewGeneration,
    representative_id: u32,
    order: Vec<u32>,
    results: Vec<(u32, EpisodeSourcesResponse)>,
    waiting: HashSet<u32>,
    submitted: bool,
}

struct PendingContinue {
    progress: storage::WatchProgress,
    in_library: bool,
}

struct ResourceCoordinator {
    runtime: ResourceWorkerRuntime,
    generation: ViewGeneration,
    context: Option<ResourceContext>,
    combined: Option<CombinedPending>,
    poster_requests: HashMap<RequestId, u32>,
    posters_in_flight: HashSet<u32>,
    pending_continue: Option<PendingContinue>,
    ready_playback: Option<(PlayTarget, Vec<PlayTarget>)>,
}

impl ResourceCoordinator {
    fn new(client: api::ApiClient) -> Self {
        Self {
            runtime: ResourceWorker::spawn(client),
            generation: ViewGeneration::default(),
            context: None,
            combined: None,
            poster_requests: HashMap::new(),
            posters_in_flight: HashSet::new(),
            pending_continue: None,
            ready_playback: None,
        }
    }

    async fn sync(&mut self, app: &mut AppState) {
        let desired = self.desired_context(app);
        if desired != self.context {
            let previous = self.generation;
            self.generation = ViewGeneration::new(previous.get().saturating_add(1));
            if previous.get() != 0 {
                let _ = self.runtime.handle.cancel_generation(previous).await;
            }
            self.context = desired.clone();
            self.combined = None;
            self.poster_requests.clear();
            self.posters_in_flight.clear();

            if let Some(context) = desired {
                self.start_context(app, context).await;
            }
        }

        self.schedule_poster(app).await;
        self.finish_continue_if_ready(app);
    }

    fn desired_context(&self, app: &AppState) -> Option<ResourceContext> {
        if let Some(pending) = &self.pending_continue {
            return Some(ResourceContext::Content {
                representative_id: pending.progress.anime_id,
                anime_ids: vec![pending.progress.anime_id],
                details_id: pending.progress.anime_id,
            });
        }
        desired_resource_context(app)
    }

    async fn request_continue(&mut self, app: &mut AppState, request: ui::ContinueRequest) {
        let (progress, in_library) = match request {
            ui::ContinueRequest::Latest => (
                app.history
                    .progress
                    .values()
                    .max_by_key(|progress| progress.updated_at)
                    .cloned(),
                false,
            ),
            ui::ContinueRequest::Group {
                anime_ids,
                in_library,
            } => (
                app.history
                    .progress
                    .values()
                    .filter(|progress| anime_ids.contains(&progress.anime_id))
                    .max_by_key(|progress| progress.updated_at)
                    .cloned(),
                in_library,
            ),
        };
        let Some(progress) = progress else {
            app.set_info_status("Немає збереженого прогресу");
            return;
        };
        self.pending_continue = Some(PendingContinue {
            progress,
            in_library,
        });
        self.context = None;
        app.set_activity("Підготовка продовження перегляду…");
        self.sync(app).await;
    }

    fn finish_continue_if_ready(&mut self, app: &mut AppState) {
        let Some(pending) = self.pending_continue.as_ref() else {
            return;
        };
        let Some(details) = app.details_cache.get(&pending.progress.anime_id) else {
            return;
        };
        let Some(sources) = app.sources_cache.get(&pending.progress.anime_id) else {
            return;
        };
        let Some(resolved) = resolve_continue_target(&pending.progress, &sources) else {
            app.clear_activity();
            app.set_info_status("Усі серії переглянуто");
            self.pending_continue = None;
            return;
        };
        if pending.in_library {
            apply_library_continue_context(app, &pending.progress, &details, &sources, &resolved);
        } else {
            apply_continue_context(app, &details, &sources, &resolved);
        }
        let is_moonanime = resolved.url.starts_with("https://moonanime.art");
        let target = PlayTarget {
            anime_id: pending.progress.anime_id,
            anime_title: pending.progress.anime_title.clone(),
            player_title: format!(
                "{} ({})",
                details.title_ukrainian,
                details.year.unwrap_or(0)
            ),
            season: resolved.season,
            episode: resolved.episode,
            episode_title: format!("Серія {}", resolved.episode),
            stream_page_url: resolved.url,
            start_time: resolved.start_time,
            studio_name: resolved.studio_name,
            referrer: if is_moonanime {
                "https://moonanime.art/".to_string()
            } else {
                "https://ashdi.vip/".to_string()
            },
        };
        let queue = build_playback_queue(app, &target);
        self.ready_playback = Some((target, queue));
        self.pending_continue = None;
        app.clear_activity();
    }

    fn take_ready_playback(&mut self) -> Option<(PlayTarget, Vec<PlayTarget>)> {
        self.ready_playback.take()
    }

    async fn start_context(&mut self, app: &mut AppState, context: ResourceContext) {
        match context {
            ResourceContext::Search(query) => {
                app.set_activity("Пошук аніме…");
                if self
                    .runtime
                    .handle
                    .load(self.generation, ResourceKey::search(query))
                    .await
                    .is_err()
                {
                    app.clear_activity();
                    app.set_error_status("Сервіс завантаження недоступний");
                }
            }
            ResourceContext::Content {
                representative_id,
                anime_ids,
                details_id,
            } => {
                app.set_activity("Завантаження метаданих і серій…");
                if let Some(details) = app.details_cache.get(&details_id) {
                    app.current_details = Some(details);
                } else {
                    let _ = self
                        .runtime
                        .handle
                        .load(self.generation, ResourceKey::details(details_id))
                        .await;
                }

                if let Some((sources, owners)) = app.combined_sources_cache.get(&representative_id)
                {
                    app.current_sources = Some(sources);
                    app.studio_anime_ids = owners;
                    app.clear_activity();
                    return;
                }

                let mut results = Vec::new();
                let mut waiting = HashSet::new();
                for anime_id in &anime_ids {
                    if let Some(sources) = app.sources_cache.get(anime_id) {
                        results.push((*anime_id, sources));
                    } else {
                        waiting.insert(*anime_id);
                        let _ = self
                            .runtime
                            .handle
                            .load(self.generation, ResourceKey::sources(*anime_id))
                            .await;
                    }
                }
                self.combined = Some(CombinedPending {
                    generation: self.generation,
                    representative_id,
                    order: anime_ids,
                    results,
                    waiting,
                    submitted: false,
                });
                self.submit_combined_if_ready(app).await;
            }
        }
    }

    async fn schedule_poster(&mut self, app: &mut AppState) {
        let Some(anime_id) = app.poster_fetch_pending.take() else {
            return;
        };
        if let Some(image) = app.poster_cache.get(&anime_id) {
            app.current_poster = Some(app.picker.new_resize_protocol((*image).clone()));
            return;
        }
        if self.posters_in_flight.contains(&anime_id) {
            return;
        }
        let poster_url = app
            .details_cache
            .get(&anime_id)
            .and_then(|details| details.poster_url.clone())
            .or_else(|| {
                app.current_details
                    .as_ref()
                    .filter(|details| details.id == anime_id)
                    .and_then(|details| details.poster_url.clone())
            });
        let Some(url) = poster_url else {
            app.poster_fetch_pending = Some(anime_id);
            return;
        };
        match self
            .runtime
            .handle
            .load(self.generation, ResourceKey::poster(url))
            .await
        {
            Ok(request_id) => {
                self.poster_requests.insert(request_id, anime_id);
                self.posters_in_flight.insert(anime_id);
            }
            Err(_) => app.set_error_status("Не вдалося поставити постер у чергу"),
        }
    }

    async fn drain(&mut self, app: &mut AppState) {
        let mut events = Vec::new();
        while let Ok(event) = self.runtime.events.try_recv() {
            events.push(event);
        }
        for event in events {
            self.apply_event(app, event).await;
        }
    }

    async fn apply_event(&mut self, app: &mut AppState, event: ResourceEvent) {
        let (request_id, generation, key, result) = match event {
            ResourceEvent::Completed {
                request_id,
                generation,
                key,
                value,
            } => (request_id, generation, key, Ok(value)),
            ResourceEvent::Failed {
                request_id,
                generation,
                key,
                error,
            } => (request_id, generation, key, Err(error.to_string())),
        };
        if generation != self.generation {
            return;
        }

        match (key, result) {
            (ResourceKey::Search(_), Ok(ResourceValue::Search(results))) => {
                apply_search_results(app, api::deduplicate_anime(results));
            }
            (ResourceKey::Search(_), Err(error)) => {
                app.clear_activity();
                app.set_error_status(format!("Помилка пошуку: {error}"));
            }
            (ResourceKey::Details(anime_id), Ok(ResourceValue::Details(details))) => {
                app.details_cache.insert(anime_id, details.clone());
                for item in app
                    .library_all_items
                    .iter_mut()
                    .chain(app.library_items.iter_mut())
                    .filter(|item| item.anime_ids.contains(&anime_id))
                {
                    if item.anime_title.starts_with("Закладка #") {
                        item.anime_title.clone_from(&details.title_ukrainian);
                        item.latest_progress.anime_title = details.title_ukrainian.clone();
                    }
                }
                let is_current = matches!(
                    self.context,
                    Some(ResourceContext::Content { details_id, .. }) if details_id == anime_id
                );
                if is_current {
                    app.current_details = Some(details);
                    if app.current_poster.is_none() {
                        app.poster_fetch_pending = Some(anime_id);
                    }
                }
            }
            (ResourceKey::Details(_), Err(error)) => {
                app.set_error_status(format!("Помилка метаданих: {error}"));
            }
            (ResourceKey::Sources(anime_id), Ok(ResourceValue::Sources(sources))) => {
                app.sources_cache.insert(anime_id, sources.clone());
                if let Some(pending) = self.combined.as_mut() {
                    if pending.generation == generation && pending.waiting.remove(&anime_id) {
                        pending.results.push((anime_id, sources));
                    }
                }
                self.submit_combined_if_ready(app).await;
            }
            (ResourceKey::Sources(anime_id), Err(error)) => {
                if let Some(pending) = self.combined.as_mut() {
                    pending.waiting.remove(&anime_id);
                }
                app.set_error_status(format!("Не вдалося завантажити частину серій: {error}"));
                self.submit_combined_if_ready(app).await;
            }
            (
                ResourceKey::Combined {
                    representative_id, ..
                },
                Ok(ResourceValue::Combined(value)),
            ) => {
                let (sources, owners) = value.into_legacy();
                if sources.ashdi.is_empty() {
                    app.set_info_status("Джерела серій відсутні");
                }
                app.combined_sources_cache
                    .insert(representative_id, (sources.clone(), owners.clone()));
                app.current_sources = Some(sources);
                app.studio_anime_ids = owners;
                app.clear_activity();
                self.combined = None;
            }
            (ResourceKey::Combined { .. }, Err(error)) => {
                app.clear_activity();
                app.set_error_status(format!("Помилка об’єднання серій: {error}"));
                self.combined = None;
            }
            (ResourceKey::Poster(_), Ok(ResourceValue::Poster(image))) => {
                if let Some(anime_id) = self.poster_requests.remove(&request_id) {
                    self.posters_in_flight.remove(&anime_id);
                    let image = std::sync::Arc::new(image);
                    app.poster_cache.insert(anime_id, image.clone());
                    app.current_poster = Some(app.picker.new_resize_protocol((*image).clone()));
                }
            }
            (ResourceKey::Poster(_), Err(_)) => {
                if let Some(anime_id) = self.poster_requests.remove(&request_id) {
                    self.posters_in_flight.remove(&anime_id);
                }
            }
            _ => {}
        }
    }

    async fn submit_combined_if_ready(&mut self, app: &mut AppState) {
        let Some(pending) = self.combined.as_mut() else {
            return;
        };
        if !pending.waiting.is_empty() || pending.submitted {
            return;
        }
        pending.submitted = true;
        if self
            .runtime
            .handle
            .load_combined(
                pending.generation,
                pending.representative_id,
                pending.order.clone(),
                pending.results.clone(),
            )
            .await
            .is_err()
        {
            app.clear_activity();
            app.set_error_status("Сервіс об’єднання серій недоступний");
        }
    }

    async fn shutdown(self) {
        let _ = self.runtime.shutdown().await;
    }
}

fn desired_resource_context(app: &AppState) -> Option<ResourceContext> {
    if app.mode == AppMode::Normal && !app.search_query.trim().is_empty() {
        return Some(ResourceContext::Search(app.search_query.trim().to_string()));
    }
    if app.is_library_mode() {
        let anime = app.library_selected_anime()?;
        let details_id = app.library_selected_anime_id()?;
        return Some(ResourceContext::Content {
            representative_id: anime.latest_progress.anime_id,
            anime_ids: anime.anime_ids.clone(),
            details_id,
        });
    }
    if app.mode == AppMode::Normal {
        let selected = app.selected_result_index?;
        let item = app.search_results.get(selected)?;
        let mut anime_ids = app
            .selected_group_index
            .and_then(|index| app.franchise_groups.get(index))
            .into_iter()
            .flatten()
            .filter_map(|index| app.search_results.get(*index))
            .filter(|anime| anime.anime_type.eq_ignore_ascii_case("tv"))
            .map(|anime| anime.id)
            .collect::<Vec<_>>();
        if anime_ids.is_empty() {
            anime_ids.push(item.id);
        }
        return Some(ResourceContext::Content {
            representative_id: item.id,
            anime_ids,
            details_id: item.id,
        });
    }
    None
}

struct TerminalRestore {
    alternate_screen: bool,
}

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.alternate_screen {
            let _ = stdout().execute(LeaveAlternateScreen);
        }
    }
}

fn install_terminal_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
        previous(panic_info);
    }));
}

#[tokio::main]
async fn main() -> Result<()> {
    // Picker MUST be initialized before enable_raw_mode
    let picker = ratatui_image::picker::Picker::from_query_stdio()
        .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());

    install_terminal_panic_hook();
    enable_raw_mode()?;
    let mut terminal_restore = TerminalRestore {
        alternate_screen: false,
    };
    stdout().execute(EnterAlternateScreen)?;
    terminal_restore.alternate_screen = true;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut app = AppState::new(picker)?;
    let mut resources = ResourceCoordinator::new(app.api_client.clone());
    let mut playback = PlaybackSupervisor::new();
    let mut persisted_positions = HashMap::new();

    loop {
        resources.drain(&mut app).await;
        resources.sync(&mut app).await;
        if let Some((target, queue)) = resources.take_ready_playback() {
            app.prepare_playback(&target);
            if let Err(error) = playback.play(target, queue).await {
                app.set_error_status(format!("Помилка відтворення: {error}"));
            }
        }
        for event in playback.drain_events() {
            persist_playback_event(&mut app, &mut persisted_positions, event);
        }

        if app.play_episode {
            app.play_episode = false;
            if let Some(target) = selected_play_target(&app) {
                let queue = build_playback_queue(&app, &target);
                app.prepare_playback(&target);
                if let Err(error) = playback.play(target, queue).await {
                    app.set_error_status(format!("Помилка відтворення: {error}"));
                }
            }
        }
        if let Some(request) = app.continue_request.take() {
            resources.request_continue(&mut app, request).await;
        }

        terminal.draw(|f| ui::render(f, &mut app))?;
        app.handle_events()?;

        if app.should_quit {
            break;
        }
    }

    let playback_shutdown_error = playback.shutdown().await.err();
    for event in playback.drain_events() {
        persist_playback_event(&mut app, &mut persisted_positions, event);
    }
    resources.shutdown().await;
    if let Some(error) = playback_shutdown_error {
        return Err(error);
    }
    Ok(())
}

fn persist_playback_event(
    app: &mut AppState,
    persisted_positions: &mut HashMap<PlaybackSessionId, f64>,
    event: PlaybackEvent,
) {
    match event {
        PlaybackEvent::SessionStarted { session_id, target } => {
            app.is_playing = true;
            app.clear_activity();
            app.now_playing = Some(ui::app::NowPlaying {
                anime_title: target.anime_title,
                season: target.season,
                episode: target.episode,
                studio_name: target.studio_name,
                position: target.start_time.unwrap_or(0.0),
                duration: 0.0,
            });
            persisted_positions.entry(session_id).or_insert(0.0);
        }
        PlaybackEvent::ProgressSnapshot(snapshot) => {
            if let Some(now_playing) = app.now_playing.as_mut() {
                if now_playing.season == snapshot.identity.season
                    && now_playing.episode == snapshot.identity.episode
                    && now_playing.studio_name == snapshot.identity.studio_name
                {
                    now_playing.position = snapshot.position;
                    now_playing.duration = snapshot.duration;
                }
            }
            let last = persisted_positions
                .get(&snapshot.session_id)
                .copied()
                .unwrap_or(0.0);
            if snapshot.watched || snapshot.position >= last + 5.0 || snapshot.position < last {
                match app.storage.update_progress(
                    snapshot.identity.anime_id,
                    &snapshot.identity.anime_title,
                    snapshot.identity.season,
                    snapshot.identity.episode,
                    &snapshot.identity.studio_name,
                    snapshot.position,
                    snapshot.duration,
                ) {
                    Ok(history) => {
                        app.history = history;
                        app.rebuild_history_indexes();
                        persisted_positions.insert(snapshot.session_id, snapshot.position);
                    }
                    Err(error) => {
                        app.set_error_status(format!("Не вдалося зберегти прогрес: {error}"));
                    }
                }
            }
        }
        PlaybackEvent::MarkWatched(mark) => {
            if let Some(now_playing) = app.now_playing.as_mut() {
                now_playing.position = mark.position;
                now_playing.duration = mark.duration;
            }
            if let Err(error) = app.storage.set_episode_watched(
                mark.identity.anime_id,
                &mark.identity.anime_title,
                mark.identity.season,
                mark.identity.episode,
                &mark.identity.studio_name,
                true,
            ) {
                app.set_error_status(format!("Не вдалося позначити серію: {error}"));
            } else if let Ok(history) = app.storage.load_history() {
                app.history = history;
                app.rebuild_history_indexes();
            }
            persisted_positions.insert(mark.session_id, mark.position);
        }
        PlaybackEvent::SessionStopped { session_id } => {
            persisted_positions.remove(&session_id);
            app.is_playing = false;
            app.now_playing = None;
            app.clear_activity();
        }
        PlaybackEvent::Error { message, .. } => {
            app.now_playing = None;
            app.clear_activity();
            app.set_error_status(format!("Помилка відтворення: {message}"));
        }
        PlaybackEvent::EndFile { .. } => {}
    }
}

pub async fn get_or_fetch_details(app: &mut AppState, anime_id: u32) -> Option<api::AnimeDetails> {
    if let Some(details) = app.details_cache.get(&anime_id) {
        return Some(details);
    }
    let details = app.api_client.get_anime_details(anime_id).await.ok()?;
    app.details_cache.insert(anime_id, details.clone());
    Some(details)
}

pub async fn get_or_fetch_sources(
    app: &mut AppState,
    anime_id: u32,
) -> Option<EpisodeSourcesResponse> {
    if let Some(sources) = app.sources_cache.get(&anime_id) {
        return Some(sources);
    }
    let sources = app
        .api_client
        .get_episode_sources_for_anime(anime_id)
        .await
        .ok()?;
    app.sources_cache.insert(anime_id, sources.clone());
    Some(sources)
}

pub fn apply_continue_context(
    app: &mut AppState,
    details: &api::AnimeDetails,
    sources: &EpisodeSourcesResponse,
    resolved: &ContinueResolvedEpisode,
) {
    let anime_item = anime_item_from_details(details);
    app.search_results = vec![anime_item];
    app.franchise_groups = vec![vec![0]];
    app.selected_group_index = Some(0);
    app.selected_result_index = Some(0);
    app.result_list_state.select(Some(0));
    app.mode = AppMode::Normal;
    app.focus = FocusPanel::EpisodeList;
    app.current_details = Some(details.clone());
    app.current_sources = Some(sources.clone());
    app.studio_anime_ids = vec![details.id; sources.ashdi.len()];
    app.sidebar_anime_idx = None;
    app.current_poster = None;
    app.poster_fetch_pending = Some(details.id);
    app.selected_season_index = Some(resolved.season_index);
    app.season_list_state.select(Some(resolved.season_index));
    app.selected_dubbing_index = Some(resolved.dubbing_index);
    app.dubbing_list_state.select(Some(resolved.dubbing_index));
    app.selected_episode_index = Some(resolved.episode_index);
    app.episode_list_state.select(Some(resolved.episode_index));
}

pub fn apply_library_continue_context(
    app: &mut AppState,
    progress: &storage::WatchProgress,
    details: &api::AnimeDetails,
    sources: &EpisodeSourcesResponse,
    resolved: &ContinueResolvedEpisode,
) {
    if app.library_items.is_empty() {
        app.open_library();
    }

    app.library_anime_index = app
        .library_items
        .iter()
        .position(|item| item.anime_ids.contains(&progress.anime_id))
        .or_else(|| (!app.library_items.is_empty()).then_some(0));
    app.library_anime_list_state.select(app.library_anime_index);

    app.mode = AppMode::LibraryEpisode;
    app.current_details = Some(details.clone());
    app.current_sources = Some(sources.clone());
    app.studio_anime_ids = vec![details.id; sources.ashdi.len()];
    app.current_poster = None;
    app.poster_fetch_pending = Some(details.id);
    app.selected_season_index = Some(resolved.season_index);
    app.season_list_state.select(Some(resolved.season_index));
    app.selected_dubbing_index = Some(resolved.dubbing_index);
    app.dubbing_list_state.select(Some(resolved.dubbing_index));
    app.selected_episode_index = Some(resolved.episode_index);
    app.episode_list_state.select(Some(resolved.episode_index));
}

fn anime_item_from_details(details: &api::AnimeDetails) -> api::AnimeItem {
    api::AnimeItem {
        id: details.id,
        anilist_id: details.anilist_id,
        slug: details.slug.clone(),
        title_ukrainian: details.title_ukrainian.clone(),
        title_original: details.title_original.clone(),
        title_english: details.title_english.clone(),
        status: details.status.clone(),
        anime_type: details.anime_type.clone(),
        year: details.year,
        has_ukrainian_dub: details.has_ukrainian_dub,
    }
}

fn apply_search_results(app: &mut AppState, results: Vec<api::AnimeItem>) {
    app.search_results = results;
    app.franchise_groups = api::group_into_franchises(&app.search_results);
    app.search_query.clear();
    app.search_cursor = 0;

    app.focus = FocusPanel::SearchList;
    app.current_sources = None;
    app.current_details = None;
    app.current_poster = None;
    app.studio_anime_ids.clear();
    app.sidebar_anime_idx = None;
    app.selected_season_index = None;
    app.season_list_state.select(None);
    app.selected_dubbing_index = None;
    app.dubbing_list_state.select(None);
    app.selected_episode_index = None;
    app.episode_list_state.select(None);

    if !app.franchise_groups.is_empty() {
        app.result_list_state.select(Some(0));
        app.selected_group_index = Some(0);
        let rep = api::representative_idx(&app.search_results, &app.franchise_groups[0]);
        app.selected_result_index = Some(rep);
        app.set_activity("Завантаження вибраного аніме…");
    } else {
        app.clear_activity();
        app.result_list_state.select(None);
        app.selected_group_index = None;
        app.selected_result_index = None;
        app.set_info_status("Нічого не знайдено");
    }
}
