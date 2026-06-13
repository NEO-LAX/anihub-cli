use crate::{get_or_fetch_details, get_or_fetch_sources, apply_continue_context, apply_library_continue_context};
use crate::ui::{AppState, ContinueRequest};
use crate::moonanime::try_moonanime_stream;
use crate::api;
use crate::api::EpisodeSourcesResponse;
use crate::storage;


#[derive(Clone)]
pub struct PlayTarget {
    pub anime_id: u32,
    pub anime_title: String,
    pub player_title: String,
    pub season: u32,
    pub episode: u32,
    pub episode_title: String,
    pub stream_page_url: String,
    pub start_time: Option<f64>,
    pub studio_name: String,
    /// HTTP Referer для mpv — "https://ashdi.vip/" або "https://moonanime.art/"
    pub referrer: String,
}



pub async fn start_episode_playback(app: &mut AppState) {
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



pub async fn continue_playback(app: &mut AppState, request: ContinueRequest) {
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



pub async fn play_target(app: &mut AppState, target: PlayTarget) {
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
pub struct ContinueResolvedEpisode {
    pub season: u32,
    pub episode: u32,
    pub season_index: usize,
    pub dubbing_index: usize,
    pub episode_index: usize,
    pub url: String,
    pub start_time: Option<f64>,
    pub studio_name: String,
}



pub fn resolve_continue_target(
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



pub async fn check_playback_finished(app: &mut AppState) {
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



pub fn get_next_episode(app: &AppState, current: &(u32, String, u32, u32, String)) -> Option<PlayTarget> {
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
