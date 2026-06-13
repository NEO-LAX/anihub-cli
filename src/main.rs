mod api;
mod player;
mod prefetch_bg;
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
    /// HTTP Referer для mpv — "https://ashdi.vip/" або "https://moonanime.art/"
    referrer: String,
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

        // Drain AniList prefetch results → anilist_cache
        if let Some(rx) = &mut app.anilist_prefetch_rx {
            loop {
                match rx.try_recv() {
                    Ok((rep_id, members)) => {
                        app.anilist_cache.insert(rep_id, members);
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        app.anilist_prefetch_rx = None;
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


        if let Some(rx) = &mut app.combined_sources_rx {
            match rx.try_recv() {
                Ok(Some((sources, ids))) => {
                    app.current_sources = Some(sources);
                    app.studio_anime_ids = ids;
                    app.loading = false;
                    app.combined_sources_rx = None;
                }
                Ok(None) => {
                    app.loading = false;
                    app.combined_sources_rx = None;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    // Still loading
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    app.loading = false;
                    app.combined_sources_rx = None;
                }
            }
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
                                if let Ok(new_history) = app.storage.update_progress(
                                    *anime_id,
                                    title,
                                    *season,
                                    *episode,
                                    studio_name,
                                    app.mpv_last_time,
                                    app.mpv_last_duration,
                                ) {
                                    app.history = new_history;
                                    app.rebuild_history_indexes();
                                }
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
                                        // MoonAnime: preload пропускаємо — proxy не можна передати з tokio::spawn
                                        // Наступна серія запуститься через start_episode_playback при EndFile
                                        if !next_target.stream_page_url.starts_with("https://moonanime.art") {
                                            tokio::spawn(async move {
                                                let m3u8 = if let Ok(parser) = api::AshdiParser::new() {
                                                    parser.extract_m3u8(&next_target.stream_page_url).await.ok()
                                                } else {
                                                    None
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
        if let Some(img) = app.poster_cache.get(&anime_id) {
            app.current_poster = Some(app.picker.new_resize_protocol((*img).clone()));
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
            app.poster_cache.insert(anime_id, std::sync::Arc::new(img));
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
    let studio_idx = app.current_sources.clone().and_then(|sources| {
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
    let is_moonanime = target_url.starts_with("https://moonanime.art");
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
        referrer: if is_moonanime {
            "https://moonanime.art/".to_string()
        } else {
            "https://ashdi.vip/".to_string()
        },
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
    let is_moonanime = resolved.url.starts_with("https://moonanime.art");
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
        referrer: if is_moonanime {
            "https://moonanime.art/".to_string()
        } else {
            "https://ashdi.vip/".to_string()
        },
    };

    play_target(app, target).await;
}

async fn play_target(app: &mut AppState, target: PlayTarget) {
    // Якщо щось уже грає — зберігаємо прогрес поточної серії
    if app.is_playing {
        if let Some((anime_id, title, season, episode, studio_name)) = &app.pending_progress {
            if let Ok(new_history) = app.storage.update_progress(
                *anime_id,
                title,
                *season,
                *episode,
                studio_name,
                app.mpv_last_time,
                app.mpv_last_duration,
            ) {
                app.history = new_history;
                app.rebuild_history_indexes();
            }
        }
    }

    let m3u8 = if target.stream_page_url.starts_with("https://moonanime.art") {
        // Вбиваємо попередній proxy якщо є
        if let Some(mut old) = app.moonanime_proxy.take() {
            let _ = old.kill().await;
        }
        match try_moonanime_stream(&target.stream_page_url).await {
            Some((url, proxy_child)) => {
                app.moonanime_proxy = Some(proxy_child);
                url
            }
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
                // MoonAnime: preload пропускаємо — proxy не можна передати з tokio::spawn
                if !next_target.stream_page_url.starts_with("https://moonanime.art") {
                    let player2 = app.mpv_player.clone();
                    tokio::spawn(async move {
                        let m3u8 = if let Ok(parser) = api::AshdiParser::new() {
                            parser.extract_m3u8(&next_target.stream_page_url).await.ok()
                        } else {
                            None
                        };
                        if let Some(url) = m3u8 {
                            let _ = player2.send_command(serde_json::json!(["loadfile", url, "append"])).await;
                        }
                    });
                }
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
            &target.referrer,
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
                // MoonAnime: preload пропускаємо — proxy не можна передати з tokio::spawn
                if !next_target.stream_page_url.starts_with("https://moonanime.art") {
                    let player2 = app.mpv_player.clone();
                    tokio::spawn(async move {
                        let m3u8 = if let Ok(parser) = api::AshdiParser::new() {
                            parser.extract_m3u8(&next_target.stream_page_url).await.ok()
                        } else {
                            None
                        };
                        if let Some(url) = m3u8 {
                            let _ = player2.send_command(serde_json::json!(["loadfile", url, "append"])).await;
                        }
                    });
                }
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
    if let Some(details) = app.details_cache.get(&anime_id) {
        return Some(details);
    }
    let details = app.api_client.get_anime_details(anime_id).await.ok()?;
    app.details_cache.insert(anime_id, details.clone());
    Some(details)
}

async fn get_or_fetch_sources(app: &mut AppState, anime_id: u32) -> Option<EpisodeSourcesResponse> {
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
            if let Ok(new_history) = app.storage.update_progress(
                anime_id,
                &title,
                season,
                episode,
                &studio_name,
                stopped_time,
                duration,
            ) {
                app.history = new_history;
                app.rebuild_history_indexes();
            }
        }
        app.is_playing = false;
        app.mpv_player.cleanup();
        // Зупиняємо MoonAnime proxy якщо він ще живий
        if let Some(mut proxy) = app.moonanime_proxy.take() {
            let _ = proxy.kill().await;
        }
    }
}

async fn handle_loading_tasks(app: &mut AppState) {
    app.clear_status();

    // 1. Пошук
    if app.mode == AppMode::Normal && !app.search_query.is_empty() {
        let query = app.search_query.trim().to_string();
        let cache_key = query.to_lowercase();
        let cached = app.search_cache.get(&cache_key);

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
                // Перевіряємо combined_sources_cache перед повним перерахунком
                if let Some((cached_sources, cached_ids)) =
                    app.combined_sources_cache.get(&representative_id)
                {
                    app.current_sources = Some(cached_sources);
                    app.studio_anime_ids = cached_ids;
                } else {
                    app.loading = true;
                    app.current_sources = None; // Reset sources while loading
                    
                    let api_client = app.api_client.clone();
                    let details_cache = app.details_cache.clone();
                    let sources_cache = app.sources_cache.clone();
                    let anilist_cache = app.anilist_cache.clone();
                    let combined_sources_cache = app.combined_sources_cache.clone();
                    let selected_ids = selected_ids.clone();
                    
                    let (tx, rx) = tokio::sync::mpsc::channel(1);
                    app.combined_sources_rx = Some(rx);
                    
                    tokio::spawn(async move {
                        let res = crate::prefetch_bg::compute_library_combined_sources(
                            api_client,
                            details_cache,
                            sources_cache,
                            anilist_cache,
                            selected_ids,
                            representative_id,
                        ).await;
                        if let Some((sources, ids)) = res.clone() {
                            combined_sources_cache.insert(representative_id, (sources.clone(), ids.clone()));
                        }
                        let _ = tx.send(res).await;
                    });
                }
            }
        }

        if let Some(context_anime_id) = app.library_selected_anime_id() {
            if app.current_details.as_ref().map(|details| details.id) != Some(context_anime_id) {
                if let Some(cached) = app.details_cache.get(&context_anime_id) {
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
                    if let Some(cached) = app.details_cache.get(&item.id) {
                        app.current_details = Some(cached);
                    } else if let Ok(details) = app.api_client.get_anime_details(item.id).await {
                        app.current_details = Some(details);
                    }
                }

                // Об'єднані джерела з усіх TV-членів франшизи
                if app.current_sources.is_none() {
                    if let Some((cached_sources, cached_ids)) =
                        app.combined_sources_cache.get(&item.id)
                    {
                        app.current_sources = Some(cached_sources);
                        app.studio_anime_ids = cached_ids;
                    } else {
                        app.loading = true;
                        app.current_sources = None; // Reset sources while loading
                        
                        let api_client = app.api_client.clone();
                        let details_cache = app.details_cache.clone();
                        let sources_cache = app.sources_cache.clone();
                        let anilist_cache = app.anilist_cache.clone();
                        let combined_sources_cache = app.combined_sources_cache.clone();
                        
                        let representative_id = item.id;
                        let mut current_tv_ids = Vec::new();
                        if let Some(g_idx) = app.selected_group_index {
                            if let Some(group) = app.franchise_groups.get(g_idx) {
                                for &i in group {
                                    let a = &app.search_results[i];
                                    if a.anime_type.to_lowercase() == "tv" {
                                        current_tv_ids.push(a.id);
                                    }
                                }
                            }
                        }
                        if current_tv_ids.is_empty() {
                            current_tv_ids.push(representative_id);
                        }
                        
                        let (tx, rx) = tokio::sync::mpsc::channel(1);
                        app.combined_sources_rx = Some(rx);
                        
                        tokio::spawn(async move {
                            let res = crate::prefetch_bg::compute_library_combined_sources(
                                api_client,
                                details_cache,
                                sources_cache,
                                anilist_cache,
                                current_tv_ids,
                                representative_id,
                            ).await;
                            if let Some((sources, ids)) = res.clone() {
                                combined_sources_cache.insert(representative_id, (sources.clone(), ids.clone()));
                            }
                            let _ = tx.send(res).await;
                        });
                    }
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
            app.current_sources.clone().and_then(|sources| {
                sources
                    .ashdi
                    .iter()
                    .position(|s| s.season_number == sn)
                    .and_then(|idx| app.studio_anime_ids.get(idx))
                    .copied()
            })
        });
        if let Some(anime_id) = season_anime_id {
            if let Some(cached) = app.details_cache.get(&anime_id) {
                app.current_details = Some(cached);
            } else if let Ok(details) = app.api_client.get_anime_details(anime_id).await {
                app.details_cache.insert(anime_id, details.clone());
                app.current_details = Some(details);
            }
        }
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

        // AniList prefetch: для кожної групи де є TV-члени з anilist_id
        let anilist_groups: Vec<(u32, u32)> = app
            .franchise_groups
            .iter()
            .filter_map(|group| {
                let rep_idx = api::representative_idx(&app.search_results, group);
                let rep_id = app.search_results[rep_idx].id;
                // Перший TV-член з anilist_id
                group.iter().find_map(|&i| {
                    let item = &app.search_results[i];
                    if item.anime_type.to_lowercase() == "tv" {
                        item.anilist_id.map(|al_id| (rep_id, al_id))
                    } else {
                        None
                    }
                })
            })
            .collect();
        if !anilist_groups.is_empty() {
            spawn_super_prefetch(app);
        }
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

    // Завантажуємо тільки ті, яких ще немає в кешах
    let pending_sources: Vec<u32> = ids
        .iter()
        .copied()
        .filter(|anime_id| !app.sources_cache.contains_key(anime_id))
        .collect();
    let pending_details: Vec<u32> = ids
        .into_iter()
        .filter(|anime_id| !app.details_cache.contains_key(anime_id))
        .collect();

    if pending_sources.is_empty() && pending_details.is_empty() {
        app.prefetch_rx = None;
        app.prefetching = false;
        return;
    }

    let client = app.api_client.clone();
    let (tx, rx) = tokio::sync::mpsc::channel(128);
    let task = tokio::spawn(async move {
        let mut join_set: JoinSet<(
            u32,
            Option<api::AnimeDetails>,
            Option<EpisodeSourcesResponse>,
        )> = JoinSet::new();

        // Sources prefetch
        for anime_id in pending_sources {
            let c = client.clone();
            join_set.spawn(async move {
                let sources = c.get_episode_sources_for_anime(anime_id).await.ok();
                (anime_id, None, sources)
            });
        }
        // Details prefetch
        for anime_id in pending_details {
            let c = client.clone();
            join_set.spawn(async move {
                let details = c.get_anime_details(anime_id).await.ok();
                (anime_id, details, None)
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

/// Запускає фоновий prefetch AniList-даних для кожної групи.
/// Результат (representative_id, members) надходить через `anilist_prefetch_rx`.

fn spawn_super_prefetch(app: &mut AppState) {
    let mut tasks = Vec::new();
    for group in &app.franchise_groups {
        if group.is_empty() { continue; }
        let rep_idx = api::representative_idx(&app.search_results, group);
        let rep_id = app.search_results[rep_idx].id;
        if app.combined_sources_cache.contains_key(&rep_id) { continue; }

        let mut tv_ids = Vec::new();
        for &i in group {
            let item = &app.search_results[i];
            if item.anime_type.to_lowercase() == "tv" {
                tv_ids.push(item.id);
            }
        }
        if tv_ids.is_empty() {
            tv_ids.push(rep_id);
        }
        tasks.push((rep_id, tv_ids));
    }

    if tasks.is_empty() {
        return;
    }

    let api_client = app.api_client.clone();
    let details_cache = app.details_cache.clone();
    let sources_cache = app.sources_cache.clone();
    let anilist_cache = app.anilist_cache.clone();
    let combined_sources_cache = app.combined_sources_cache.clone();

    tokio::spawn(async move {
        // Ми можемо обробляти їх паралельно з JoinSet, але щоб не спамити API,
        // можна лімітувати concurrency через Semaphore.
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
        let mut join_set = tokio::task::JoinSet::new();

        for (rep_id, tv_ids) in tasks {
            let api_client = api_client.clone();
            let details_cache = details_cache.clone();
            let sources_cache = sources_cache.clone();
            let anilist_cache = anilist_cache.clone();
            let combined_sources_cache = combined_sources_cache.clone();
            let permit = semaphore.clone().acquire_owned().await.unwrap();

            join_set.spawn(async move {
                let _permit = permit;
                if let Some((sources, ids)) = crate::prefetch_bg::compute_library_combined_sources(
                    api_client,
                    details_cache,
                    sources_cache,
                    anilist_cache,
                    tv_ids,
                    rep_id,
                ).await {
                    combined_sources_cache.insert(rep_id, (sources, ids));
                }
            });
        }
        while let Some(_) = join_set.join_next().await {}
    });
}



/// Python-скрипт для Playwright: відкриває iframe сторінку headless Firefox,
/// перехоплює перший мережевий запит до `s.moonanime.art/*.m3u8` і виводить URL у stdout.
/// m3u8 URL формується обфускованим JS на стороні клієнта — статичний парсинг HTML не працює.
/// Playwright скрипт для MoonAnime:
/// 1. Firefox headless завантажує iframe сторінку (обходить TLS fingerprint CDN s.moonanime.art)
/// 2. Перехоплює master m3u8 URL (генерується obfuscated JS)
/// 3. Завантажує master manifest через JS fetch (browser контекст)
/// 4. Знаходить найкращу якість (1080p → 720p → перша)
/// 5. Завантажує variant manifest через JS fetch
/// 6. Зберігає у /tmp/anihub_moon_stream.m3u8 (segments — прямі s3.mooncdn.space URLs, доступні без proxy)
/// 7. Виводить шлях до файлу в stdout → mpv грає локальний manifest
/// Playwright скрипт для MoonAnime:
/// 1. Firefox headless завантажує iframe (обходить TLS fingerprint CDN s.moonanime.art)
/// 2. Перехоплює master m3u8 URL, завантажує manifest через JS fetch
/// 3. Знаходить найкращу якість, завантажує variant manifest
/// 4. Запускає локальний HTTP сервер (Content-Type: application/vnd.apple.mpegurl)
///    щоб mpv правильно розпізнав HLS (без HTTP → трактує як M3U плейлист)
/// 5. Виводить http://127.0.0.1:PORT/stream.m3u8 → Rust передає mpv
/// 6. Завершується коли Rust закриває stdin (mpv завершив відтворення)
const MOONANIME_PLAYWRIGHT_SCRIPT: &str = r#"
import asyncio, sys, re, socket
from http.server import HTTPServer, BaseHTTPRequestHandler
from threading import Thread

_manifest = b""

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/vnd.apple.mpegurl")
        self.send_header("Content-Length", str(len(_manifest)))
        self.end_headers()
        self.wfile.write(_manifest)
    def log_message(self, *a): pass

async def js_fetch(page, url):
    return await page.evaluate("""async (url) => {
        const r = await fetch(url, {
            headers: {"Origin": "https://moonanime.art", "Referer": "https://moonanime.art/"}
        });
        return {status: r.status, text: await r.text()};
    }""", url)

async def main():
    global _manifest
    from playwright.async_api import async_playwright
    iframe_url = sys.argv[1]

    # Один браузерний сеанс: завантажуємо iframe, ловимо media URL через event.
    # Підтримуємо два формати:
    #   HLS:  s.moonanime.art/...m3u8  — завантажуємо variant manifest, роздаємо через HTTP proxy
    #   WebM: s.moonanime.art/content/v/...  — отримуємо redirect URL (s1.mooncdn.space),
    #         який mpv може грати напряму без proxy
    async with async_playwright() as p:
        browser = await p.firefox.launch(headless=True)
        ctx = await browser.new_context(
            user_agent="Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0",
            extra_http_headers={
                "Accept-Language": "uk,en-US;q=0.9,en;q=0.8",
                "Accept": "text/html,application/xhtml+xml,*/*;q=0.8",
            },
        )
        page = await ctx.new_page()
        master_url = None
        webm_url = None
        async def on_req(req):
            nonlocal master_url, webm_url
            if "s.moonanime" in req.url and ".m3u8" in req.url and not master_url:
                master_url = req.url
            elif "s.moonanime" in req.url and "/content/v/" in req.url and not webm_url:
                webm_url = req.url
        page.on("request", on_req)
        try:
            await page.goto(iframe_url, wait_until="networkidle", timeout=20000)
            await asyncio.sleep(2)
            # WebM-плеєр потребує кліку для старту відтворення
            if not master_url:
                try:
                    await page.click("body")
                except Exception:
                    pass
                await asyncio.sleep(3)
        except Exception:
            pass

        if master_url:
            # --- HLS шлях: завантажуємо variant manifest і роздаємо через HTTP proxy ---
            master = await js_fetch(page, master_url)
            if master["status"] != 200:
                await browser.close()
                sys.exit(1)

            variants = re.findall(r'(https://s\.moonanime\.art/[^\s]+\.m3u8[^\s]*)', master["text"])
            if not variants:
                await browser.close()
                sys.exit(1)
            best = next((v for v in variants if "1080" in v),
                   next((v for v in variants if "720" in v), variants[0]))

            variant = await js_fetch(page, best)
            if variant["status"] != 200:
                await browser.close()
                sys.exit(1)

            _manifest = variant["text"].encode("utf-8")
            await browser.close()

            s = socket.socket(); s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]; s.close()
            srv = HTTPServer(("127.0.0.1", port), Handler)
            Thread(target=srv.serve_forever, daemon=True).start()

            print(f"http://127.0.0.1:{port}/stream.m3u8", flush=True)

            import time
            while True:
                time.sleep(60)

        elif webm_url:
            # --- WebM шлях: отримуємо фінальний URL через редирект, mpv грає напряму ---
            result = await page.evaluate("""async (url) => {
                try {
                    const r = await fetch(url, {redirect: "follow", headers: {Range: "bytes=0-0"}});
                    return {finalUrl: r.url, status: r.status};
                } catch(e) {
                    return {error: e.toString()};
                }
            }""", webm_url)
            await browser.close()

            final_url = result.get("finalUrl", "")
            status = result.get("status", 0)
            if not final_url or status not in (200, 206):
                sys.exit(1)

            # s1.mooncdn.space доступний для mpv без proxy
            print(final_url, flush=True)
            # Виходимо — proxy не потрібен

        else:
            await browser.close()
            sys.exit(1)

asyncio.run(main())
"#;

/// Запускає MoonAnime HLS proxy (Python + Playwright Firefox headless).
/// Скрипт завантажує variant manifest через browser JS fetch (обходить TLS fingerprint),
/// стартує локальний HTTP сервер з Content-Type: application/vnd.apple.mpegurl,
/// виводить URL в stdout. Процес живе доки Rust не викличе kill().
///
/// Повертає `Some((http_url, child_process))` або `None` якщо не вдалось.
async fn try_moonanime_stream(
    iframe_url: &str,
) -> Option<(String, tokio::process::Child)> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let script_path = std::env::temp_dir().join("anihub_moon_extract.py");
    std::fs::write(&script_path, MOONANIME_PLAYWRIGHT_SCRIPT).ok()?;

    let mut child = tokio::process::Command::new("python3")
        .arg(&script_path)
        .arg(iframe_url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // Читаємо першу непорожню лінію зі stdout — це HTTP URL proxy
    let stdout = child.stdout.take()?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let read_result = tokio::time::timeout(
        tokio::time::Duration::from_secs(40),
        reader.read_line(&mut line),
    )
    .await;

    match read_result {
        Ok(Ok(_)) => {
            let url = line.trim().to_string();
            // Приймаємо і HLS proxy (http://127.0.0.1) і WebM direct URL (https://s1.mooncdn.space)
            if url.starts_with("http://") || url.starts_with("https://") {
                debug_log(&format!("[moonanime] stream OK: {}", url));
                Some((url, child))
            } else {
                debug_log(&format!("[moonanime] bad output: {:?}", url));
                let _ = child.kill().await;
                None
            }
        }
        Ok(Err(e)) => {
            debug_log(&format!("[moonanime] read error: {}", e));
            let _ = child.kill().await;
            None
        }
        Err(_) => {
            debug_log("[moonanime] proxy timeout (>40s)");
            let _ = child.kill().await;
            None
        }
    }
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
    
    let sources = app.sources_cache.get(current_anime_id).or_else(|| app.current_sources.clone())?;
    
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
            
        let is_moonanime = next_ep.url.starts_with("https://moonanime.art");
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
            referrer: if is_moonanime {
                "https://moonanime.art/".to_string()
            } else {
                "https://ashdi.vip/".to_string()
            },
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

    let is_moonanime = next_ep.url.starts_with("https://moonanime.art");
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
        referrer: if is_moonanime {
            "https://moonanime.art/".to_string()
        } else {
            "https://ashdi.vip/".to_string()
        },
    })
}
