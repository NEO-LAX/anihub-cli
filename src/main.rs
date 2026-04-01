mod api;
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
use api::{AshdiStudio, EpisodeSourcesResponse};
use crossterm::{
    ExecutableCommand,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::stdout;
use tokio::task::JoinSet;
use ui::{AppMode, AppState, ContinueRequest, FocusPanel};

#[derive(Clone)]
struct PlayTarget {
    anime_id: u32,
    anime_title: String,
    player_title: String,
    season: u32,
    episode: u32,
    episode_title: String,
    stream_page_url: String,
    start_time: Option<f64>,
    studio_name: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Picker MUST be initialized before enable_raw_mode
    let picker = ratatui_image::picker::Picker::from_query_stdio()
        .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut app = AppState::new(picker)?;

    loop {
        terminal.draw(|f| ui::render(f, &mut app))?;

        // Drain prefetch results each tick
        if let Some(rx) = &mut app.prefetch_rx {
            loop {
                match rx.try_recv() {
                    Ok((id, details, sources)) => {
                        if let Some(d) = details {
                            app.details_cache.insert(id, d);
                        }
                        if let Some(s) = sources {
                            app.sources_cache.insert(id, s);
                        }
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        app.prefetch_rx = None;
                        app.prefetching = false;
                        break;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                }
            }
        }

        if let Some(ids) = app.pending_prefetch_ids.take() {
            spawn_prefetch_for_ids(&mut app, ids);
        }

        // Фоновий запит постера без показу Loading popup
        if let Some(anime_id) = app.poster_fetch_pending.take() {
            fetch_poster_for_anime(&mut app, anime_id).await;
        }

        if app.loading {
            app.loading = false;
            handle_loading_tasks(&mut app).await;
        }

        if app.play_episode {
            app.play_episode = false;
            app.loading = true;
            terminal.draw(|f| ui::render(f, &mut app))?;
            app.loading = false;
            start_episode_playback(&mut app).await;
        }

        if let Some(request) = app.continue_request.take() {
            app.loading = true;
            terminal.draw(|f| ui::render(f, &mut app))?;
            app.loading = false;
            continue_playback(&mut app, request).await;
        }

        let mut mpv_events = Vec::new();
        if let Some(rx) = &mut app.mpv_rx {
            while let Ok(event) = rx.try_recv() {
                mpv_events.push(event);
            }
        }

        for event in mpv_events {
            match event {
                    crate::player::MpvEvent::Progress(t, d) => {
                        app.mpv_last_time = t;
                        app.mpv_last_duration = d;
                    }
                    crate::player::MpvEvent::PlaylistPos(pos) => {
                        if pos < app.mpv_playlist.len() {
                            // Зберігаємо прогрес для попередньої серії
                            if let Some((anime_id, title, season, episode, studio_name)) = &app.pending_progress {
                                let _ = app.storage.update_progress(
                                    *anime_id,
                                    title,
                                    *season,
                                    *episode,
                                    studio_name,
                                    app.mpv_last_time,
                                    app.mpv_last_duration,
                                );
                                app.history = app.storage.load_history().unwrap_or_default();
                            }

                            // Оновлюємо pending_progress на нову серію
                            app.pending_progress = Some(app.mpv_playlist[pos].clone());
                            app.mpv_last_time = 0.0;
                            app.mpv_last_duration = 0.0;

                            if let Some(pending) = &app.pending_progress {
                                let title = format!("{} - Серія {}", pending.1, pending.3);
                                let player = app.mpv_player.clone();
                                tokio::spawn(async move {
                                    let _ = player.send_command(serde_json::json!(["set_property", "force-media-title", title])).await;
                                });

                                // Якщо це остання серія в списку відтворення, спробуємо знайти наступну
                                if pos == app.mpv_playlist.len() - 1 {
                                    if let Some(next_target) = get_next_episode(&app, pending) {
                                        app.mpv_playlist.push((
                                            next_target.anime_id,
                                            next_target.anime_title.clone(),
                                            next_target.season,
                                            next_target.episode,
                                            next_target.studio_name.clone(),
                                        ));
                                        let player2 = app.mpv_player.clone();
                                        tokio::spawn(async move {
                                            let m3u8 = if next_target.stream_page_url.starts_with("https://moonanime.art") {
                                                try_moonanime_stream(&next_target.stream_page_url).await
                                            } else {
                                                if let Ok(parser) = api::AshdiParser::new() {
                                                    parser.extract_m3u8(&next_target.stream_page_url).await.ok()
                                                } else {
                                                    None
                                                }
                                            };
                                            if let Some(url) = m3u8 {
                                                let _ = player2.send_command(serde_json::json!(["loadfile", url, "append"])).await;
                                            }
                                        });
                                    }
                                }
                            }
                        }
                    }
                    crate::player::MpvEvent::FileStarted => {}
                    crate::player::MpvEvent::FileLoaded => {
                        if let Some((anime_id, _title, season, episode, studio_name)) = &app.pending_progress {
                            let key = crate::storage::StorageManager::make_progress_key(*anime_id, *season, *episode, studio_name);
                            if let Some(saved) = app.history.progress.get(&key) {
                                if saved.timestamp > 0.0 && saved.timestamp < saved.duration.max(1200.0) && !saved.watched {
                                    let player = app.mpv_player.clone();
                                    let time_to_resume = saved.timestamp;
                                    tokio::spawn(async move {
                                        let _ = player.send_command(serde_json::json!(["set_property", "time-pos", time_to_resume])).await;
                                    });
                                }
                            }
                        }
                    }
                    crate::player::MpvEvent::EndFile => {}
                }
        }

        if app.is_playing {
            check_playback_finished(&mut app).await;
        }

        app.handle_events()?;

        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

/// Завантажує постер для anime_id без блокування UI.
async fn fetch_poster_for_anime(app: &mut AppState, anime_id: u32) {
    if app.poster_cache.contains_key(&anime_id) {
        if let Some(img) = app.poster_cache.get(&anime_id).cloned() {
            app.current_poster = Some(app.picker.new_resize_protocol(img));
        }
        return;
    }
    let poster_url = app
        .details_cache
        .get(&anime_id)
        .and_then(|d| d.poster_url.clone())
        .or_else(|| {
            app.current_details
                .as_ref()
                .filter(|d| d.id == anime_id)
                .and_then(|d| d.poster_url.clone())
        });
    // Якщо URL не знайдено в кешах (напр. anime_id не в search_results) —
    // завантажуємо деталі напряму, щоб отримати poster_url.
    let poster_url = if poster_url.is_none() {
        if let Ok(details) = app.api_client.get_anime_details(anime_id).await {
            let url = details.poster_url.clone();
            app.details_cache.insert(anime_id, details);
            url
        } else {
            None
        }
    } else {
        poster_url
    };
    if let Some(url) = poster_url {
        if let Ok(img) = app.api_client.fetch_poster(&url).await {
            let proto = app.picker.new_resize_protocol(img.clone());
            app.poster_cache.insert(anime_id, img);
            app.current_poster = Some(proto);
        }
    }
}

async fn start_episode_playback(app: &mut AppState) {
    let Some(e_idx) = app.selected_episode_index else {
        return;
    };

    // Витягуємо дані з обраної студії до будь-яких мутацій
    let studio_info = app.selected_studio().and_then(|s| {
        s.episodes
            .get(e_idx)
            .map(|ep| (ep.url.clone(), ep.episode_number, s.season_number, s.studio_name.clone()))
    });
    let (target_url, episode_num, actual_season, studio_name) = match studio_info {
        Some(info) => info,
        None => return,
    };

    // Знаходимо anime_id для прогресу через studio_anime_ids
    let season_num = match app.selected_season_num() {
        Some(n) => n,
        None => return,
    };
    let dub_idx = match app.selected_dubbing_index {
        Some(i) => i,
        None => return,
    };
    let studio_idx = app.current_sources.as_ref().and_then(|sources| {
        sources
            .ashdi
            .iter()
            .enumerate()
            .filter(|(_, s)| s.season_number == season_num)
            .nth(dub_idx)
            .map(|(i, _)| i)
    });
    let progress_anime_id = studio_idx
        .and_then(|i| app.studio_anime_ids.get(i).copied())
        .or_else(|| app.current_details.as_ref().map(|details| details.id))
        .or_else(|| {
            app.selected_result_index
                .and_then(|idx| app.search_results.get(idx).map(|a| a.id))
        })
        .unwrap_or(0);
    let progress_title = app
        .current_details
        .as_ref()
        .filter(|details| details.id == progress_anime_id)
        .map(|details| details.title_ukrainian.clone())
        .or_else(|| {
            app.search_results
                .iter()
                .find(|a| a.id == progress_anime_id)
                .map(|a| a.title_ukrainian.clone())
        })
        .or_else(|| {
            app.library_selected_anime()
                .map(|anime| anime.anime_title.clone())
        })
        .unwrap_or_default();

    let player_title = app
        .current_details
        .as_ref()
        .map(|details| {
            format!(
                "{} ({})",
                details.title_ukrainian,
                details.year.unwrap_or(0)
            )
        })
        .or_else(|| {
            app.selected_result_index.and_then(|result_idx| {
                app.search_results
                    .get(result_idx)
                    .map(|anime| format!("{} ({})", anime.title_ukrainian, anime.year.unwrap_or(0)))
            })
        })
        .unwrap_or_else(|| progress_title.clone());

    let episode_title = format!("Серія {}", episode_num);
    let target = PlayTarget {
        anime_id: progress_anime_id,
        anime_title: progress_title,
        player_title,
        season: actual_season,
        episode: episode_num,
        episode_title,
        stream_page_url: target_url,
        start_time: None,
        studio_name,
    };

    play_target(app, target).await;
}

async fn continue_playback(app: &mut AppState, request: ContinueRequest) {
    let continue_in_library = matches!(
        &request,
        ContinueRequest::Group {
            in_library: true,
            ..
        }
    );
    let progress = match request {
        ContinueRequest::Latest => match app.storage.latest_progress() {
            Ok(Some(progress)) => progress,
            Ok(None) => {
                app.set_info_status("Немає збереженого прогресу");
                return;
            }
            Err(e) => {
                app.set_error_status(format!("Не вдалося прочитати історію: {}", e));
                return;
            }
        },
        ContinueRequest::Group {
            anime_ids,
            in_library: _,
        } => match app
            .history
            .progress
            .values()
            .filter(|progress| anime_ids.contains(&progress.anime_id))
            .max_by_key(|progress| progress.updated_at)
            .cloned()
        {
            Some(progress) => progress,
            None => {
                app.set_info_status("Немає збереженого прогресу");
                return;
            }
        },
    };

    let details = match get_or_fetch_details(app, progress.anime_id).await {
        Some(details) => details,
        None => {
            app.set_error_status("Не вдалося завантажити дані аніме");
            return;
        }
    };
    let sources = match get_or_fetch_sources(app, progress.anime_id).await {
        Some(sources) => sources,
        None => {
            app.set_error_status("Не вдалося завантажити серії");
            return;
        }
    };

    let resolved = match resolve_continue_target(&progress, &sources) {
        Some(target) => target,
        None => {
            app.set_info_status("Усі серії переглянуто");
            return;
        }
    };

    if continue_in_library {
        apply_library_continue_context(app, &progress, &details, &sources, &resolved);
    } else {
        apply_continue_context(app, &details, &sources, &resolved);
    }

    let player_title = format!(
        "{} ({})",
        details.title_ukrainian,
        details.year.unwrap_or(0)
    );
    let target = PlayTarget {
        anime_id: progress.anime_id,
        anime_title: progress.anime_title.clone(),
        player_title,
        season: resolved.season,
        episode: resolved.episode,
        episode_title: format!("Серія {}", resolved.episode),
        stream_page_url: resolved.url,
        start_time: resolved.start_time,
        studio_name: resolved.studio_name,
    };

    play_target(app, target).await;
}

async fn play_target(app: &mut AppState, target: PlayTarget) {
    // Якщо щось уже грає — зберігаємо прогрес поточної серії
    if app.is_playing {
        if let Some((anime_id, title, season, episode, studio_name)) = &app.pending_progress {
            let _ = app.storage.update_progress(
                *anime_id,
                title,
                *season,
                *episode,
                studio_name,
                app.mpv_last_time,
                app.mpv_last_duration,
            );
            app.history = app.storage.load_history().unwrap_or_default();
        }
    }

    let m3u8 = if target.stream_page_url.starts_with("https://moonanime.art") {
        match try_moonanime_stream(&target.stream_page_url).await {
            Some(u) => u,
            None => {
                std::process::Command::new("xdg-open")
                    .arg(&target.stream_page_url)
                    .spawn()
                    .ok();
                app.set_info_status("MoonAnime відкрито у браузері");
                return;
            }
        }
    } else {
        let parser = match api::AshdiParser::new() {
            Ok(p) => p,
            Err(e) => {
                app.set_error_status(format!("Помилка парсера: {}", e));
                return;
            }
        };
        match parser.extract_m3u8(&target.stream_page_url).await {
            Ok(u) => u,
            Err(e) => {
                app.set_error_status(format!("Помилка парсингу: {}", e));
                return;
            }
        }
    };

    // Якщо MPV уже запущено — використовуємо IPC
    if app.is_playing && app.mpv_child.is_some() {
        let title = format!("{} - {}", target.player_title, target.episode_title);
        let res = async {
            app.mpv_player
                .send_command(serde_json::json!(["loadfile", m3u8]))
                .await?;
            app.mpv_player
                .send_command(serde_json::json!(["set_property", "force-media-title", title]))
                .await?;
            if let Some(t) = target.start_time {
                if t > 0.0 {
                    app.mpv_player
                        .send_command(serde_json::json!(["set_property", "time-pos", t]))
                        .await?;
                }
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;

        if res.is_ok() {
            app.pending_progress = Some((
                target.anime_id,
                target.anime_title.clone(),
                target.season,
                target.episode,
                target.studio_name.clone(),
            ));
            app.mpv_playlist.clear();
            app.mpv_playlist.push((
                target.anime_id,
                target.anime_title.clone(),
                target.season,
                target.episode,
                target.studio_name.clone(),
            ));
            app.mpv_last_time = 0.0;
            app.mpv_last_duration = 0.0;
            
            if let Some(next_target) = get_next_episode(&app, app.pending_progress.as_ref().unwrap()) {
                app.mpv_playlist.push((
                    next_target.anime_id,
                    next_target.anime_title.clone(),
                    next_target.season,
                    next_target.episode,
                    next_target.studio_name.clone(),
                ));
                let player2 = app.mpv_player.clone();
                tokio::spawn(async move {
                    let m3u8 = if next_target.stream_page_url.starts_with("https://moonanime.art") {
                        try_moonanime_stream(&next_target.stream_page_url).await
                    } else {
                        if let Ok(parser) = api::AshdiParser::new() {
                            parser.extract_m3u8(&next_target.stream_page_url).await.ok()
                        } else {
                            None
                        }
                    };
                    if let Some(url) = m3u8 {
                        let _ = player2.send_command(serde_json::json!(["loadfile", url, "append"])).await;
                    }
                });
            }
            return;
        }
        // Якщо IPC не спрацював (наприклад, mpv "завис") — йдемо далі до перезапуску
    }

    // Запуск нового процесу MPV
    match app
        .mpv_player
        .start(
            &m3u8,
            target.start_time,
            &target.player_title,
            &target.episode_title,
        )
        .await
    {
        Ok((child, rx, monitor)) => {
            app.mpv_child = Some(child);
            app.mpv_rx = Some(rx);
            app.mpv_monitor = Some(monitor);
            app.pending_progress = Some((
                target.anime_id,
                target.anime_title.clone(),
                target.season,
                target.episode,
                target.studio_name.clone(),
            ));
            app.is_playing = true;
            app.mpv_playlist.clear();
            app.mpv_playlist.push((
                target.anime_id,
                target.anime_title.clone(),
                target.season,
                target.episode,
                target.studio_name.clone(),
            ));
            app.mpv_last_time = 0.0;
            app.mpv_last_duration = 0.0;
            
            if let Some(next_target) = get_next_episode(&app, app.pending_progress.as_ref().unwrap()) {
                app.mpv_playlist.push((
                    next_target.anime_id,
                    next_target.anime_title.clone(),
                    next_target.season,
                    next_target.episode,
                    next_target.studio_name.clone(),
                ));
                let player2 = app.mpv_player.clone();
                tokio::spawn(async move {
                    let m3u8 = if next_target.stream_page_url.starts_with("https://moonanime.art") {
                        try_moonanime_stream(&next_target.stream_page_url).await
                    } else {
                        if let Ok(parser) = api::AshdiParser::new() {
                            parser.extract_m3u8(&next_target.stream_page_url).await.ok()
                        } else {
                            None
                        }
                    };
                    if let Some(url) = m3u8 {
                        let _ = player2.send_command(serde_json::json!(["loadfile", url, "append"])).await;
                    }
                });
            }
        }
        Err(e) => {
            app.set_error_status(format!("Помилка відтворення: {}", e));
        }
    }
}

#[derive(Clone)]
struct ContinueResolvedEpisode {
    season: u32,
    episode: u32,
    season_index: usize,
    dubbing_index: usize,
    episode_index: usize,
    url: String,
    start_time: Option<f64>,
    studio_name: String,
}

fn resolve_continue_target(
    progress: &storage::WatchProgress,
    sources: &EpisodeSourcesResponse,
) -> Option<ContinueResolvedEpisode> {
    let mut seasons: Vec<u32> = sources
        .ashdi
        .iter()
        .map(|studio| studio.season_number)
        .collect();
    seasons.sort();
    seasons.dedup();

    let season_index = seasons
        .iter()
        .position(|season| *season == progress.season)?;
    
    let current_studio_data = sources
        .ashdi
        .iter()
        .filter(|studio| studio.season_number == progress.season)
        .enumerate()
        .find(|(_, studio)| studio.studio_name == progress.studio_name)
        .or_else(|| {
            sources
                .ashdi
                .iter()
                .filter(|studio| studio.season_number == progress.season)
                .enumerate()
                .next()
        })?;
    
    let current_dubbing_index = current_studio_data.0;
    let current_studio = current_studio_data.1;

    let current_episode_index = current_studio
        .episodes
        .iter()
        .position(|episode| episode.episode_number == progress.episode)?;

    if !progress.watched {
        let episode = current_studio.episodes.get(current_episode_index)?;
        return Some(ContinueResolvedEpisode {
            season: progress.season,
            episode: progress.episode,
            season_index,
            dubbing_index: current_dubbing_index,
            episode_index: current_episode_index,
            url: episode.url.clone(),
            start_time: Some(progress.timestamp),
            studio_name: current_studio.studio_name.clone(),
        });
    }

    if let Some(next_episode) = current_studio.episodes.get(current_episode_index + 1) {
        return Some(ContinueResolvedEpisode {
            season: progress.season,
            episode: next_episode.episode_number,
            season_index,
            dubbing_index: current_dubbing_index,
            episode_index: current_episode_index + 1,
            url: next_episode.url.clone(),
            start_time: Some(0.0),
            studio_name: current_studio.studio_name.clone(),
        });
    }

    let next_season = seasons.get(season_index + 1).copied()?;
    let next_studio_data = sources
        .ashdi
        .iter()
        .filter(|studio| studio.season_number == next_season)
        .enumerate()
        .next()?;
    let episode = next_studio_data.1.episodes.first()?;
    Some(ContinueResolvedEpisode {
        season: next_season,
        episode: episode.episode_number,
        season_index: season_index + 1,
        dubbing_index: next_studio_data.0,
        episode_index: 0,
        url: episode.url.clone(),
        start_time: Some(0.0),
        studio_name: next_studio_data.1.studio_name.clone(),
    })
}

async fn get_or_fetch_details(app: &mut AppState, anime_id: u32) -> Option<api::AnimeDetails> {
    if let Some(details) = app.details_cache.get(&anime_id).cloned() {
        return Some(details);
    }
    let details = app.api_client.get_anime_details(anime_id).await.ok()?;
    app.details_cache.insert(anime_id, details.clone());
    Some(details)
}

async fn get_or_fetch_sources(app: &mut AppState, anime_id: u32) -> Option<EpisodeSourcesResponse> {
    if let Some(sources) = app.sources_cache.get(&anime_id).cloned() {
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

fn apply_continue_context(
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

fn apply_library_continue_context(
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

async fn check_playback_finished(app: &mut AppState) {
    let finished = if let Some(child) = &mut app.mpv_child {
        match child.try_wait() {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => true,
        }
    } else {
        false
    };

    if finished {
        app.mpv_child = None;
        let stopped_time = app.mpv_last_time;
        let duration = app.mpv_last_duration;
        app.mpv_rx = None;
        let _ = app.mpv_monitor.take(); // Abort/Clean up the monitor task

        if let Some((anime_id, title, season, episode, studio_name)) = app.pending_progress.take() {
            let _ = app.storage.update_progress(
                anime_id,
                &title,
                season,
                episode,
                &studio_name,
                stopped_time,
                duration,
            );
            app.history = app.storage.load_history().unwrap_or_default();
        }
        app.is_playing = false;
        app.mpv_player.cleanup();
    }
}

async fn handle_loading_tasks(app: &mut AppState) {
    app.clear_status();

    // 1. Пошук
    if app.mode == AppMode::Normal && !app.search_query.is_empty() {
        let query = app.search_query.trim().to_string();
        let cache_key = query.to_lowercase();
        let cached = app.search_cache.get(&cache_key).cloned();

        match cached {
            Some(results) => {
                apply_search_results(app, results);
            }
            None => match app.api_client.search_anime(&query).await {
                Ok(results) => {
                    let results = api::deduplicate_anime(results);
                    app.search_cache.insert(cache_key, results.clone());
                    apply_search_results(app, results);
                }
                Err(e) => app.set_error_status(format!("Помилка пошуку: {}", e)),
            },
        }
        return;
    }

    if app.is_library_mode() {
        if let Some((selected_ids, representative_id)) = app
            .library_selected_anime()
            .map(|anime| (anime.anime_ids.clone(), anime.latest_progress.anime_id))
        {
            let current_ids: Vec<u32> = app
                .studio_anime_ids
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let expected_ids: Vec<u32> = selected_ids
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();

            if app.current_sources.is_none() || current_ids != expected_ids {
                load_library_combined_sources(app, &selected_ids, representative_id).await;
            }
        }

        if let Some(context_anime_id) = app.library_selected_anime_id() {
            if app.current_details.as_ref().map(|details| details.id) != Some(context_anime_id) {
                if let Some(cached) = app.details_cache.get(&context_anime_id).cloned() {
                    app.current_details = Some(cached);
                } else if let Ok(details) = app.api_client.get_anime_details(context_anime_id).await
                {
                    app.current_details = Some(details.clone());
                    app.details_cache.insert(context_anime_id, details);
                }
            }

            if app.current_poster.is_none() && app.poster_fetch_pending.is_none() {
                app.poster_fetch_pending = Some(context_anime_id);
            }
        }
        return;
    }

    // 2. Завантаження деталей і джерел для вибраної групи
    if app.mode == AppMode::Normal && app.focus == FocusPanel::SearchList {
        if let Some(idx) = app.selected_result_index {
            if let Some(item) = app.search_results.get(idx).cloned() {
                // Деталі
                if app.current_details.is_none() {
                    if let Some(cached) = app.details_cache.get(&item.id).cloned() {
                        app.current_details = Some(cached);
                    } else if let Ok(details) = app.api_client.get_anime_details(item.id).await {
                        app.current_details = Some(details);
                    }
                }

                // Об'єднані джерела з усіх TV-членів франшизи
                if app.current_sources.is_none() {
                    load_combined_sources(app, item.id).await;
                }

                // Постер: ставимо в чергу на фоновий fetch (після наступного render)
                if app.current_poster.is_none() && app.poster_fetch_pending.is_none() {
                    let first_tv_id = app
                        .studio_anime_ids
                        .first()
                        .copied()
                        .unwrap_or_else(|| get_first_tv_id(app).unwrap_or(item.id));
                    app.poster_fetch_pending = Some(first_tv_id);
                }
            }
        }
        return;
    }

    // 3. Завантаження деталей для обраного сезону у SeasonList/DubbingList/EpisodeList.
    //    Потрібно коли аніме поточного сезону не є в search_results (напр. S4 доданий на anihub
    //    без has_ukrainian_dub=true): search_results не містить S4, тому details треба
    //    завантажити напряму за anime_id з studio_anime_ids.
    if app.mode == AppMode::Normal
        && matches!(
            app.focus,
            FocusPanel::SeasonList | FocusPanel::DubbingList | FocusPanel::EpisodeList
        )
        && app.current_details.is_none()
    {
        let season_anime_id = app.selected_season_num().and_then(|sn| {
            app.current_sources.as_ref().and_then(|sources| {
                sources
                    .ashdi
                    .iter()
                    .position(|s| s.season_number == sn)
                    .and_then(|idx| app.studio_anime_ids.get(idx))
                    .copied()
            })
        });
        if let Some(anime_id) = season_anime_id {
            if let Some(cached) = app.details_cache.get(&anime_id).cloned() {
                app.current_details = Some(cached);
            } else if let Ok(details) = app.api_client.get_anime_details(anime_id).await {
                app.details_cache.insert(anime_id, details.clone());
                app.current_details = Some(details);
            }
        }
    }
}

async fn load_library_combined_sources(
    app: &mut AppState,
    current_tv_ids: &[u32],
    representative_id: u32,
) {
    let mut tv_with_year: Vec<(u32, u32)> = Vec::new();
    let mut id_to_anilist: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();

    for &anime_id in current_tv_ids {
        let details = if let Some(cached) = app.details_cache.get(&anime_id).cloned() {
            Some(cached)
        } else if let Ok(details) = app.api_client.get_anime_details(anime_id).await {
            app.details_cache.insert(anime_id, details.clone());
            Some(details)
        } else {
            None
        };

        let year = details.as_ref().and_then(|d| d.year).unwrap_or(0);
        tv_with_year.push((anime_id, year));
        if let Some(al_id) = details.and_then(|d| d.anilist_id) {
            id_to_anilist.insert(anime_id, al_id);
        }
    }

    let extra_tv =
        augment_library_franchise_with_anilist(app, current_tv_ids, representative_id).await;
    for (id, year, al_id) in extra_tv {
        if !tv_with_year
            .iter()
            .any(|(existing_id, _)| *existing_id == id)
        {
            tv_with_year.push((id, year));
        }
        id_to_anilist.entry(id).or_insert(al_id);
    }

    tv_with_year.sort_by(|&(a_id, a_year), &(b_id, b_year)| {
        let a_al = id_to_anilist.get(&a_id).copied().unwrap_or(u32::MAX);
        let b_al = id_to_anilist.get(&b_id).copied().unwrap_or(u32::MAX);
        a_al.cmp(&b_al).then(a_year.cmp(&b_year))
    });

    let franchise_tv_ids: Vec<u32> = tv_with_year.into_iter().map(|(id, _)| id).collect();
    if franchise_tv_ids.is_empty() {
        return;
    }

    let multi = franchise_tv_ids.len() > 1;
    let mut combined: Vec<AshdiStudio> = Vec::new();
    let mut anime_ids: Vec<u32> = Vec::new();

    if multi {
        let mut join_set: tokio::task::JoinSet<(u32, Option<EpisodeSourcesResponse>)> =
            tokio::task::JoinSet::new();
        for &anime_id in &franchise_tv_ids {
            if let Some(cached) = app.sources_cache.get(&anime_id).cloned() {
                join_set.spawn(async move { (anime_id, Some(cached)) });
            } else {
                let client = app.api_client.clone();
                join_set.spawn(async move {
                    (
                        anime_id,
                        client.get_episode_sources_for_anime(anime_id).await.ok(),
                    )
                });
            }
        }

        let mut fetched: Vec<(u32, Option<EpisodeSourcesResponse>)> =
            Vec::with_capacity(franchise_tv_ids.len());
        while let Some(Ok(result)) = join_set.join_next().await {
            fetched.push(result);
        }
        fetched.sort_by_key(|(id, _)| {
            franchise_tv_ids
                .iter()
                .position(|&x| x == *id)
                .unwrap_or(usize::MAX)
        });

        let mut all: Vec<(u32, AshdiStudio)> = Vec::new();
        let mut per_member: std::collections::HashMap<u32, Vec<AshdiStudio>> =
            std::collections::HashMap::new();
        for (anime_id, sources) in fetched {
            if let Some(sources) = sources {
                app.sources_cache.insert(anime_id, sources.clone());
                per_member.insert(anime_id, sources.ashdi.clone());
                for studio in sources.ashdi {
                    all.push((anime_id, studio));
                }
            }
        }

        let active_franchise_ids: Vec<u32> = franchise_tv_ids
            .iter()
            .copied()
            .filter(|id| per_member.contains_key(id))
            .collect();
        let mut best: Vec<(u32, AshdiStudio)> = Vec::new();
        for (aid, studio) in &all {
            if let Some(pos) = best.iter().position(|(_, s)| {
                s.season_number == studio.season_number && s.studio_name == studio.studio_name
            }) {
                if studio.episodes.len() >= best[pos].1.episodes.len() {
                    best[pos] = (*aid, studio.clone());
                }
            } else {
                best.push((*aid, studio.clone()));
            }
        }
        best.sort_by(|(_, a), (_, b)| {
            a.season_number
                .cmp(&b.season_number)
                .then(b.episodes_count.cmp(&a.episodes_count))
        });
        let unique_season_nums: Vec<u32> = {
            let mut v: Vec<u32> = Vec::new();
            for (_, s) in &best {
                if !v.contains(&s.season_number) {
                    v.push(s.season_number);
                }
            }
            v
        };
        let n_seasons = unique_season_nums.len();
        let n_members = active_franchise_ids.len();

        if n_members >= 2 * n_seasons.max(1) {
            let mut season_counter = 1u32;
            let mut claimed_urls: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for &anime_id in &active_franchise_ids {
                let Some(member_studios) = per_member.get(&anime_id) else {
                    continue;
                };
                let mut season_nums: Vec<u32> =
                    member_studios.iter().map(|s| s.season_number).collect();
                season_nums.sort();
                season_nums.dedup();
                let chosen = season_nums
                    .iter()
                    .find(|&&s_num| {
                        member_studios
                            .iter()
                            .filter(|s| s.season_number == s_num)
                            .any(|s| {
                                s.episodes
                                    .first()
                                    .map(|e| !claimed_urls.contains(&e.url))
                                    .unwrap_or(false)
                            })
                    })
                    .copied();
                let Some(chosen_s) = chosen else { continue };
                for studio in member_studios
                    .iter()
                    .filter(|s| s.season_number == chosen_s)
                {
                    if let Some(ep) = studio.episodes.first() {
                        claimed_urls.insert(ep.url.clone());
                    }
                    combined.push(AshdiStudio {
                        season_number: season_counter,
                        ..studio.clone()
                    });
                    anime_ids.push(anime_id);
                }
                season_counter += 1;
            }
        } else {
            let member_offset = n_members.saturating_sub(n_seasons);
            for (data_aid, studio) in best {
                let season_pos = unique_season_nums
                    .iter()
                    .position(|&s| s == studio.season_number)
                    .unwrap_or(0);
                let new_season = (season_pos + 1) as u32;
                let owner_id = active_franchise_ids
                    .get(season_pos + member_offset)
                    .copied()
                    .unwrap_or_else(|| active_franchise_ids.last().copied().unwrap_or(data_aid));
                anime_ids.push(owner_id);
                combined.push(AshdiStudio {
                    season_number: new_season,
                    ..studio
                });
            }
        }
    } else {
        let anime_id = franchise_tv_ids[0];
        let sources = if let Some(cached) = app.sources_cache.get(&anime_id).cloned() {
            Some(cached)
        } else {
            app.api_client
                .get_episode_sources_for_anime(anime_id)
                .await
                .ok()
        };
        if let Some(sources) = sources {
            app.sources_cache.insert(anime_id, sources.clone());
            for studio in sources.ashdi {
                anime_ids.push(anime_id);
                combined.push(studio);
            }
        }
    }

    if !combined.is_empty() {
        app.studio_anime_ids = anime_ids;
        app.current_sources = Some(EpisodeSourcesResponse {
            ashdi: combined,
            moonanime: Vec::new(),
        });
    }
}

fn apply_search_results(app: &mut AppState, results: Vec<api::AnimeItem>) {
    app.search_results = results;
    app.franchise_groups = api::group_into_franchises(&app.search_results);
    app.search_query.clear();

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
        app.loading = true;
        app.pending_prefetch_ids = Some(app.search_results.iter().map(|anime| anime.id).collect());
    } else {
        app.result_list_state.select(None);
        app.selected_group_index = None;
        app.selected_result_index = None;
        app.set_info_status("Нічого не знайдено");
    }
}

fn spawn_prefetch_for_ids(app: &mut AppState, ids: Vec<u32>) {
    if let Some(abort) = app.preload_abort.take() {
        abort.abort();
    }

    let pending: Vec<u32> = ids
        .into_iter()
        .filter(|anime_id| !app.sources_cache.contains_key(anime_id))
        .collect();

    if pending.is_empty() {
        app.prefetch_rx = None;
        app.prefetching = false;
        return;
    }

    let client = app.api_client.clone();
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let task = tokio::spawn(async move {
        let mut join_set: JoinSet<(
            u32,
            Option<api::AnimeDetails>,
            Option<EpisodeSourcesResponse>,
        )> = JoinSet::new();
        for anime_id in pending {
            let client = client.clone();
            join_set.spawn(async move {
                let sources = client.get_episode_sources_for_anime(anime_id).await.ok();
                (anime_id, None, sources)
            });
        }

        while let Some(result) = join_set.join_next().await {
            let Ok((id, details, sources)) = result else {
                continue;
            };
            if tx.send((id, details, sources)).await.is_err() {
                break;
            }
        }
    });

    app.preload_abort = Some(task.abort_handle());
    app.prefetch_rx = Some(rx);
    app.prefetching = true;
}

/// Об'єднує джерела серій з усіх TV-елементів поточної франшизи.
/// Також збагачує через AniList (знаходить приховані члени з іншими назвами та фільми).
async fn load_combined_sources(app: &mut AppState, representative_id: u32) {
    // Будуємо початковий список TV-членів (id, year) з franchise_groups
    let mut tv_with_year: Vec<(u32, u32)> = if let Some(g_idx) = app.selected_group_index {
        if let Some(group) = app.franchise_groups.get(g_idx) {
            let mut tv: Vec<(u32, u32)> = group
                .iter()
                .map(|&i| &app.search_results[i])
                .filter(|a| a.anime_type.to_lowercase() == "tv")
                .map(|a| (a.id, a.year.unwrap_or(0)))
                .collect();
            // Сортуємо за anilist_id (відображає порядок виходу), fallback — рік
            // Рік дубляжу на anihub може бути пізніший оригінального, тому anilist_id надійніший
            tv.sort_by(|&(a_id, a_year), &(b_id, b_year)| {
                let a_al = app
                    .search_results
                    .iter()
                    .find(|a| a.id == a_id)
                    .and_then(|a| a.anilist_id)
                    .unwrap_or(u32::MAX);
                let b_al = app
                    .search_results
                    .iter()
                    .find(|a| a.id == b_id)
                    .and_then(|a| a.anilist_id)
                    .unwrap_or(u32::MAX);
                a_al.cmp(&b_al).then(a_year.cmp(&b_year))
            });
            tv
        } else {
            vec![(representative_id, 0)]
        }
    } else {
        vec![(representative_id, 0)]
    };

    // Збагачуємо через AniList лише якщо є хоча б один TV-член.
    // Якщо tv_with_year порожній (фільм/ova/спец) — не шукаємо, бо AniList поверне TV-сезони
    // і вони з'являться замість самого фільму.
    if !tv_with_year.is_empty() {
        let current_tv_ids: Vec<u32> = tv_with_year.iter().map(|(id, _)| *id).collect();
        // id_to_anilist: зберігаємо anilist_id для всіх TV-членів (включно з augmented),
        // щоб сортувати за ним (нижчий = раніший сезон) замість ненадійного year.
        let mut id_to_anilist: std::collections::HashMap<u32, u32> = app
            .search_results
            .iter()
            .filter_map(|a| a.anilist_id.map(|al| (a.id, al)))
            .collect();

        let extra_tv =
            augment_franchise_with_anilist(app, &current_tv_ids, representative_id).await;
        for (id, year, al_id) in extra_tv {
            if !tv_with_year.iter().any(|(i, _)| *i == id) {
                tv_with_year.push((id, year));
            }
            id_to_anilist.entry(id).or_insert(al_id);
        }
        tv_with_year.sort_by(|&(a_id, a_year), &(b_id, b_year)| {
            let a_al = id_to_anilist.get(&a_id).copied().unwrap_or(u32::MAX);
            let b_al = id_to_anilist.get(&b_id).copied().unwrap_or(u32::MAX);
            a_al.cmp(&b_al).then(a_year.cmp(&b_year))
        });

        // Видаляємо записи, де AniList підтверджує що це НЕ TV (наприклад, теізер PV з type=tv на anihub,
        // але format=ONA на AniList). Так запобігаємо зміщенню back-alignment через фальшиві TV-записи.
        if let Some(anilist_members) = app.anilist_cache.get(&representative_id).cloned() {
            tv_with_year.retain(|(id, _)| {
                // Використовуємо id_to_anilist (охоплює і augment-записи, не лише search_results)
                let anilist_id = id_to_anilist.get(id).copied();
                match anilist_id {
                    Some(al_id) => anilist_members
                        .iter()
                        .find(|m| m.anilist_id == al_id)
                        .map(|m| m.is_tv)
                        .unwrap_or(false), // відомий anilist_id НЕ знайдено в BFS → видаляємо (ONA/Movie teaser)
                    None => true, // немає anilist_id → залишаємо
                }
            });
        }
    }
    let franchise_tv_ids: Vec<u32> = tv_with_year.into_iter().map(|(id, _)| id).collect();

    let multi = franchise_tv_ids.len() > 1;
    let mut combined: Vec<AshdiStudio> = Vec::new();
    let mut anime_ids: Vec<u32> = Vec::new();

    if multi {
        // Паралельний запит для всіх franchise-членів (JoinSet).
        // Кожен anime_id → get_episode_sources_for_anime незалежно і одночасно.
        let mut join_set: tokio::task::JoinSet<(u32, Option<EpisodeSourcesResponse>)> =
            tokio::task::JoinSet::new();
        for &anime_id in &franchise_tv_ids {
            if let Some(cached) = app.sources_cache.get(&anime_id).cloned() {
                join_set.spawn(async move { (anime_id, Some(cached)) });
            } else {
                let client = app.api_client.clone();
                join_set.spawn(async move {
                    (
                        anime_id,
                        client.get_episode_sources_for_anime(anime_id).await.ok(),
                    )
                });
            }
        }
        let mut fetched: Vec<(u32, Option<EpisodeSourcesResponse>)> =
            Vec::with_capacity(franchise_tv_ids.len());
        while let Some(Ok(result)) = join_set.join_next().await {
            fetched.push(result);
        }
        // Відновлюємо порядок franchise_tv_ids (JoinSet не гарантує порядок)
        fetched.sort_by_key(|(id, _)| {
            franchise_tv_ids
                .iter()
                .position(|&x| x == *id)
                .unwrap_or(usize::MAX)
        });
        let mut all: Vec<(u32, AshdiStudio)> = Vec::new();
        let mut per_member: std::collections::HashMap<u32, Vec<AshdiStudio>> =
            std::collections::HashMap::new();
        for (anime_id, sources) in fetched {
            if let Some(sources) = sources {
                per_member.insert(anime_id, sources.ashdi.clone());
                for studio in sources.ashdi {
                    all.push((anime_id, studio));
                }
            }
        }
        // Лише члени з реальними даними Ashdi — щоб AniList-доповнення без даних
        // не зміщували back-alignment offset і не впливали на n_members.
        let active_franchise_ids: Vec<u32> = franchise_tv_ids
            .iter()
            .copied()
            .filter(|id| per_member.contains_key(id))
            .collect();

        // Dedup по (season_number, studio_name); зберігаємо entry з найбільшим episodes.len()
        let mut best: Vec<(u32, AshdiStudio)> = Vec::new();
        for (aid, studio) in &all {
            if let Some(pos) = best.iter().position(|(_, s)| {
                s.season_number == studio.season_number && s.studio_name == studio.studio_name
            }) {
                if studio.episodes.len() >= best[pos].1.episodes.len() {
                    best[pos] = (*aid, studio.clone());
                }
            } else {
                best.push((*aid, studio.clone()));
            }
        }
        best.sort_by(|(_, a), (_, b)| {
            a.season_number
                .cmp(&b.season_number)
                .then(b.episodes_count.cmp(&a.episodes_count))
        });
        let unique = best;
        let unique_season_nums: Vec<u32> = {
            let mut v: Vec<u32> = Vec::new();
            for (_, s) in &unique {
                if !v.contains(&s.season_number) {
                    v.push(s.season_number);
                }
            }
            v
        };
        let n_seasons = unique_season_nums.len();
        let n_members = active_franchise_ids.len();

        if n_members >= 2 * n_seasons.max(1) {
            // Per-member режим: кожен anime_id отримує свій sequential сезон-слот.
            // Ashdi крос-лінкує старі сезони в нові члени → виявляємо дублікати по URL першого
            // епізоду: якщо всі сезони члена вже claimed попередніми членами — пропускаємо.
            let mut season_counter = 1u32;
            let mut claimed_urls: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for &anime_id in &active_franchise_ids {
                let member_studios = per_member.get(&anime_id).unwrap(); // safe: відфільтровано вище
                // Сезони в порядку зростання
                let mut season_nums: Vec<u32> =
                    member_studios.iter().map(|s| s.season_number).collect();
                season_nums.sort();
                season_nums.dedup();
                // Шукаємо перший сезон, де є хоч одна студія з непривласненим URL першого епізоду
                let chosen = season_nums
                    .iter()
                    .find(|&&s_num| {
                        member_studios
                            .iter()
                            .filter(|s| s.season_number == s_num)
                            .any(|s| {
                                s.episodes
                                    .first()
                                    .map(|e| !claimed_urls.contains(&e.url))
                                    .unwrap_or(false)
                            })
                    })
                    .copied();
                let Some(chosen_s) = chosen else { continue }; // всі сезони — кросс-лінк → пропускаємо
                for studio in member_studios
                    .iter()
                    .filter(|s| s.season_number == chosen_s)
                {
                    if let Some(ep) = studio.episodes.first() {
                        claimed_urls.insert(ep.url.clone());
                    }
                    combined.push(AshdiStudio {
                        season_number: season_counter,
                        ..studio.clone()
                    });
                    anime_ids.push(anime_id);
                }
                season_counter += 1;
            }
        } else {
            // Back-align: якщо franchise-членів більше ніж сезонів Ashdi,
            // вирівнюємо з кінця: SN → active_members[offset + season_pos].
            let member_offset = n_members.saturating_sub(n_seasons);
            for (data_aid, studio) in unique {
                let season_pos = unique_season_nums
                    .iter()
                    .position(|&s| s == studio.season_number)
                    .unwrap_or(0);
                let new_season = (season_pos + 1) as u32;
                let owner_id = active_franchise_ids
                    .get(season_pos + member_offset)
                    .copied()
                    .unwrap_or_else(|| active_franchise_ids.last().copied().unwrap_or(data_aid));
                anime_ids.push(owner_id);
                combined.push(AshdiStudio {
                    season_number: new_season,
                    ..studio
                });
            }
        }
        // DEBUG → /tmp/anihub_debug.log
        debug_log(&format!("[DEBUG] representative_id={representative_id}"));
        debug_log("[DEBUG] active_franchise_ids (впорядковані):");
        for &id in &active_franchise_ids {
            let title = app
                .search_results
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.title_ukrainian.as_str())
                .unwrap_or("?");
            let al = app
                .search_results
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| a.anilist_id)
                .or_else(|| app.details_cache.get(&id).and_then(|d| d.anilist_id));
            debug_log(&format!(
                "[DEBUG]   id={id}  anilist_id={al:?}  title={title:?}"
            ));
        }
        debug_log(&format!(
            "[DEBUG] n_members={n_members}  n_seasons={n_seasons}  mode={}",
            if n_members >= 2 * n_seasons.max(1) {
                "per-member"
            } else {
                "back-align"
            }
        ));
        for (i, (s, id)) in combined.iter().zip(anime_ids.iter()).enumerate() {
            let first_ep = s
                .episodes
                .first()
                .map(|e| (e.episode_number, e.url.chars().take(60).collect::<String>()))
                .unwrap_or((0, String::new()));
            debug_log(&format!(
                "[DEBUG] combined[{i}] season={} studio={:?} ep={} owner={id}  first_ep#{}  url_prefix={:?}",
                s.season_number, s.studio_name, s.episodes_count, first_ep.0, first_ep.1
            ));
        }
    } else {
        if franchise_tv_ids.is_empty() {
            // Fallback: всі члени групи — OVA/movie/спец → використовуємо representative
            match app
                .api_client
                .get_episode_sources_for_anime(representative_id)
                .await
            {
                Ok(sources) => {
                    app.studio_anime_ids = vec![representative_id; sources.ashdi.len()];
                    app.current_sources = Some(sources);
                }
                Err(e) => app.set_error_status(format!("Помилка джерел: {}", e)),
            }
            return;
        }
        let anime_id = franchise_tv_ids[0];
        let sources = if let Some(cached) = app.sources_cache.get(&anime_id).cloned() {
            Some(cached)
        } else {
            app.api_client
                .get_episode_sources_for_anime(anime_id)
                .await
                .ok()
        };
        if let Some(sources) = sources {
            for studio in sources.ashdi {
                anime_ids.push(anime_id);
                combined.push(studio);
            }
        }
    }

    if !combined.is_empty() {
        app.studio_anime_ids = anime_ids;
        app.current_sources = Some(EpisodeSourcesResponse {
            ashdi: combined,
            moonanime: Vec::new(),
        });
    } else {
        match app
            .api_client
            .get_episode_sources_for_anime(representative_id)
            .await
        {
            Ok(sources) => {
                app.studio_anime_ids = vec![representative_id; sources.ashdi.len()];
                app.current_sources = Some(sources);
            }
            Err(e) => app.set_error_status(format!("Помилка джерел: {}", e)),
        }
    }
}

/// Збагачує список TV-членів через AniList: знаходить нові TV IDs з іншими назвами.
/// Повертає extra_tv: Vec<(anihub_id, year, anilist_id)>.
async fn augment_franchise_with_anilist(
    app: &mut AppState,
    current_tv_ids: &[u32],
    representative_id: u32,
) -> Vec<(u32, u32, u32)> {
    // Якщо є відомий anilist_id у першого TV-члена — використовуємо ID-based BFS,
    // він надійніший: обходить граф починаючи з точного запису, а не пошуку за назвою.
    let first_known_al_id = current_tv_ids
        .iter()
        .filter_map(|&id| {
            app.search_results
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| a.anilist_id)
        })
        .next();

    let anilist_members = if let Some(cached) = app.anilist_cache.get(&representative_id) {
        cached.clone()
    } else {
        let members = if let Some(al_id) = first_known_al_id {
            api::anilist::get_franchise_members_by_id(app.api_client.http_client(), al_id).await
        } else {
            let title_original = app
                .search_results
                .iter()
                .find(|a| a.id == representative_id)
                .and_then(|a| a.title_original.clone())
                .unwrap_or_default();
            if title_original.is_empty() {
                return Vec::new();
            }
            api::anilist::get_franchise_members(app.api_client.http_client(), &title_original).await
        };
        if !members.is_empty() {
            app.anilist_cache.insert(representative_id, members.clone());
        }
        members
    };

    if anilist_members.is_empty() {
        return Vec::new();
    }

    let mut extra_tv: Vec<(u32, u32, u32)> = Vec::new();

    for member in &anilist_members {
        if !member.is_tv {
            continue; // лише справжні TV-серіали; фільми/ONA — окремі записи
        }
        let anihub_id = match find_anihub_id(&app.search_results, &app.api_client, member).await {
            Some(id) => id,
            None => continue,
        };
        if current_tv_ids.contains(&anihub_id) {
            continue;
        }
        let year = app
            .search_results
            .iter()
            .find(|a| a.id == anihub_id)
            .and_then(|a| a.year)
            .or_else(|| app.details_cache.get(&anihub_id).and_then(|d| d.year))
            .unwrap_or(9999);
        if !extra_tv.iter().any(|(id, _, _)| *id == anihub_id) {
            extra_tv.push((anihub_id, year, member.anilist_id));
        }
    }

    extra_tv
}

async fn augment_library_franchise_with_anilist(
    app: &mut AppState,
    current_tv_ids: &[u32],
    representative_id: u32,
) -> Vec<(u32, u32, u32)> {
    let mut first_known_al_id: Option<u32> = None;
    let mut fallback_title_original: Option<String> = None;

    for &id in current_tv_ids {
        let details = if let Some(cached) = app.details_cache.get(&id).cloned() {
            Some(cached)
        } else if let Ok(details) = app.api_client.get_anime_details(id).await {
            app.details_cache.insert(id, details.clone());
            Some(details)
        } else {
            None
        };

        if first_known_al_id.is_none() {
            first_known_al_id = details.as_ref().and_then(|d| d.anilist_id);
        }
        if fallback_title_original.is_none() {
            fallback_title_original = details.as_ref().and_then(|d| d.title_original.clone());
        }
    }

    let anilist_members = if let Some(cached) = app.anilist_cache.get(&representative_id) {
        cached.clone()
    } else {
        let members = if let Some(al_id) = first_known_al_id {
            api::anilist::get_franchise_members_by_id(app.api_client.http_client(), al_id).await
        } else if let Some(title_original) = fallback_title_original {
            api::anilist::get_franchise_members(app.api_client.http_client(), &title_original).await
        } else {
            Vec::new()
        };
        if !members.is_empty() {
            app.anilist_cache.insert(representative_id, members.clone());
        }
        members
    };

    if anilist_members.is_empty() {
        return Vec::new();
    }

    let known_ids = current_tv_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let mut extra_tv: Vec<(u32, u32, u32)> = Vec::new();

    for member in &anilist_members {
        if !member.is_tv {
            continue;
        }
        if let Ok(Some(anime_id)) = app
            .api_client
            .get_anime_by_anilist_id(member.anilist_id)
            .await
        {
            if known_ids.contains(&anime_id) || extra_tv.iter().any(|(id, _, _)| *id == anime_id) {
                continue;
            }
            extra_tv.push((anime_id, 0, member.anilist_id));
        }
    }

    extra_tv
}

/// Шукає anihub ID для AniList-члена франшизи.
/// Стратегія (у порядку пріоритету):
///   1. Точний збіг anilist_id у search_results (без HTTP)
///   2. Прямий API-запит ?anilist_id=X (надійний)
/// Title-matching навмисно прибрано: може дати хибний збіг (підрядок у тизері тощо).
async fn find_anihub_id(
    search_results: &[api::AnimeItem],
    client: &api::ApiClient,
    member: &api::anilist::FranchiseMember,
) -> Option<u32> {
    // 1. Точний anilist_id серед вже завантажених результатів
    for item in search_results {
        if item.anilist_id == Some(member.anilist_id) {
            return Some(item.id);
        }
    }
    // 2. Прямий API-запит за anilist_id
    client
        .get_anime_by_anilist_id(member.anilist_id)
        .await
        .ok()
        .flatten()
}

/// Намагається витягнути URL потоку з moonanime через yt-dlp з кукі браузера.
/// Перебирає Firefox, Chrome, Chromium. Повертає None якщо не вдалось.
async fn try_moonanime_stream(iframe_url: &str) -> Option<String> {
    for browser in &["firefox", "chrome", "chromium"] {
        let Ok(output) = tokio::process::Command::new("yt-dlp")
            .args([
                "--cookies-from-browser",
                browser,
                "--get-url",
                "--no-playlist",
                "--quiet",
                iframe_url,
            ])
            .output()
            .await
        else {
            continue;
        };

        if output.status.success() {
            let raw = String::from_utf8_lossy(&output.stdout);
            // yt-dlp може повернути декілька рядків — беремо перший непорожній
            let url = raw.lines().find(|l| !l.trim().is_empty())?.trim();
            if !url.is_empty() {
                return Some(url.to_string());
            }
        }
    }
    None
}

/// Повертає id першого (найстаршого за роком) TV-члена поточної франшизи.
fn get_first_tv_id(app: &ui::AppState) -> Option<u32> {
    let g_idx = app.selected_group_index?;
    let group = app.franchise_groups.get(g_idx)?;
    let mut tv: Vec<(u32, u32)> = group
        .iter()
        .map(|&i| &app.search_results[i])
        .filter(|a| {
            let t = a.anime_type.to_lowercase();
            !t.contains("ova")
                && !t.contains("ona")
                && !t.contains("фільм")
                && !t.contains("film")
                && !t.contains("спец")
                && !t.contains("special")
                && !t.contains("movie")
                && !t.contains("short")
        })
        .map(|a| (a.id, a.year.unwrap_or(0)))
        .collect();
    tv.sort_by_key(|&(_, y)| y);
    tv.first().map(|(id, _)| *id)
}

fn get_next_episode(app: &AppState, current: &(u32, String, u32, u32, String)) -> Option<PlayTarget> {
    let (current_anime_id, current_title, current_season, current_episode, current_studio) = current;
    
    let sources = app.sources_cache.get(current_anime_id).or_else(|| app.current_sources.as_ref())?;
    
    // Check if the current studio data is present
    let mut seasons: Vec<u32> = sources.ashdi.iter().map(|s| s.season_number).collect();
    seasons.sort();
    seasons.dedup();
    
    let season_index = seasons.iter().position(|&s| s == *current_season)?;
    
    // Find the studio index for current
    let (studio_idx, studio_data) = sources.ashdi.iter().enumerate()
        .filter(|(_, s)| s.season_number == *current_season)
        .find(|(_, s)| s.studio_name == *current_studio)?;
        
    let ep_index = studio_data.episodes.iter().position(|e| e.episode_number == *current_episode)?;
    
    if let Some(next_ep) = studio_data.episodes.get(ep_index + 1) {
        let anime_id = app.studio_anime_ids.get(studio_idx).copied().unwrap_or(*current_anime_id);
        let title = app.details_cache.get(&anime_id).map(|d| d.title_ukrainian.clone())
            .unwrap_or_else(|| current_title.clone());
        let player_title = app.details_cache.get(&anime_id)
            .map(|d| format!("{} ({})", d.title_ukrainian, d.year.unwrap_or(0)))
            .unwrap_or_else(|| title.clone());
            
        return Some(PlayTarget {
            anime_id,
            anime_title: title,
            player_title,
            season: *current_season,
            episode: next_ep.episode_number,
            episode_title: format!("Серія {}", next_ep.episode_number),
            stream_page_url: next_ep.url.clone(),
            start_time: None,
            studio_name: current_studio.clone(),
        });
    }
    
    // Next season
    let next_season = seasons.get(season_index + 1).copied()?;
    let (next_studio_idx, next_studio_data) = sources.ashdi.iter().enumerate()
        .find(|(_, s)| s.season_number == next_season)?;
    let next_ep = next_studio_data.episodes.first()?;
    
    let anime_id = app.studio_anime_ids.get(next_studio_idx).copied().unwrap_or(*current_anime_id);
    let title = app.details_cache.get(&anime_id).map(|d| d.title_ukrainian.clone())
        .unwrap_or_else(|| current_title.clone());
    let player_title = app.details_cache.get(&anime_id)
        .map(|d| format!("{} ({})", d.title_ukrainian, d.year.unwrap_or(0)))
        .unwrap_or_else(|| title.clone());
        
    Some(PlayTarget {
        anime_id,
        anime_title: title,
        player_title,
        season: next_season,
        episode: next_ep.episode_number,
        episode_title: format!("Серія {}", next_ep.episode_number),
        stream_page_url: next_ep.url.clone(),
        start_time: None,
        studio_name: next_studio_data.studio_name.clone(),
    })
}
