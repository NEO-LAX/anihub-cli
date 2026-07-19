//! Foreground resource orchestration for the active TUI view.
//!
//! This module owns generation changes, cancellation, cached search fallback,
//! poster scheduling, continue preparation, and the independent library
//! refresh lane. ResourceWorker semantics stay isolated from the runtime loop.

use crate::api::resource::LoadError;
use crate::api::{
    EpisodeSourcesKey, EpisodeSourcesResponse, RequestId, ResourceEvent, ResourceKey,
    ResourceValue, ResourceWorker, ResourceWorkerRuntime, ViewGeneration,
};
use crate::library_refresh;
use crate::playback::*;
use crate::poster_cache;
use crate::storage;
use crate::ui::{AppMode, AppState, FocusPanel};
use crate::{
    anime_item_from_details, api, apply_continue_context, apply_library_continue_context,
    apply_search_results, build_active_playback_timeline, rebuild_franchise_projection,
    should_add_details_to_search, ui,
};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SourceLoadScope {
    DetailsOnly,
    Preview,
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ResourceContext {
    Search {
        query: String,
        extended: bool,
    },
    Content {
        source_keys: Vec<EpisodeSourcesKey>,
        details_key: EpisodeSourcesKey,
        source_scope: SourceLoadScope,
    },
}

struct PendingContinue {
    progress: storage::WatchProgress,
    in_library: bool,
}

pub(super) struct ResourceCoordinator {
    runtime: ResourceWorkerRuntime,
    generation: ViewGeneration,
    context: Option<ResourceContext>,
    poster_requests: HashMap<RequestId, u32>,
    posters_in_flight: HashSet<u32>,
    pending_continue: Option<PendingContinue>,
    ready_playback: Option<PlaybackTimeline>,
    cached_search_used: Option<(String, bool)>,
    poster_candidate: Option<(u32, Instant)>,
    force_reload: bool,
    library_refresh: library_refresh::LibraryRefreshCoordinator,
}

impl ResourceCoordinator {
    pub(super) fn new(client: api::ApiClient, poster_cache: poster_cache::PosterCache) -> Self {
        Self {
            runtime: ResourceWorker::spawn_with_poster_cache(client, poster_cache),
            generation: ViewGeneration::default(),
            context: None,
            poster_requests: HashMap::new(),
            posters_in_flight: HashSet::new(),
            pending_continue: None,
            ready_playback: None,
            cached_search_used: None,
            poster_candidate: None,
            force_reload: false,
            library_refresh: library_refresh::LibraryRefreshCoordinator::default(),
        }
    }

    pub(super) async fn sync(&mut self, app: &mut AppState) {
        let desired = self.desired_context(app);
        if desired != self.context {
            let previous = self.generation;
            self.generation = ViewGeneration::new(previous.get().saturating_add(1));
            if previous.get() != 0 {
                let _ = self.runtime.handle.cancel_generation(previous).await;
            }
            self.context = desired.clone();
            self.poster_requests.clear();
            self.posters_in_flight.clear();
            self.cached_search_used = None;

            if let Some(context) = desired {
                self.start_context(app, context).await;
            }
        }

        // Foreground details/sources are submitted before background library
        // work so refreshing a large collection cannot delay navigation.
        self.library_refresh
            .start_if_requested(app, &self.runtime.handle)
            .await;

        self.schedule_poster(app).await;
        self.finish_continue_if_ready(app);
    }

    pub(super) fn retry_current_context(&mut self) {
        // `sync` will create a fresh generation and resubmit the same desired
        // context. Completed worker entries may still satisfy it immediately.
        self.context = None;
        self.cached_search_used = None;
        self.force_reload = true;
    }

    async fn load_resource(
        &self,
        key: ResourceKey,
        force_reload: bool,
    ) -> Result<RequestId, api::resource::ResourceCommandError> {
        if force_reload {
            self.runtime.handle.reload(self.generation, key).await
        } else {
            self.runtime.handle.load(self.generation, key).await
        }
    }

    fn desired_context(&self, app: &AppState) -> Option<ResourceContext> {
        if let Some(pending) = &self.pending_continue {
            return Some(ResourceContext::Content {
                source_keys: vec![EpisodeSourcesKey::new(
                    pending.progress.anime_id,
                    pending.progress.season,
                )],
                details_key: EpisodeSourcesKey::new(
                    pending.progress.anime_id,
                    pending.progress.season,
                ),
                source_scope: SourceLoadScope::Full,
            });
        }
        desired_resource_context(app)
    }

    pub(super) async fn request_continue(
        &mut self,
        app: &mut AppState,
        request: crate::ui::ContinueRequest,
    ) {
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
        let source_key = EpisodeSourcesKey::new(pending.progress.anime_id, pending.progress.season);
        let Some(sources) = app.sources_cache.get(&source_key) else {
            return;
        };
        if sources.is_moonanime_only() {
            if let Some(episode) = sources
                .moonanime
                .iter()
                .find(|studio| studio.studio_name == pending.progress.studio_name)
                .and_then(|studio| {
                    studio
                        .episodes
                        .iter()
                        .find(|episode| episode.episode_number == pending.progress.episode)
                })
                .or_else(|| {
                    sources
                        .moonanime
                        .iter()
                        .flat_map(|studio| studio.episodes.iter())
                        .find(|episode| episode.episode_number == pending.progress.episode)
                })
                .or_else(|| {
                    sources
                        .moonanime
                        .iter()
                        .find_map(|studio| studio.episodes.first())
                })
            {
                app.prompt_moonanime_browser(
                    format!(
                        "{} — серія {} [MoonAnime]",
                        pending.progress.anime_title, episode.episode_number
                    ),
                    episode.iframe_url.clone(),
                );
            } else {
                app.set_info_status("MoonAnime не повернув посилання на епізод");
            }
            self.pending_continue = None;
            app.clear_activity();
            return;
        }
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
            referrer: "https://ashdi.vip/".to_string(),
        };
        self.ready_playback = Some(build_active_playback_timeline(app, &target));
        self.pending_continue = None;
        app.clear_activity();
    }

    pub(super) fn take_ready_playback(&mut self) -> Option<PlaybackTimeline> {
        self.ready_playback.take()
    }

    async fn start_context(&mut self, app: &mut AppState, context: ResourceContext) {
        let force_reload = std::mem::take(&mut self.force_reload);
        match context {
            ResourceContext::Search { query, extended } => {
                app.set_activity("Пошук аніме…");
                if let Some(cached) = app.metadata_cache.search(&query, extended) {
                    apply_search_results(app, cached.items, cached.anilist_media, false);
                    self.cached_search_used = Some((query.clone(), extended));
                    app.set_activity("Кешовані результати · перевіряємо мережу…");
                }
                if self
                    .load_resource(ResourceKey::search(query, extended), force_reload)
                    .await
                    .is_err()
                {
                    app.clear_activity();
                    app.set_error_status("Сервіс завантаження недоступний");
                }
            }
            ResourceContext::Content {
                source_keys,
                details_key,
                source_scope,
            } => {
                app.set_activity(match source_scope {
                    SourceLoadScope::DetailsOnly => "Завантаження метаданих…",
                    SourceLoadScope::Preview => "Завантаження першого випуску…",
                    SourceLoadScope::Full => "Завантаження випусків…",
                });
                let cached_details = app.details_cache.get(&details_key.anime_id);
                if let Some(details) = cached_details.clone() {
                    app.current_details = Some(details);
                }
                let details_need_refresh = cached_details.is_none()
                    || !app.metadata_cache.details_are_fresh(details_key.anime_id);
                if details_need_refresh {
                    let request = self
                        .load_resource(ResourceKey::details(details_key.anime_id), force_reload)
                        .await;
                    if request.is_err() {
                        app.clear_activity();
                        app.set_error_status("Сервіс завантаження недоступний");
                        return;
                    }
                }

                if matches!(source_scope, SourceLoadScope::DetailsOnly) {
                    if cached_details.is_some() {
                        app.clear_activity();
                    }
                    return;
                }

                if let Some(sources) = app.sources_cache.get(&details_key) {
                    app.current_sources = Some(sources);
                    app.current_sources_key = Some(details_key);
                    app.studio_anime_ids = vec![
                        details_key.anime_id;
                        app.current_sources
                            .as_ref()
                            .map_or(0, |sources| sources.ashdi.len())
                    ];
                    app.clear_activity();
                }

                // The selected release is requested first; the rest are
                // background prefetches. Each response keeps its raw AniHub
                // release identity instead of being normalized and merged.
                if !app.sources_cache.contains_key(&details_key) {
                    let _ = self
                        .load_resource(
                            ResourceKey::sources(details_key.anime_id, details_key.season),
                            force_reload,
                        )
                        .await;
                }
                for source_key in &source_keys {
                    if *source_key != details_key && !app.sources_cache.contains_key(source_key) {
                        let _ = self
                            .load_resource(
                                ResourceKey::sources(source_key.anime_id, source_key.season),
                                force_reload,
                            )
                            .await;
                    }
                }

                if matches!(source_scope, SourceLoadScope::Full) {
                    let unavailable = app
                        .selected_franchise_catalog()
                        .into_iter()
                        .flat_map(|catalog| catalog.unresolved_anilist_ids.iter().copied())
                        .collect::<Vec<_>>();
                    for anilist_id in unavailable {
                        let _ = self
                            .runtime
                            .handle
                            .load(self.generation, ResourceKey::anihub_by_anilist(anilist_id))
                            .await;
                    }
                }
            }
        }
    }

    async fn schedule_poster(&mut self, app: &mut AppState) {
        if !app.settings.show_posters {
            app.poster_fetch_pending = None;
            self.poster_candidate = None;
            return;
        }
        let Some(anime_id) = app.poster_fetch_pending else {
            self.poster_candidate = None;
            return;
        };
        const POSTER_DEBOUNCE: Duration = Duration::from_millis(120);
        match self.poster_candidate {
            Some((candidate, since))
                if candidate == anime_id && since.elapsed() >= POSTER_DEBOUNCE => {}
            Some((candidate, _)) if candidate == anime_id => return,
            _ => {
                self.poster_candidate = Some((anime_id, Instant::now()));
                return;
            }
        }
        app.poster_fetch_pending = None;
        self.poster_candidate = None;
        if let Some(image) = app.poster_cache.get(&anime_id) {
            app.install_poster(anime_id, image);
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
                app.search
                    .results
                    .iter()
                    .find(|item| item.id == anime_id)
                    .and_then(|item| item.poster_url.clone())
            })
            .or_else(|| app.poster_url_for_subject(anime_id))
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

    pub(super) async fn drain(&mut self, app: &mut AppState) {
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
            } => (request_id, generation, key, Err(error)),
        };
        if self.library_refresh.generation() == Some(generation) {
            self.library_refresh
                .apply_event(app, &self.runtime.handle, request_id, key, result)
                .await;
            return;
        }
        if generation != self.generation {
            return;
        }

        match (key, result) {
            (ResourceKey::Search { query, extended }, Ok(ResourceValue::Search(results))) => {
                let _ = app.metadata_cache.put_search(
                    &query,
                    extended,
                    results.items.clone(),
                    results.anilist_media.clone(),
                );
                self.cached_search_used = None;
                apply_search_results(
                    app,
                    api::deduplicate_anime(results.items),
                    results.anilist_media,
                    true,
                );
            }
            (ResourceKey::Search { query, extended }, Err(error)) => {
                if self.cached_search_used.as_ref() == Some(&(query, extended)) {
                    self.cached_search_used = None;
                    app.search.query.clear();
                    app.clear_activity();
                    app.set_info_status(
                        "Показано кешовані результати · мережеве оновлення не вдалося",
                    );
                } else {
                    app.clear_activity();
                    set_resource_error(app, "Не вдалося виконати пошук", &error);
                }
            }
            (ResourceKey::Details(anime_id), Ok(ResourceValue::Details(details))) => {
                let _ = app.metadata_cache.put_details(details.clone());
                app.details_cache.insert(anime_id, details.clone());
                if should_add_details_to_search(
                    app.mode,
                    app.search.results.iter().any(|item| item.id == anime_id),
                    details.anilist_id.is_some(),
                ) {
                    app.search.results.push(anime_item_from_details(&details));
                    rebuild_franchise_projection(app);
                    if !app.search.last_query.is_empty() {
                        let _ = app.metadata_cache.put_search(
                            &app.search.last_query,
                            app.settings.search_mode.is_extended(),
                            app.search.results.clone(),
                            app.search.anilist_media.clone(),
                        );
                    }
                }
                for item in app
                    .library
                    .all_items
                    .iter_mut()
                    .chain(app.library.items.iter_mut())
                    .filter(|item| item.anime_ids.contains(&anime_id))
                {
                    if item.anime_title.starts_with("Закладка #") {
                        item.anime_title.clone_from(&details.title_ukrainian);
                        item.latest_progress.anime_title = details.title_ukrainian.clone();
                    }
                }
                let is_current = matches!(
                    self.desired_context(app),
                    Some(ResourceContext::Content { details_key, .. })
                        if details_key.anime_id == anime_id
                );
                if is_current {
                    app.current_details = Some(details);
                    if app.settings.show_posters
                        && app.current_poster.is_none()
                        && app.sidebar_subject() == Some(anime_id)
                    {
                        app.poster_fetch_pending = Some(anime_id);
                    }
                    if matches!(
                        self.desired_context(app),
                        Some(ResourceContext::Content {
                            source_scope: SourceLoadScope::DetailsOnly,
                            ..
                        })
                    ) {
                        app.clear_activity();
                    }
                }
            }
            (ResourceKey::Details(anime_id), Err(error)) => {
                app.clear_activity();
                if app.details_cache.contains_key(&anime_id) {
                    app.set_info_status("Показано кешовані метадані · оновлення не вдалося");
                } else {
                    set_resource_error(app, "Не вдалося завантажити метадані", &error);
                }
            }
            (ResourceKey::Sources(source_key), Ok(ResourceValue::Sources(sources))) => {
                let sources = cap_sources_to_available_episodes(
                    sources,
                    available_episode_limit(app, source_key.anime_id),
                );
                app.sources_cache.insert(source_key, sources.clone());
                let is_current = matches!(
                    self.desired_context(app),
                    Some(ResourceContext::Content { details_key, .. }) if details_key == source_key
                );
                if is_current {
                    app.studio_anime_ids = vec![source_key.anime_id; sources.ashdi.len()];
                    app.current_sources = Some(sources);
                    app.current_sources_key = Some(source_key);
                    app.clear_activity();
                }
            }
            (ResourceKey::Sources(source_key), Err(error)) => {
                let is_current = matches!(
                    self.desired_context(app),
                    Some(ResourceContext::Content { details_key, .. }) if details_key == source_key
                );
                if is_current {
                    app.clear_activity();
                    set_resource_error(app, "Не вдалося завантажити випуск", &error);
                }
            }
            (ResourceKey::AniHubByAniList(_), Ok(ResourceValue::AniHubId(Some(anime_id))))
                if !app.search.results.iter().any(|item| item.id == anime_id) =>
            {
                let _ = self
                    .runtime
                    .handle
                    .load(self.generation, ResourceKey::details(anime_id))
                    .await;
            }
            (ResourceKey::AniHubByAniList(_), Ok(ResourceValue::AniHubId(Some(_)))) => {}
            (ResourceKey::AniHubByAniList(_), Ok(ResourceValue::AniHubId(None))) => {}
            (ResourceKey::AniHubByAniList(_), Err(_)) => {}
            (ResourceKey::Poster(_), Ok(ResourceValue::Poster(image))) => {
                if let Some(anime_id) = self.poster_requests.remove(&request_id) {
                    self.posters_in_flight.remove(&anime_id);
                    let image = std::sync::Arc::new(image);
                    app.install_poster(anime_id, image);
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

    pub(super) async fn shutdown(self) {
        let _ = self.runtime.shutdown().await;
    }
}

pub(super) fn resource_error_hint(error: &LoadError) -> String {
    match error {
        LoadError::Network(_) => "Немає з’єднання з AniHub".to_string(),
        LoadError::Http {
            status: 429,
            retry_after,
            ..
        } => retry_after.map_or_else(
            || "AniHub тимчасово обмежив кількість запитів".to_string(),
            |delay| {
                format!(
                    "AniHub обмежив запити · повторіть приблизно через {} с",
                    delay.as_secs().max(1)
                )
            },
        ),
        LoadError::Http { status, .. } if (500..=599).contains(status) => {
            format!("AniHub тимчасово недоступний · HTTP {status}")
        }
        LoadError::Http { status, .. } => format!("AniHub повернув HTTP {status}"),
        LoadError::NotFound => "Дані більше не доступні на AniHub".to_string(),
        LoadError::NoSources => "Немає озвучок або серій для цього випуску на AniHub".to_string(),
        LoadError::Parse(_) | LoadError::Decode(_) => "AniHub повернув некоректні дані".to_string(),
        LoadError::Unsupported(_) => "Цей ресурс не підтримується".to_string(),
        LoadError::Shutdown => "Сервіс завантаження завершує роботу".to_string(),
    }
}

fn set_resource_error(app: &mut AppState, context: &str, error: &LoadError) {
    let message = format!("{context}\n{}", resource_error_hint(error));
    if error.is_transient() {
        app.set_retryable_error_status(message);
    } else {
        app.set_error_status(message);
    }
}

fn available_episode_limit(app: &AppState, anime_id: u32) -> Option<u32> {
    app.search
        .franchise_catalogs
        .iter()
        .flat_map(|catalog| catalog.releases.iter())
        .find(|release| release.anihub_id == Some(anime_id))
        .and_then(|release| {
            release.available_episodes.filter(|available| {
                release
                    .episodes_count
                    .is_some_and(|total| *available < total)
            })
        })
}

/// AniHub may publish placeholder rows for episodes that have not aired yet.
/// The cap is release-local: split cours can use raw numbers such as 12..24,
/// so comparing those raw episode numbers with the count would drop valid rows.
pub(super) fn cap_sources_to_available_episodes(
    mut sources: EpisodeSourcesResponse,
    available: Option<u32>,
) -> EpisodeSourcesResponse {
    let Some(limit) = available.map(|count| count as usize) else {
        return sources;
    };

    for studio in &mut sources.ashdi {
        studio.episodes.truncate(limit);
        studio.episodes_count = studio.episodes.len() as u32;
    }
    sources
}

fn desired_resource_context(app: &AppState) -> Option<ResourceContext> {
    if app.mode == AppMode::Normal && !app.search.query.trim().is_empty() {
        return Some(ResourceContext::Search {
            query: app.search.query.trim().to_string(),
            extended: app.settings.search_mode.is_extended(),
        });
    }
    if app.is_library_mode() {
        let anime = app.library_selected_anime()?;
        let details_id = app.library_selected_anime_id()?;
        let details_season = app
            .selected_season_num()
            .or_else(|| {
                anime
                    .seasons
                    .iter()
                    .find(|season| season.anime_id == details_id)
                    .map(|season| season.season)
            })
            .unwrap_or(anime.latest_progress.season);
        let source_scope = if app.mode == AppMode::Library {
            SourceLoadScope::DetailsOnly
        } else {
            SourceLoadScope::Full
        };
        let mut source_keys = anime
            .seasons
            .iter()
            .map(|season| EpisodeSourcesKey::new(season.anime_id, season.season))
            .collect::<Vec<_>>();
        for anime_id in &anime.anime_ids {
            if !source_keys.iter().any(|key| key.anime_id == *anime_id) {
                source_keys.push(EpisodeSourcesKey::new(*anime_id, 1));
            }
        }
        source_keys.sort_by_key(|key| (key.season, key.anime_id));
        source_keys.dedup();
        let details_key = EpisodeSourcesKey::new(details_id, details_season);
        if !source_keys.contains(&details_key) {
            source_keys.push(details_key);
        }
        let source_keys = source_keys_for_scope(source_keys, &source_scope);
        return Some(ResourceContext::Content {
            source_keys,
            details_key,
            source_scope,
        });
    }
    if app.mode == AppMode::Normal {
        let source_scope = if app.focus == FocusPanel::SearchList {
            SourceLoadScope::Preview
        } else {
            SourceLoadScope::Full
        };

        if let Some(catalog) = app.selected_franchise_catalog() {
            let mut source_keys = catalog
                .releases
                .iter()
                .filter_map(|release| {
                    release.anihub_id.map(|anime_id| {
                        EpisodeSourcesKey::new(anime_id, release.conceptual_season.unwrap_or(1))
                    })
                })
                .collect::<Vec<_>>();
            source_keys.sort_by_key(|key| (key.season, key.anime_id));
            source_keys.dedup();
            let canonical_key = catalog
                .releases
                .iter()
                .find_map(|release| {
                    release.anihub_id.map(|anime_id| {
                        EpisodeSourcesKey::new(anime_id, release.conceptual_season.unwrap_or(1))
                    })
                })
                .or_else(|| {
                    app.search
                        .selected_result_index
                        .and_then(|index| app.search.results.get(index))
                        .map(|item| EpisodeSourcesKey::new(item.id, 1))
                })?;
            let details_key = if app.focus == FocusPanel::SearchList {
                canonical_key
            } else {
                app.selected_release_source_key().unwrap_or(canonical_key)
            };
            let source_keys = if matches!(source_scope, SourceLoadScope::Preview) {
                vec![canonical_key]
            } else {
                source_keys
            };
            return Some(ResourceContext::Content {
                source_keys,
                details_key,
                source_scope,
            });
        }

        let selected = app.search.selected_result_index?;
        let item = app.search.results.get(selected)?;
        let details_key = EpisodeSourcesKey::new(item.id, 1);
        let source_keys = source_keys_for_scope(vec![details_key], &source_scope);
        return Some(ResourceContext::Content {
            source_keys,
            details_key,
            source_scope,
        });
    }
    None
}

pub(super) fn source_keys_for_scope(
    mut source_keys: Vec<EpisodeSourcesKey>,
    scope: &SourceLoadScope,
) -> Vec<EpisodeSourcesKey> {
    match scope {
        SourceLoadScope::DetailsOnly => source_keys.clear(),
        SourceLoadScope::Preview => source_keys.truncate(1),
        SourceLoadScope::Full => {}
    }
    source_keys
}
