mod api;
mod atomic_file;
mod cache;
mod discord;
mod library_refresh;
mod platform;
mod playback;
mod player;
mod poster_cache;
mod resource_coordinator;
mod settings;
mod storage;
mod ui;

use crate::discord::{DiscordPresence, PRESENCE_SYNC_INTERVAL, PresenceActivity, WatchingPresence};
use crate::playback::*;
use crate::resource_coordinator::ResourceCoordinator;
#[cfg(test)]
use crate::resource_coordinator::{
    SourceLoadScope, cap_sources_to_available_episodes, resource_error_hint, source_keys_for_scope,
};
use anyhow::{Result, bail};
use api::{EpisodeSourcesKey, EpisodeSourcesResponse};
#[cfg(test)]
use api::{ResourceKey, resource::LoadError};
use crossterm::{
    ExecutableCommand,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::collections::HashMap;
use std::env;
use std::io::stdout;
use std::time::Instant;
use ui::{AppMode, AppState, FocusPanel};

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
    let mut last_discord_sync = Instant::now();
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
            last_discord_sync = Instant::now();
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
        let previous_now_playing = app.now_playing.clone();
        let mut presence_immediate = false;
        for event in playback_events {
            presence_immediate |= matches!(
                &event,
                PlaybackEvent::SessionStarted { .. }
                    | PlaybackEvent::SessionStopped { .. }
                    | PlaybackEvent::Error { .. }
                    | PlaybackEvent::PauseChanged { .. }
            );
            persist_playback_event(&mut app, &mut persisted_positions, event);
        }
        // Duration often arrives only after the first progress snapshot — push
        // immediately so Discord can switch from elapsed-only to a full bar.
        presence_immediate |=
            duration_became_known(previous_now_playing.as_ref(), app.now_playing.as_ref())
                || episode_identity_changed(
                    previous_now_playing.as_ref(),
                    app.now_playing.as_ref(),
                );
        let presence_due = app.settings.discord_presence
            && app.now_playing.is_some()
            && last_discord_sync.elapsed() >= PRESENCE_SYNC_INTERVAL;
        if presence_immediate || presence_due {
            sync_discord_presence(&app, &discord_presence);
            last_discord_sync = Instant::now();
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

    let session_settings_error = app.persist_library_session().err();
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
    if let Some(error) = session_settings_error {
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
    discord.update(PresenceActivity::watching(WatchingPresence {
        title: &now.anime_title,
        season: now.season,
        episode: now.episode,
        studio: &now.studio_name,
        poster_url: app
            .details_cache
            .get(&now.anime_id)
            .and_then(|details| details.poster_url.clone())
            .or_else(|| app.poster_url_for_subject(now.anime_id)),
        position: now.position,
        duration: now.duration,
        paused: now.paused,
    }));
}

fn duration_became_known(
    previous: Option<&ui::app::NowPlaying>,
    current: Option<&ui::app::NowPlaying>,
) -> bool {
    match (previous, current) {
        (Some(previous), Some(current)) => previous.duration <= 0.0 && current.duration > 0.0,
        (None, Some(current)) => current.duration > 0.0,
        _ => false,
    }
}

fn episode_identity_changed(
    previous: Option<&ui::app::NowPlaying>,
    current: Option<&ui::app::NowPlaying>,
) -> bool {
    match (previous, current) {
        (Some(previous), Some(current)) => {
            previous.anime_id != current.anime_id
                || previous.season != current.season
                || previous.episode != current.episode
                || previous.studio_name != current.studio_name
        }
        (None, Some(_)) | (Some(_), None) => true,
        (None, None) => false,
    }
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
            let item = app.search.results.iter().find(|item| item.id == anime_id);
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
    app.search.results = vec![anime_item];
    app.search.anilist_media.clear();
    app.search.franchise_catalogs = api::build_franchise_catalogs(&app.search.results, &[]);
    app.search.franchise_groups = vec![vec![0]];
    app.search.selected_group_index = Some(0);
    app.search.selected_result_index = Some(0);
    app.search.selected_release_index = Some(0);
    app.search.result_list_state.select(Some(0));
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
    if app.library.items.is_empty() {
        app.open_library();
    }

    app.library.anime_index = app
        .library
        .items
        .iter()
        .position(|item| item.anime_ids.contains(&progress.anime_id))
        .or_else(|| (!app.library.items.is_empty()).then_some(0));
    app.library.anime_list_state.select(app.library.anime_index);

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
    app.search.results = results;
    app.search.anilist_media = anilist_media;
    for item in &app.search.results {
        app.details_cache
            .insert(item.id, anime_details_from_item(item));
    }
    rebuild_franchise_projection(app);
    if finish_search {
        app.search.query.clear();
        app.search.cursor = 0;
    }

    app.focus = FocusPanel::SearchList;
    app.current_sources = None;
    app.current_sources_key = None;
    app.current_details = None;
    app.current_poster = None;
    app.studio_anime_ids.clear();
    app.sidebar_anime_idx = None;
    app.sidebar_subject_id = None;
    app.search.selected_release_index = None;
    app.selected_season_index = None;
    app.season_list_state.select(None);
    app.selected_dubbing_index = None;
    app.dubbing_list_state.select(None);
    app.selected_episode_index = None;
    app.episode_list_state.select(None);

    if !app.search.franchise_groups.is_empty() {
        app.search.result_list_state.select(Some(0));
        app.search.selected_group_index = Some(0);
        let rep = app.search.franchise_groups[0][0];
        app.search.selected_result_index = Some(rep);
        let canonical_id = app.search.results[rep].id;
        app.select_sidebar_subject(app.canonical_sidebar_subject().or(Some(canonical_id)));
        app.set_activity("Завантаження вибраного аніме…");
    } else {
        app.clear_activity();
        app.search.result_list_state.select(None);
        app.search.selected_group_index = None;
        app.search.selected_result_index = None;
        app.set_info_status("Нічого не знайдено");
    }
}

fn rebuild_franchise_projection(app: &mut AppState) {
    let selected_anchor = app
        .selected_franchise_catalog()
        .and_then(|catalog| catalog.anchor_anilist_id);
    let selected_title = app
        .selected_franchise_catalog()
        .map(|catalog| catalog.canonical_title.clone());
    let selected_release_anilist = app
        .selected_release()
        .and_then(|release| release.anilist_id);

    let catalogs = api::build_franchise_catalogs(&app.search.results, &app.search.anilist_media);
    let groups = catalogs
        .iter()
        .map(|catalog| {
            catalog
                .releases
                .iter()
                .filter_map(|release| release.anihub_id)
                .filter_map(|anime_id| {
                    app.search
                        .results
                        .iter()
                        .position(|item| item.id == anime_id)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    app.search.franchise_catalogs = catalogs;
    app.search.franchise_groups = groups;
    app.sort_search_projection();

    if let Some(anchor) = selected_anchor {
        app.search.selected_group_index = app
            .search
            .franchise_catalogs
            .iter()
            .position(|catalog| catalog.anchor_anilist_id == Some(anchor));
    } else if let Some(title) = selected_title {
        app.search.selected_group_index = app
            .search
            .franchise_catalogs
            .iter()
            .position(|catalog| catalog.canonical_title == title);
    }
    if app.search.selected_group_index.is_none() && !app.search.franchise_catalogs.is_empty() {
        app.search.selected_group_index = Some(0);
    }
    if let Some(group_index) = app.search.selected_group_index {
        app.search.selected_result_index = app
            .search
            .franchise_groups
            .get(group_index)
            .and_then(|group| group.first())
            .copied();
        if let Some(anilist_id) = selected_release_anilist {
            app.search.selected_release_index = app.search.franchise_catalogs[group_index]
                .releases
                .iter()
                .position(|release| release.anilist_id == Some(anilist_id));
            app.season_list_state
                .select(app.search.selected_release_index);
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
