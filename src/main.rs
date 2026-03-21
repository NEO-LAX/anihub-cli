mod api;
mod storage;
mod ui;
mod player;

use anyhow::Result;

/// Пише debug-повідомлення у файл, не в stderr — щоб не ламати TUI.
fn debug_log(msg: &str) {
    use std::io::Write;
    let mut path = std::env::temp_dir();
    path.push("anihub_debug.log");
    let _ = std::fs::OpenOptions::new()
        .append(true).create(true)
        .open(path)
        .and_then(|mut f| writeln!(f, "{msg}").map_err(Into::into));
}
use crossterm::{
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::stdout;
use ui::{AppMode, AppState, FocusPanel};
use player::MpvPlayer;
use api::{AshdiStudio, EpisodeSourcesResponse};

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
                        if let Some(d) = details { app.details_cache.insert(id, d); }
                        if let Some(s) = sources { app.sources_cache.insert(id, s); }
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
    let poster_url = app.details_cache.get(&anime_id)
        .and_then(|d| d.poster_url.clone())
        .or_else(|| {
            app.current_details.as_ref()
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
    let (Some(result_idx), Some(e_idx)) = (
        app.selected_result_index,
        app.selected_episode_index,
    ) else {
        return;
    };

    let anime_title = {
        let anime = match app.search_results.get(result_idx) {
            Some(a) => a,
            None => return,
        };
        format!("{} ({})", anime.title_ukrainian, anime.year.unwrap_or(0))
    };

    // Витягуємо дані з обраної студії до будь-яких мутацій
    let studio_info = app.selected_studio().and_then(|s| {
        s.episodes.get(e_idx).map(|ep| (ep.url.clone(), ep.episode_number, s.season_number))
    });
    let (target_url, episode_num, actual_season) = match studio_info {
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
        sources.ashdi.iter().enumerate()
            .filter(|(_, s)| s.season_number == season_num)
            .nth(dub_idx)
            .map(|(i, _)| i)
    });
    let progress_anime_id = studio_idx
        .and_then(|i| app.studio_anime_ids.get(i).copied())
        .or_else(|| app.search_results.get(result_idx).map(|a| a.id))
        .unwrap_or(0);
    let progress_title = app.search_results.iter()
        .find(|a| a.id == progress_anime_id)
        .map(|a| a.title_ukrainian.clone())
        .unwrap_or_else(|| {
            app.search_results.get(result_idx)
                .map(|a| a.title_ukrainian.clone())
                .unwrap_or_default()
        });

    let episode_title = format!("Серія {}", episode_num);

    // Якщо джерело — MoonAnime: пробуємо витягнути потік через yt-dlp з кукі браузера,
    // якщо не виходить — відкриваємо у браузері.
    if target_url.starts_with("https://moonanime.art") {
        match try_moonanime_stream(&target_url).await {
            Some(stream_url) => {
                let player = match MpvPlayer::new() {
                    Ok(p) => p,
                    Err(e) => { app.error_msg = Some(format!("Помилка плеєра: {}", e)); return; }
                };
                match player.start(&stream_url, None, &anime_title, &episode_title).await {
                    Ok((child, monitor)) => {
                        app.mpv_child = Some(child);
                        app.mpv_monitor = Some(monitor);
                        app.pending_progress = Some((progress_anime_id, progress_title, actual_season, episode_num));
                        app.is_playing = true;
                    }
                    Err(e) => app.error_msg = Some(format!("Помилка відтворення: {}", e)),
                }
            }
            None => {
                // Fallback: відкриваємо у браузері
                std::process::Command::new("xdg-open").arg(&target_url).spawn().ok();
                app.error_msg = Some(format!("MoonAnime відкрито у браузері\n(yt-dlp не зміг витягнути потік)"));
            }
        }
        return;
    }

    let parser = match api::AshdiParser::new() {
        Ok(p) => p,
        Err(e) => { app.error_msg = Some(format!("Помилка парсера: {}", e)); return; }
    };

    let m3u8 = match parser.extract_m3u8(&target_url).await {
        Ok(u) => u,
        Err(e) => { app.error_msg = Some(format!("Помилка парсингу: {}", e)); return; }
    };

    let player = match MpvPlayer::new() {
        Ok(p) => p,
        Err(e) => { app.error_msg = Some(format!("Помилка плеєра: {}", e)); return; }
    };

    match player.start(&m3u8, None, &anime_title, &episode_title).await {
        Ok((child, monitor)) => {
            app.mpv_child = Some(child);
            app.mpv_monitor = Some(monitor);
            app.pending_progress = Some((progress_anime_id, progress_title, actual_season, episode_num));
            app.is_playing = true;
        }
        Err(e) => {
            app.error_msg = Some(format!("Помилка відтворення: {}", e));
        }
    }
}

async fn check_playback_finished(app: &mut AppState) {
    let finished = if let Some(child) = &mut app.mpv_child {
        match child.try_wait() {
            Ok(Some(_)) => true,
            Ok(None)    => false,
            Err(_)      => true,
        }
    } else {
        false
    };

    if finished {
        app.mpv_child = None;
        let stopped_time = if let Some(monitor) = app.mpv_monitor.take() {
            monitor.await.unwrap_or(0.0)
        } else {
            0.0
        };
        if let Some((anime_id, title, season, episode)) = app.pending_progress.take() {
            let _ = app.storage.update_progress(anime_id, &title, season, episode, stopped_time);
        }
        app.is_playing = false;
        if let Ok(p) = MpvPlayer::new() {
            p.cleanup();
        }
    }
}

async fn handle_loading_tasks(app: &mut AppState) {
    app.error_msg = None;

    // 1. Пошук
    if app.mode == AppMode::Normal && !app.search_query.is_empty() {
        match app.api_client.search_anime(&app.search_query).await {
            Ok(results) => {
                app.search_results = api::deduplicate_anime(results);
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

                    let (tx, rx) = tokio::sync::mpsc::channel(64);
                    app.prefetch_rx = Some(rx);
                    app.prefetching = true;
                    for anime in app.search_results.clone() {
                        let client = app.api_client.clone();
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            let details = client.get_anime_details(anime.id).await.ok();
                            let sources = client.get_episode_sources_for_anime(anime.id).await.ok();
                            let _ = tx.send((anime.id, details, sources)).await;
                        });
                    }
                } else {
                    app.error_msg = Some("Нічого не знайдено".to_string());
                }
            }
            Err(e) => app.error_msg = Some(format!("Помилка пошуку: {}", e)),
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
                    let first_tv_id = app.studio_anime_ids.first().copied()
                        .unwrap_or_else(|| get_first_tv_id(app).unwrap_or(item.id));
                    app.poster_fetch_pending = Some(first_tv_id);
                }
            }
        }
        return;
    }
}

/// Об'єднує джерела серій з усіх TV-елементів поточної франшизи.
/// Також збагачує через AniList (знаходить приховані члени з іншими назвами та фільми).
async fn load_combined_sources(app: &mut AppState, representative_id: u32) {
    // Будуємо початковий список TV-членів (id, year) з franchise_groups
    let mut tv_with_year: Vec<(u32, u32)> = if let Some(g_idx) = app.selected_group_index {
        if let Some(group) = app.franchise_groups.get(g_idx) {
            let mut tv: Vec<(u32, u32)> = group.iter()
                .map(|&i| &app.search_results[i])
                .filter(|a| a.anime_type.to_lowercase() == "tv")
                .map(|a| (a.id, a.year.unwrap_or(0)))
                .collect();
            // Сортуємо за anilist_id (відображає порядок виходу), fallback — рік
            // Рік дубляжу на anihub може бути пізніший оригінального, тому anilist_id надійніший
            tv.sort_by(|&(a_id, a_year), &(b_id, b_year)| {
                let a_al = app.search_results.iter().find(|a| a.id == a_id).and_then(|a| a.anilist_id).unwrap_or(u32::MAX);
                let b_al = app.search_results.iter().find(|a| a.id == b_id).and_then(|a| a.anilist_id).unwrap_or(u32::MAX);
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
        let mut id_to_anilist: std::collections::HashMap<u32, u32> = app.search_results.iter()
            .filter_map(|a| a.anilist_id.map(|al| (a.id, al)))
            .collect();

        let extra_tv = augment_franchise_with_anilist(app, &current_tv_ids, representative_id).await;
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
                    Some(al_id) => anilist_members.iter()
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
        let mut join_set: tokio::task::JoinSet<(u32, Option<EpisodeSourcesResponse>)> = tokio::task::JoinSet::new();
        for &anime_id in &franchise_tv_ids {
            if let Some(cached) = app.sources_cache.get(&anime_id).cloned() {
                join_set.spawn(async move { (anime_id, Some(cached)) });
            } else {
                let client = app.api_client.clone();
                join_set.spawn(async move { (anime_id, client.get_episode_sources_for_anime(anime_id).await.ok()) });
            }
        }
        let mut fetched: Vec<(u32, Option<EpisodeSourcesResponse>)> = Vec::with_capacity(franchise_tv_ids.len());
        while let Some(Ok(result)) = join_set.join_next().await {
            fetched.push(result);
        }
        // Відновлюємо порядок franchise_tv_ids (JoinSet не гарантує порядок)
        fetched.sort_by_key(|(id, _)| franchise_tv_ids.iter().position(|&x| x == *id).unwrap_or(usize::MAX));
        let mut all: Vec<(u32, AshdiStudio)> = Vec::new();
        let mut per_member: std::collections::HashMap<u32, Vec<AshdiStudio>> = std::collections::HashMap::new();
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
        let active_franchise_ids: Vec<u32> = franchise_tv_ids.iter()
            .copied()
            .filter(|id| per_member.contains_key(id))
            .collect();

        // Dedup по (season_number, studio_name); зберігаємо entry з найбільшим episodes.len()
        let mut best: Vec<(u32, AshdiStudio)> = Vec::new();
        for (aid, studio) in &all {
            if let Some(pos) = best.iter().position(|(_, s)| s.season_number == studio.season_number && s.studio_name == studio.studio_name) {
                if studio.episodes.len() >= best[pos].1.episodes.len() {
                    best[pos] = (*aid, studio.clone());
                }
            } else {
                best.push((*aid, studio.clone()));
            }
        }
        best.sort_by(|(_, a), (_, b)| {
            a.season_number.cmp(&b.season_number)
                .then(b.episodes_count.cmp(&a.episodes_count))
        });
        let unique = best;
        let unique_season_nums: Vec<u32> = {
            let mut v: Vec<u32> = Vec::new();
            for (_, s) in &unique { if !v.contains(&s.season_number) { v.push(s.season_number); } }
            v
        };
        let n_seasons = unique_season_nums.len();
        let n_members = active_franchise_ids.len();

        if n_members >= 2 * n_seasons.max(1) {
            // Per-member режим: кожен anime_id отримує свій sequential сезон-слот.
            // Ashdi крос-лінкує старі сезони в нові члени → виявляємо дублікати по URL першого
            // епізоду: якщо всі сезони члена вже claimed попередніми членами — пропускаємо.
            let mut season_counter = 1u32;
            let mut claimed_urls: std::collections::HashSet<String> = std::collections::HashSet::new();
            for &anime_id in &active_franchise_ids {
                let member_studios = per_member.get(&anime_id).unwrap(); // safe: відфільтровано вище
                // Сезони в порядку зростання
                let mut season_nums: Vec<u32> = member_studios.iter().map(|s| s.season_number).collect();
                season_nums.sort(); season_nums.dedup();
                // Шукаємо перший сезон, де є хоч одна студія з непривласненим URL першого епізоду
                let chosen = season_nums.iter().find(|&&s_num| {
                    member_studios.iter()
                        .filter(|s| s.season_number == s_num)
                        .any(|s| s.episodes.first().map(|e| !claimed_urls.contains(&e.url)).unwrap_or(false))
                }).copied();
                let Some(chosen_s) = chosen else { continue }; // всі сезони — кросс-лінк → пропускаємо
                for studio in member_studios.iter().filter(|s| s.season_number == chosen_s) {
                    if let Some(ep) = studio.episodes.first() {
                        claimed_urls.insert(ep.url.clone());
                    }
                    combined.push(AshdiStudio { season_number: season_counter, ..studio.clone() });
                    anime_ids.push(anime_id);
                }
                season_counter += 1;
            }
        } else {
            // Back-align: якщо franchise-членів більше ніж сезонів Ashdi,
            // вирівнюємо з кінця: SN → active_members[offset + season_pos].
            let member_offset = n_members.saturating_sub(n_seasons);
            for (data_aid, studio) in unique {
                let season_pos = unique_season_nums.iter().position(|&s| s == studio.season_number).unwrap_or(0);
                let new_season = (season_pos + 1) as u32;
                let owner_id = active_franchise_ids
                    .get(season_pos + member_offset)
                    .copied()
                    .unwrap_or_else(|| active_franchise_ids.last().copied().unwrap_or(data_aid));
                anime_ids.push(owner_id);
                combined.push(AshdiStudio { season_number: new_season, ..studio });
            }
        }
        // DEBUG → /tmp/anihub_debug.log
        debug_log(&format!("[DEBUG] representative_id={representative_id}"));
        debug_log("[DEBUG] active_franchise_ids (впорядковані):");
        for &id in &active_franchise_ids {
            let title = app.search_results.iter().find(|a| a.id == id).map(|a| a.title_ukrainian.as_str()).unwrap_or("?");
            let al = app.search_results.iter().find(|a| a.id == id).and_then(|a| a.anilist_id)
                .or_else(|| app.details_cache.get(&id).and_then(|d| d.anilist_id));
            debug_log(&format!("[DEBUG]   id={id}  anilist_id={al:?}  title={title:?}"));
        }
        debug_log(&format!("[DEBUG] n_members={n_members}  n_seasons={n_seasons}  mode={}", if n_members >= 2 * n_seasons.max(1) { "per-member" } else { "back-align" }));
        for (i, (s, id)) in combined.iter().zip(anime_ids.iter()).enumerate() {
            let first_ep = s.episodes.first().map(|e| (e.episode_number, e.url.chars().take(60).collect::<String>())).unwrap_or((0, String::new()));
            debug_log(&format!("[DEBUG] combined[{i}] season={} studio={:?} ep={} owner={id}  first_ep#{}  url_prefix={:?}", s.season_number, s.studio_name, s.episodes_count, first_ep.0, first_ep.1));
        }
    } else {
        if franchise_tv_ids.is_empty() {
            // Fallback: всі члени групи — OVA/movie/спец → використовуємо representative
            match app.api_client.get_episode_sources_for_anime(representative_id).await {
                Ok(sources) => {
                    app.studio_anime_ids = vec![representative_id; sources.ashdi.len()];
                    app.current_sources = Some(sources);
                }
                Err(e) => app.error_msg = Some(format!("Помилка джерел: {}", e)),
            }
            return;
        }
        let anime_id = franchise_tv_ids[0];
        let sources = if let Some(cached) = app.sources_cache.get(&anime_id).cloned() {
            Some(cached)
        } else {
            app.api_client.get_episode_sources_for_anime(anime_id).await.ok()
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
        app.current_sources = Some(EpisodeSourcesResponse { ashdi: combined, moonanime: Vec::new() });
    } else {
        match app.api_client.get_episode_sources_for_anime(representative_id).await {
            Ok(sources) => {
                app.studio_anime_ids = vec![representative_id; sources.ashdi.len()];
                app.current_sources = Some(sources);
            }
            Err(e) => app.error_msg = Some(format!("Помилка джерел: {}", e)),
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
    let first_known_al_id = current_tv_ids.iter()
        .filter_map(|&id| app.search_results.iter().find(|a| a.id == id).and_then(|a| a.anilist_id))
        .next();

    let anilist_members = if let Some(cached) = app.anilist_cache.get(&representative_id) {
        cached.clone()
    } else {
        let members = if let Some(al_id) = first_known_al_id {
            api::anilist::get_franchise_members_by_id(app.api_client.http_client(), al_id).await
        } else {
            let title_original = app.search_results.iter()
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
        let year = app.search_results.iter()
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
    client.get_anime_by_anilist_id(member.anilist_id).await.ok().flatten()
}

/// Намагається витягнути URL потоку з moonanime через yt-dlp з кукі браузера.
/// Перебирає Firefox, Chrome, Chromium. Повертає None якщо не вдалось.
async fn try_moonanime_stream(iframe_url: &str) -> Option<String> {
    for browser in &["firefox", "chrome", "chromium"] {
        let Ok(output) = tokio::process::Command::new("yt-dlp")
            .args([
                "--cookies-from-browser", browser,
                "--get-url",
                "--no-playlist",
                "--quiet",
                iframe_url,
            ])
            .output()
            .await
        else { continue; };

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
    let mut tv: Vec<(u32, u32)> = group.iter()
        .map(|&i| &app.search_results[i])
        .filter(|a| {
            let t = a.anime_type.to_lowercase();
            !t.contains("ova") && !t.contains("ona") && !t.contains("фільм") && !t.contains("film")
                && !t.contains("спец") && !t.contains("special") && !t.contains("movie") && !t.contains("short")
        })
        .map(|a| (a.id, a.year.unwrap_or(0)))
        .collect();
    tv.sort_by_key(|&(_, y)| y);
    tv.first().map(|(id, _)| *id)
}
