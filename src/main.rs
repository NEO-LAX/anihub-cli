mod api;
mod cache;
mod discord;
mod platform;
mod playback;
mod player;
mod poster_cache;
mod settings;
mod storage;
mod ui;

use crate::discord::{DiscordPresence, PresenceActivity};
use crate::playback::*;
use anyhow::{Result, bail};
use api::resource::LoadError;
use api::{
    EpisodeSourcesKey, EpisodeSourcesResponse, RequestId, ResourceEvent, ResourceKey,
    ResourceValue, ResourceWorker, ResourceWorkerRuntime, ViewGeneration,
};
use crossterm::{
    ExecutableCommand,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::collections::{HashMap, HashSet};
use std::env;
use std::io::stdout;
use std::time::{Duration, Instant};
use ui::{AppMode, AppState, FocusPanel};

#[derive(Clone, Debug, PartialEq, Eq)]
enum SourceLoadScope {
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

struct ResourceCoordinator {
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
}

impl ResourceCoordinator {
    fn new(client: api::ApiClient, poster_cache: poster_cache::PosterCache) -> Self {
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
            self.poster_requests.clear();
            self.posters_in_flight.clear();
            self.cached_search_used = None;

            if let Some(context) = desired {
                self.start_context(app, context).await;
            }
        }

        self.schedule_poster(app).await;
        self.finish_continue_if_ready(app);
    }

    fn retry_current_context(&mut self) {
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

    fn take_ready_playback(&mut self) -> Option<PlaybackTimeline> {
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
                app.search_results
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
            } => (request_id, generation, key, Err(error)),
        };
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
                    app.search_query.clear();
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
                    app.search_results.iter().any(|item| item.id == anime_id),
                    details.anilist_id.is_some(),
                ) {
                    app.search_results.push(anime_item_from_details(&details));
                    rebuild_franchise_projection(app);
                    if !app.last_search_query.is_empty() {
                        let _ = app.metadata_cache.put_search(
                            &app.last_search_query,
                            app.settings.search_mode.is_extended(),
                            app.search_results.clone(),
                            app.anilist_media.clone(),
                        );
                    }
                }
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
                if !app.search_results.iter().any(|item| item.id == anime_id) =>
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

    async fn shutdown(self) {
        let _ = self.runtime.shutdown().await;
    }
}

fn resource_error_hint(error: &LoadError) -> String {
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
    app.franchise_catalogs
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
fn cap_sources_to_available_episodes(
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
    if app.mode == AppMode::Normal && !app.search_query.trim().is_empty() {
        return Some(ResourceContext::Search {
            query: app.search_query.trim().to_string(),
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
                    app.selected_result_index
                        .and_then(|index| app.search_results.get(index))
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

        let selected = app.selected_result_index?;
        let item = app.search_results.get(selected)?;
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

fn source_keys_for_scope(
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

fn handle_cli_mode() -> Result<bool> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => Ok(false),
        [argument] if argument == "--version" || argument == "-V" => {
            println!("anihub-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(true)
        }
        [argument] if argument == "--migrate-data" => {
            let storage = storage::StorageManager::new()?;
            let history = storage.load_history()?;
            let settings_store = settings::SettingsStore::new()?;
            let settings = settings_store.load()?;
            settings_store.save(&settings)?;

            println!("Local data was validated and migrated:");
            println!("  history: {}", storage.history_path().display());
            println!("  settings: {}", settings_store.settings_path().display());
            println!(
                "  progress: {} · library: {}",
                history.progress.len(),
                history.library.len()
            );
            Ok(true)
        }
        [argument] if argument == "--help" || argument == "-h" => {
            println!(
                "anihub-cli {}\n\n  --version       show the version\n  --migrate-data  validate and migrate local data",
                env!("CARGO_PKG_VERSION")
            );
            Ok(true)
        }
        _ => bail!("невідомі аргументи: {}", arguments.join(" ")),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    if handle_cli_mode()? {
        return Ok(());
    }

    // Picker MUST be initialized before enable_raw_mode
    let (picker, image_protocol) = match ratatui_image::picker::Picker::from_query_stdio() {
        Ok(picker) => {
            let protocol = match picker.protocol_type() {
                ratatui_image::picker::ProtocolType::Halfblocks => "Halfblocks",
                ratatui_image::picker::ProtocolType::Sixel => "Sixel",
                ratatui_image::picker::ProtocolType::Kitty => "Kitty",
                ratatui_image::picker::ProtocolType::Iterm2 => "iTerm2",
            };
            (picker, protocol)
        }
        Err(_) => (ratatui_image::picker::Picker::halfblocks(), "Halfblocks"),
    };

    install_terminal_panic_hook();
    enable_raw_mode()?;
    let mut terminal_restore = TerminalRestore {
        alternate_screen: false,
    };
    stdout().execute(EnterAlternateScreen)?;
    terminal_restore.alternate_screen = true;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut app = AppState::new(picker, image_protocol)?;
    let mut resources =
        ResourceCoordinator::new(app.api_client.clone(), app.poster_disk_cache.clone());
    let mut playback = PlaybackSupervisor::new();
    let discord_presence = DiscordPresence::new(app.settings.discord_presence);
    sync_discord_presence(&app, &discord_presence);
    let mut persisted_positions = HashMap::new();
    let mut update_check: Option<tokio::task::JoinHandle<Result<settings::UpdateCheck>>> = None;

    loop {
        // Apply the latest UI selection before accepting worker completions.
        // Otherwise a late S1 response can be installed after the cursor has
        // already moved to S2/S3 but before the generation is canceled.
        if app.take_retry_request() {
            resources.retry_current_context();
        }
        resources.sync(&mut app).await;
        resources.drain(&mut app).await;
        if app.take_discord_config_changed() {
            discord_presence.configure(app.settings.discord_presence);
            sync_discord_presence(&app, &discord_presence);
        }
        if app.take_update_check_request() && update_check.is_none() {
            update_check = Some(tokio::spawn(settings::check_for_update(env!(
                "CARGO_PKG_VERSION"
            ))));
        }
        if update_check.as_ref().is_some_and(|task| task.is_finished()) {
            let result = update_check
                .take()
                .expect("finished update task exists")
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result);
            app.finish_update_check(result);
        }

        if let Some(mut timeline) = resources.take_ready_playback() {
            if let Err(error) = apply_playback_settings(&app, &mut timeline) {
                app.set_error_status(format!("Помилка відтворення: {error}"));
            } else {
                app.prepare_playback(timeline.current());
                if let Err(error) = playback.play(timeline, app.settings.autoplay_next).await {
                    app.set_error_status(format!("Помилка відтворення: {error}"));
                }
            }
        }
        let playback_events = playback.drain_events();
        let mut presence_changed = false;
        for event in playback_events {
            presence_changed |= matches!(
                &event,
                PlaybackEvent::SessionStarted { .. }
                    | PlaybackEvent::SessionStopped { .. }
                    | PlaybackEvent::Error { .. }
            );
            persist_playback_event(&mut app, &mut persisted_positions, event);
        }
        if presence_changed {
            sync_discord_presence(&app, &discord_presence);
        }

        if app.play_episode {
            app.play_episode = false;
            if let Some(target) = selected_play_target(&app) {
                let mut timeline = build_active_playback_timeline(&app, &target);
                if let Err(error) = apply_playback_settings(&app, &mut timeline) {
                    app.set_error_status(format!("Помилка відтворення: {error}"));
                } else {
                    app.prepare_playback(timeline.current());
                    if let Err(error) = playback.play(timeline, app.settings.autoplay_next).await {
                        app.set_error_status(format!("Помилка відтворення: {error}"));
                    }
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
    discord_presence.clear();
    resources.shutdown().await;
    if let Some(task) = update_check {
        task.abort();
    }
    discord_presence.shutdown();
    if let Some(error) = playback_shutdown_error {
        return Err(error);
    }
    Ok(())
}

fn sync_discord_presence(app: &AppState, discord: &DiscordPresence) {
    if !app.settings.discord_presence {
        discord.clear();
        return;
    }
    let Some(now) = &app.now_playing else {
        discord.update(PresenceActivity::idle());
        return;
    };
    discord.update(PresenceActivity::watching(
        &now.anime_title,
        now.season,
        now.episode,
        &now.studio_name,
        app.details_cache
            .get(&now.anime_id)
            .and_then(|details| details.poster_url.clone())
            .or_else(|| app.poster_url_for_subject(now.anime_id)),
    ));
}

fn apply_playback_settings(app: &AppState, timeline: &mut PlaybackTimeline) -> Result<()> {
    player::configure_mpv(&app.settings.mpv_path, &app.settings.mpv_extra_args)?;
    if !app.settings.resume_from_timestamp {
        timeline.clear_resume_positions();
    }
    Ok(())
}

fn build_active_playback_timeline(app: &AppState, target: &PlayTarget) -> PlaybackTimeline {
    let Some(catalog) = app.selected_franchise_catalog() else {
        return build_playback_timeline(app, target);
    };

    let loaded = catalog
        .releases
        .iter()
        .filter(|release| release.classification != api::ReleaseClassification::Extra)
        .filter_map(|release| {
            let anime_id = release.anihub_id?;
            let source_key =
                EpisodeSourcesKey::new(anime_id, release.conceptual_season.unwrap_or(1));
            let sources = app.sources_cache.get(&source_key)?;
            let item = app.search_results.iter().find(|item| item.id == anime_id);
            let title = item
                .map(|item| item.title_ukrainian.clone())
                .unwrap_or_else(|| release.title.clone());
            let player_title = format!("{} ({})", title, release.year.unwrap_or(0));
            Some((anime_id, title, player_title, sources))
        })
        .collect::<Vec<_>>();
    let timeline = loaded
        .iter()
        .map(|(anime_id, title, player_title, sources)| PlaybackRelease {
            anime_id: *anime_id,
            anime_title: title,
            player_title,
            sources,
        })
        .collect::<Vec<_>>();
    build_release_playback_timeline(target, &timeline)
}

fn persist_playback_event(
    app: &mut AppState,
    persisted_positions: &mut HashMap<PlaybackSessionId, f64>,
    event: PlaybackEvent,
) {
    match event {
        PlaybackEvent::SessionStarted { session_id, target } => {
            app.clear_activity();
            app.now_playing = Some(ui::app::NowPlaying {
                anime_id: target.anime_id,
                anime_title: target.anime_title,
                season: target.season,
                episode: target.episode,
                studio_name: target.studio_name,
                position: target.start_time.unwrap_or(0.0),
                duration: 0.0,
                paused: false,
            });
            // One mpv process can now visit several logical episodes. Reset
            // the persistence debounce for every selected timeline entry.
            persisted_positions.insert(session_id, target.start_time.unwrap_or(0.0));
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
                    app.settings.watched_threshold_percent,
                ) {
                    Ok(history) => {
                        app.history = history;
                        app.rebuild_history_indexes();
                        // The active search catalog knows the whole AniList
                        // franchise. Persist it on the first progress write so
                        // Library can still show sibling seasons after restart.
                        app.hydrate_library_catalog_metadata();
                        persisted_positions.insert(snapshot.session_id, snapshot.position);
                    }
                    Err(error) => {
                        app.set_error_status(format!("Не вдалося зберегти прогрес: {error}"));
                    }
                }
            }
        }
        PlaybackEvent::PauseChanged {
            identity,
            paused,
            position,
            ..
        } => {
            if let Some(now_playing) = app.now_playing.as_mut()
                && now_playing.anime_id == identity.anime_id
                && now_playing.season == identity.season
                && now_playing.episode == identity.episode
                && now_playing.studio_name == identity.studio_name
            {
                if let Some(position) = position {
                    now_playing.position = position;
                }
                now_playing.paused = paused;
            }
        }
        PlaybackEvent::MarkWatched(mark) => {
            if let Some(now_playing) = app.now_playing.as_mut() {
                now_playing.position = mark.position;
                now_playing.duration = mark.duration;
            }
            match app.storage.set_episode_watched(
                mark.identity.anime_id,
                &mark.identity.anime_title,
                mark.identity.season,
                mark.identity.episode,
                &mark.identity.studio_name,
                true,
            ) {
                Ok(history) => {
                    app.history = history;
                    app.rebuild_history_indexes();
                    app.hydrate_library_catalog_metadata();
                }
                Err(error) => {
                    app.set_error_status(format!("Не вдалося позначити серію: {error}"));
                }
            }
            persisted_positions.insert(mark.session_id, mark.position);
        }
        PlaybackEvent::SessionStopped { session_id } => {
            persisted_positions.remove(&session_id);
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

pub fn apply_continue_context(
    app: &mut AppState,
    details: &api::AnimeDetails,
    sources: &EpisodeSourcesResponse,
    resolved: &ContinueResolvedEpisode,
) {
    let anime_item = anime_item_from_details(details);
    app.search_results = vec![anime_item];
    app.anilist_media.clear();
    app.franchise_catalogs = api::build_franchise_catalogs(&app.search_results, &[]);
    app.franchise_groups = vec![vec![0]];
    app.selected_group_index = Some(0);
    app.selected_result_index = Some(0);
    app.selected_release_index = Some(0);
    app.result_list_state.select(Some(0));
    app.mode = AppMode::Normal;
    app.focus = FocusPanel::EpisodeList;
    app.current_details = Some(details.clone());
    app.current_sources = Some(sources.clone());
    app.current_sources_key = Some(EpisodeSourcesKey::new(details.id, resolved.season));
    app.studio_anime_ids = vec![details.id; sources.ashdi.len()];
    app.sidebar_anime_idx = None;
    app.sidebar_subject_id = Some(details.id);
    app.current_poster = None;
    app.poster_fetch_pending = app.settings.show_posters.then_some(details.id);
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
    app.current_sources_key = Some(EpisodeSourcesKey::new(details.id, resolved.season));
    app.studio_anime_ids = vec![details.id; sources.ashdi.len()];
    app.current_poster = None;
    app.sidebar_subject_id = Some(details.id);
    app.poster_fetch_pending = app.settings.show_posters.then_some(details.id);
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
        poster_url: details.poster_url.clone(),
        episodes_count: details.episodes_count,
        description: details.description.clone(),
        rating: details.rating,
        genres: details.genres.clone(),
        dubbing_studios: details.dubbing_studios.clone(),
    }
}

fn should_add_details_to_search(
    mode: AppMode,
    already_present: bool,
    has_anilist_id: bool,
) -> bool {
    mode == AppMode::Normal && !already_present && has_anilist_id
}

fn apply_search_results(
    app: &mut AppState,
    results: Vec<api::AnimeItem>,
    anilist_media: Vec<api::AniListMedia>,
    finish_search: bool,
) {
    app.search_results = results;
    app.anilist_media = anilist_media;
    for item in &app.search_results {
        app.details_cache
            .insert(item.id, anime_details_from_item(item));
    }
    rebuild_franchise_projection(app);
    if finish_search {
        app.search_query.clear();
        app.search_cursor = 0;
    }

    app.focus = FocusPanel::SearchList;
    app.current_sources = None;
    app.current_sources_key = None;
    app.current_details = None;
    app.current_poster = None;
    app.studio_anime_ids.clear();
    app.sidebar_anime_idx = None;
    app.sidebar_subject_id = None;
    app.selected_release_index = None;
    app.selected_season_index = None;
    app.season_list_state.select(None);
    app.selected_dubbing_index = None;
    app.dubbing_list_state.select(None);
    app.selected_episode_index = None;
    app.episode_list_state.select(None);

    if !app.franchise_groups.is_empty() {
        app.result_list_state.select(Some(0));
        app.selected_group_index = Some(0);
        let rep = app.franchise_groups[0][0];
        app.selected_result_index = Some(rep);
        let canonical_id = app.search_results[rep].id;
        app.select_sidebar_subject(app.canonical_sidebar_subject().or(Some(canonical_id)));
        app.set_activity("Завантаження вибраного аніме…");
    } else {
        app.clear_activity();
        app.result_list_state.select(None);
        app.selected_group_index = None;
        app.selected_result_index = None;
        app.set_info_status("Нічого не знайдено");
    }
}

fn rebuild_franchise_projection(app: &mut AppState) {
    let selected_anchor = app
        .selected_franchise_catalog()
        .and_then(|catalog| catalog.anchor_anilist_id);
    let selected_release_anilist = app
        .selected_release()
        .and_then(|release| release.anilist_id);

    let catalogs = api::build_franchise_catalogs(&app.search_results, &app.anilist_media);
    let groups = catalogs
        .iter()
        .map(|catalog| {
            catalog
                .releases
                .iter()
                .filter_map(|release| release.anihub_id)
                .filter_map(|anime_id| {
                    app.search_results
                        .iter()
                        .position(|item| item.id == anime_id)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    app.franchise_catalogs = catalogs;
    app.franchise_groups = groups;

    if let Some(anchor) = selected_anchor {
        app.selected_group_index = app
            .franchise_catalogs
            .iter()
            .position(|catalog| catalog.anchor_anilist_id == Some(anchor));
    }
    if app.selected_group_index.is_none() && !app.franchise_catalogs.is_empty() {
        app.selected_group_index = Some(0);
    }
    if let Some(group_index) = app.selected_group_index {
        app.selected_result_index = app
            .franchise_groups
            .get(group_index)
            .and_then(|group| group.first())
            .copied();
        if let Some(anilist_id) = selected_release_anilist {
            app.selected_release_index = app.franchise_catalogs[group_index]
                .releases
                .iter()
                .position(|release| release.anilist_id == Some(anilist_id));
            app.season_list_state.select(app.selected_release_index);
        }
    }
    if app.focus != FocusPanel::SearchList {
        app.refresh_selected_release();
    }
}

fn anime_details_from_item(item: &api::AnimeItem) -> api::AnimeDetails {
    api::AnimeDetails {
        id: item.id,
        anilist_id: item.anilist_id,
        slug: item.slug.clone(),
        title_ukrainian: item.title_ukrainian.clone(),
        title_original: item.title_original.clone(),
        title_english: item.title_english.clone(),
        status: item.status.clone(),
        anime_type: item.anime_type.clone(),
        year: item.year,
        has_ukrainian_dub: item.has_ukrainian_dub,
        poster_url: item.poster_url.clone(),
        episodes_count: item.episodes_count,
        description: item.description.clone(),
        rating: item.rating,
        genres: item.genres.clone(),
        dubbing_studios: item.dubbing_studios.clone(),
    }
}

#[cfg(test)]
mod staged_source_loading_tests {
    use super::*;

    fn source_response(first_episode: u32, count: u32) -> EpisodeSourcesResponse {
        EpisodeSourcesResponse {
            ashdi: vec![api::AshdiStudio {
                id: 1,
                studio_name: "Test".to_string(),
                season_number: 1,
                episodes: (first_episode..first_episode + count)
                    .map(|episode_number| api::AshdiEpisode {
                        episode_number,
                        display_episode_number: None,
                        title: format!("Episode {episode_number}"),
                        url: format!("https://example.test/{episode_number}"),
                        ashdi_episode_id: episode_number.to_string(),
                    })
                    .collect(),
                episodes_count: count,
            }],
            moonanime: Vec::new(),
        }
    }

    #[test]
    fn preview_uses_only_the_first_franchise_entry() {
        assert_eq!(
            source_keys_for_scope(
                vec![
                    EpisodeSourcesKey::new(10, 1),
                    EpisodeSourcesKey::new(20, 2),
                    EpisodeSourcesKey::new(30, 3),
                ],
                &SourceLoadScope::Preview,
            ),
            vec![EpisodeSourcesKey::new(10, 1)]
        );
    }

    #[test]
    fn library_root_does_not_request_episode_sources() {
        assert!(
            source_keys_for_scope(
                vec![EpisodeSourcesKey::new(10, 1), EpisodeSourcesKey::new(20, 2),],
                &SourceLoadScope::DetailsOnly,
            )
            .is_empty()
        );
    }

    #[test]
    fn opened_title_uses_every_franchise_entry() {
        assert_eq!(
            source_keys_for_scope(
                vec![
                    EpisodeSourcesKey::new(10, 1),
                    EpisodeSourcesKey::new(20, 2),
                    EpisodeSourcesKey::new(30, 3),
                ],
                &SourceLoadScope::Full,
            ),
            vec![
                EpisodeSourcesKey::new(10, 1),
                EpisodeSourcesKey::new(20, 2),
                EpisodeSourcesKey::new(30, 3),
            ]
        );
    }

    #[test]
    fn source_requests_for_the_same_anime_keep_seasons_disjoint() {
        assert_ne!(ResourceKey::sources(5180, 1), ResourceKey::sources(5180, 2));
        assert_ne!(
            EpisodeSourcesKey::new(24675, 1),
            EpisodeSourcesKey::new(24675, 3)
        );
    }

    #[test]
    fn ongoing_release_hides_future_placeholder_episodes() {
        let sources = cap_sources_to_available_episodes(source_response(1, 14), Some(3));

        assert_eq!(sources.ashdi[0].episodes_count, 3);
        assert_eq!(sources.ashdi[0].episodes.len(), 3);
        assert_eq!(sources.ashdi[0].episodes[2].episode_number, 3);
    }

    #[test]
    fn split_cour_cap_uses_ordinal_count_not_raw_episode_number() {
        let sources = cap_sources_to_available_episodes(source_response(12, 13), Some(13));

        assert_eq!(sources.ashdi[0].episodes_count, 13);
        assert_eq!(
            sources.ashdi[0].episodes.first().unwrap().episode_number,
            12
        );
        assert_eq!(sources.ashdi[0].episodes.last().unwrap().episode_number, 24);
    }

    #[test]
    fn library_metadata_never_extends_search_results() {
        assert!(should_add_details_to_search(AppMode::Normal, false, true));
        assert!(!should_add_details_to_search(AppMode::Library, false, true));
        assert!(!should_add_details_to_search(
            AppMode::LibrarySeason,
            false,
            true
        ));
        assert!(!should_add_details_to_search(
            AppMode::LibraryEpisode,
            false,
            true
        ));
    }

    #[test]
    fn network_failures_get_short_user_facing_hints() {
        assert_eq!(
            resource_error_hint(&LoadError::Network("dns details".to_string())),
            "Немає з’єднання з AniHub"
        );
        assert_eq!(
            resource_error_hint(&LoadError::Http {
                status: 503,
                message: "upstream".to_string(),
                retry_after: None,
            }),
            "AniHub тимчасово недоступний · HTTP 503"
        );
    }
}
