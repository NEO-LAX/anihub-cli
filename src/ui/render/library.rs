//! Library tab renderers.

use super::*;
use chrono::{Local, TimeZone};

pub(super) fn render_sidebar(f: &mut Frame, app: &mut AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color_primary()))
        .title(Span::styled(
            " Інформація ",
            Style::default().fg(color_primary()),
        ))
        .title_alignment(Alignment::Center);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.library_selected_anime().is_none() {
        render_centered_sidebar_message(
            f,
            inner,
            app.activity_message
                .as_deref()
                .unwrap_or("Оберіть тайтл зі списку"),
        );
        return;
    }

    let has_eng = app
        .current_details
        .as_ref()
        .and_then(|d| d.title_english.as_ref())
        .is_some();
    let title_h: u16 = if has_eng { 2 } else { 1 };

    if app.current_poster.is_some() && inner.height > title_h + 5 {
        let poster_h = sidebar_poster_height(inner, title_h);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(title_h),
                Constraint::Length(1),
                Constraint::Length(poster_h),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);

        render_library_sidebar_title_area(f, app, chunks[0]);

        let sep_style = Style::default().fg(color_dim());
        let sep_w = inner.width as usize;
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("─".repeat(sep_w), sep_style))),
            chunks[1],
        );

        if let Some(poster) = &mut app.current_poster {
            let avail = chunks[2];
            let img_rect = poster.size_for(ratatui_image::Resize::Fit(None), avail);
            let x_off = (avail.width.saturating_sub(img_rect.width)) / 2;
            let centered = Rect::new(
                avail.x + x_off,
                avail.y,
                img_rect.width.min(avail.width),
                avail.height,
            );
            f.render_stateful_widget(
                StatefulImage::<StatefulProtocol>::default(),
                centered,
                poster,
            );
        }

        f.render_widget(
            Paragraph::new(Line::from(Span::styled("─".repeat(sep_w), sep_style))),
            chunks[3],
        );

        render_library_sidebar_details_area(f, app, chunks[4], false);
    } else {
        render_library_sidebar_details_area(f, app, inner, true);
    }
}

pub(super) fn render_lists(f: &mut Frame, app: &mut AppState, area: Rect) {
    let compact = area.width < 90;
    let constraints = match (app.mode, compact) {
        (AppMode::Library, _) => vec![Constraint::Percentage(100)],
        (AppMode::LibrarySeason, true) => {
            vec![Constraint::Percentage(25), Constraint::Percentage(75)]
        }
        (AppMode::LibraryDubbing, true) => vec![
            Constraint::Percentage(15),
            Constraint::Percentage(25),
            Constraint::Percentage(60),
        ],
        (AppMode::LibraryEpisode, true) => vec![
            Constraint::Percentage(10),
            Constraint::Percentage(15),
            Constraint::Percentage(20),
            Constraint::Percentage(55),
        ],
        (AppMode::LibrarySeason, false) => {
            vec![Constraint::Percentage(50), Constraint::Percentage(50)]
        }
        (AppMode::LibraryDubbing, false) => vec![
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ],
        (AppMode::LibraryEpisode, false) => vec![
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ],
        _ => vec![Constraint::Percentage(100)],
    };

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    let library_title = format!(
        " {} · {} {} ",
        app.library.filter.label(),
        app.library.sort.label(),
        app.library.sort.direction_symbol(app.library.sort_reversed)
    );
    if app.library.items.is_empty() {
        let border_style = if app.mode == AppMode::Library {
            Style::default().fg(color_highlight())
        } else {
            Style::default().fg(color_dim())
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(library_title)
            .title_alignment(Alignment::Center)
            .border_style(border_style);
        let inner = block.inner(chunks[0]);
        f.render_widget(block, chunks[0]);

        let message = if app.library.search_query.is_empty() {
            format!("У категорії «{}» поки порожньо", app.library.filter.label())
        } else {
            format!("Нічого не знайдено за «{}»", app.library.search_query)
        };
        let centered = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .split(inner);
        f.render_widget(
            Paragraph::new(Span::styled(message, Style::default().fg(color_dim())))
                .alignment(Alignment::Center),
            centered[1],
        );
    } else {
        let anime_items: Vec<ListItem> = app
            .library
            .items
            .iter()
            .map(|item| {
                let seasons = item
                    .seasons
                    .iter()
                    .filter(|release| release.kind == LibraryReleaseKind::Season)
                    .count();
                let extras = item.seasons.len().saturating_sub(seasons);
                let mut metadata = Vec::new();
                if seasons > 0 {
                    metadata.push(season_count_label(seasons));
                }
                if extras > 0 {
                    metadata.push(format!("{extras} дод."));
                }
                if let Some((watched, total)) = anime_progress(item) {
                    metadata.push(format!("{watched}/{total}"));
                }
                if let Some(label) = new_content_summary(&app.settings, &item.seasons) {
                    metadata.push(label);
                }
                ListItem::new(label_with_metadata(&item.anime_title, &metadata))
            })
            .collect();
        let anime_list = create_list(&library_title, anime_items, app.mode == AppMode::Library);
        f.render_stateful_widget(anime_list, chunks[0], &mut app.library.anime_list_state);
    }

    if chunks.len() >= 2 {
        let season_items: Vec<ListItem> = app
            .library_selected_anime()
            .into_iter()
            .flat_map(|anime| anime.seasons.iter())
            .map(|release| {
                let count = app.dubbing_choices_for_season(release.season).len();
                let mut metadata = release_progress(release)
                    .map(|(watched, total)| format!("{watched}/{total}"))
                    .into_iter()
                    .collect::<Vec<_>>();
                if count > 1 {
                    metadata.push(format!("{count} озвучок"));
                }
                if let Some(label) = new_content_label(&app.settings, release) {
                    metadata.push(label);
                }
                if let Some(next_airing) = next_airing_label(release) {
                    metadata.push(next_airing);
                }
                let release_label = match release.kind {
                    LibraryReleaseKind::Season => match release.part {
                        Some(part) if part > 1 => {
                            format!("Сезон {} · Частина {part}", release.season)
                        }
                        _ => format!("Сезон {}", release.season),
                    },
                    LibraryReleaseKind::Movie => format!("Фільм · {}", release.title),
                    LibraryReleaseKind::Special => format!("Спецвипуск · {}", release.title),
                    LibraryReleaseKind::Extra => format!("Додатково · {}", release.title),
                };
                let label = label_with_metadata(&release_label, &metadata);
                let marker = if season_is_complete(app, release.anime_id, release.season) {
                    "✓"
                } else {
                    ""
                };
                ListItem::new(with_right_marker(
                    &label,
                    marker,
                    chunks[1].width.saturating_sub(6) as usize,
                ))
            })
            .collect();
        if season_items.is_empty() {
            render_list_message(
                f,
                chunks[1],
                " Випуски ",
                app.mode == AppMode::LibrarySeason,
                if app.loading {
                    "Завантаження випусків…"
                } else {
                    "Випусків не знайдено"
                },
                app.loading,
            );
        } else {
            let season_list = create_list(
                " Випуски ",
                season_items,
                app.mode == AppMode::LibrarySeason,
            );
            f.render_stateful_widget(season_list, chunks[1], &mut app.season_list_state);
        }
    }

    if chunks.len() >= 3 {
        let dubbing_items: Vec<ListItem> = if let Some(sn) = app.selected_season_num() {
            app.dubbing_choices_for_season(sn)
                .iter()
                .map(|choice| {
                    let mut metadata = vec![format!("{} сер.", choice.episodes_count())];
                    if choice.is_moonanime() {
                        metadata.push("MoonAnime".to_string());
                    }
                    ListItem::new(label_with_metadata(choice.studio_name(), &metadata))
                })
                .collect()
        } else {
            vec![]
        };
        if dubbing_items.is_empty() {
            let (message, loading) = if app.loading {
                ("Завантаження озвучок…", true)
            } else if app.selected_season_num().is_none() {
                ("Оберіть випуск", false)
            } else {
                ("Озвучок не знайдено", false)
            };
            render_list_message(
                f,
                chunks[2],
                " Озвучки ",
                app.mode == AppMode::LibraryDubbing,
                message,
                loading,
            );
        } else {
            let dubbing_list = create_list(
                " Озвучки ",
                dubbing_items,
                app.mode == AppMode::LibraryDubbing,
            );
            f.render_stateful_widget(dubbing_list, chunks[2], &mut app.dubbing_list_state);
        }
    }

    if chunks.len() >= 4 {
        let episode_items: Vec<ListItem> = if app
            .selected_dubbing_choice()
            .is_some_and(|choice| choice.is_moonanime())
        {
            let anime_id = app.library_selected_anime_id();
            let season = app.selected_season_num();
            app.selected_episode_choices()
                .iter()
                .map(|episode| {
                    let title = episode.title();
                    let suffix = if title.is_empty() {
                        "".to_string()
                    } else {
                        format!(" · {title}")
                    };
                    let label = format!("Серія {}{}", episode.episode_number(), suffix);
                    let mut metadata = vec!["браузер".to_string()];
                    if anime_id.zip(season).is_some_and(|(anime_id, season)| {
                        episode_is_watched(app, anime_id, season, episode.episode_number())
                    }) {
                        metadata.push("✓".to_string());
                    }
                    ListItem::new(label_with_metadata(&label, &metadata))
                })
                .collect()
        } else if let Some(studio) = app.selected_studio() {
            let anime_id = app.library_selected_anime_id();
            studio
                .episodes
                .iter()
                .map(|episode| {
                    let label = format!("Серія {}", episode.episode_number);
                    let mut metadata = Vec::new();
                    if let Some(t) = anime_id.and_then(|id| {
                        episode_progress_timestamp(
                            app,
                            id,
                            studio.season_number,
                            episode.episode_number,
                        )
                    }) {
                        metadata.push(format!("⏱ {}", format_elapsed_timestamp(t)));
                    }
                    if anime_id.is_some_and(|id| {
                        episode_is_watched(app, id, studio.season_number, episode.episode_number)
                    }) {
                        metadata.push("✓".to_string());
                    }
                    ListItem::new(label_with_metadata(&label, &metadata))
                })
                .collect()
        } else {
            vec![]
        };
        if episode_items.is_empty() {
            let (message, loading) = if app.loading {
                ("Завантаження серій…", true)
            } else if app.selected_dubbing_choice().is_none() {
                ("Оберіть озвучку", false)
            } else {
                ("Серій не знайдено", false)
            };
            render_list_message(
                f,
                chunks[3],
                " Серії ",
                app.mode == AppMode::LibraryEpisode,
                message,
                loading,
            );
        } else {
            let episode_list = create_list(
                " Серії ",
                episode_items,
                app.mode == AppMode::LibraryEpisode,
            );
            f.render_stateful_widget(episode_list, chunks[3], &mut app.episode_list_state);
        }
    }
}

pub(super) fn render_sort_popup(f: &mut Frame, app: &AppState) {
    let Some(selected) = app.library.sort_popup else {
        return;
    };
    let actions = [
        ("Enter", "Застосувати / ↕", color_highlight()),
        ("Esc", "", color_dim()),
    ];
    let area = centered_fixed(f.area(), dialog_width_for(54, &actions), 11);
    let block = dialog_block(
        " Сортування бібліотеки ",
        color_highlight(),
        color_highlight(),
    );
    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let items = LibrarySort::ALL
        .iter()
        .map(|sort| {
            let active = *sort == app.library.sort;
            let reversed = active && app.library.sort_reversed;
            let marker = if active { "✓" } else { " " };
            ListItem::new(format!(
                "{marker} {} · {}",
                sort.label(),
                sort.order_label(reversed)
            ))
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .highlight_symbol(">> ")
        .highlight_style(selection_style());
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(selected));
    f.render_stateful_widget(list, rows[0], &mut state);
    f.render_widget(
        Paragraph::new(action_footer_line(&actions)).alignment(Alignment::Center),
        rows[1],
    );
}

pub(super) fn render_watched_confirmation(f: &mut Frame, app: &AppState) {
    let Some(confirmation) = &app.library.pending_watched_confirmation else {
        return;
    };
    let release_count = confirmation.releases.len();
    let action = if confirmation.mark_watched {
        "Позначити переглянутими"
    } else {
        "Позначити непереглянутими"
    };
    let body = vec![
        Line::from(Span::styled(
            truncate_middle(&confirmation.anime_title, 44),
            Style::default()
                .fg(color_text())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("{action} усі випуски ({release_count})?"),
            Style::default().fg(color_dim()),
        )),
    ];
    render_confirm_dialog(
        f,
        " Статус усього аніме ",
        color_highlight(),
        &body,
        &[
            ("Enter", "Підтвердити", color_highlight()),
            ("Esc", "", color_dim()),
        ],
        50,
        8,
    );
}

fn release_progress(release: &crate::ui::app::LibrarySeasonEntry) -> Option<(u32, u32)> {
    let total = release.episodes_count?;
    let watched = if release.status == AnimeStatus::Completed {
        total
    } else {
        let first = release.first_episode.unwrap_or(1);
        let end = first.saturating_add(total);
        release
            .episodes
            .iter()
            .filter(|progress| {
                progress.watched && progress.episode >= first && progress.episode < end
            })
            .map(|progress| progress.episode)
            .collect::<std::collections::HashSet<_>>()
            .len()
            .min(total as usize) as u32
    };
    Some((watched, total))
}

fn anime_progress(anime: &crate::ui::app::LibraryAnimeEntry) -> Option<(u32, u32)> {
    let progress = anime
        .seasons
        .iter()
        .filter_map(release_progress)
        .collect::<Vec<_>>();
    (!progress.is_empty()).then(|| {
        progress.into_iter().fold(
            (0, 0),
            |(watched, total), (release_watched, release_total)| {
                (watched + release_watched, total + release_total)
            },
        )
    })
}

fn displayed_library_progress(
    mode: AppMode,
    anime: &crate::ui::app::LibraryAnimeEntry,
    release: Option<&crate::ui::app::LibrarySeasonEntry>,
) -> Option<(u32, u32)> {
    if mode == AppMode::Library {
        anime_progress(anime)
    } else {
        release.and_then(release_progress)
    }
}

fn latest_progress_for_release(
    release: &crate::ui::app::LibrarySeasonEntry,
) -> Option<&crate::storage::WatchProgress> {
    let first = release.first_episode.unwrap_or(1);
    let end = release
        .episodes_count
        .map(|total| first.saturating_add(total));
    release
        .episodes
        .iter()
        .filter(|progress| {
            progress.episode >= first && end.is_none_or(|end| progress.episode < end)
        })
        .max_by_key(|progress| progress.updated_at)
}

fn library_tracking_lines(app: &AppState) -> Vec<Line<'static>> {
    let Some(anime) = app.library_selected_anime() else {
        return Vec::new();
    };

    if app.mode == AppMode::Library {
        let Some((watched, total)) = displayed_library_progress(app.mode, anime, None) else {
            return Vec::new();
        };
        return tracking_summary_lines(
            anime.status,
            watched,
            Some(total),
            latest_progress_for_ids(app, &anime.anime_ids).map(TrackingPosition::from),
        );
    }

    let Some(release) = app.library_selected_season() else {
        return Vec::new();
    };
    let Some((watched, total)) = displayed_library_progress(app.mode, anime, Some(release)) else {
        return Vec::new();
    };
    tracking_summary_lines(
        release.status,
        watched,
        Some(total),
        latest_progress_for_release(release).map(|progress| TrackingPosition {
            season: release.season,
            episode: progress.episode,
            timestamp: progress.timestamp,
        }),
    )
}

fn new_episode_count(
    settings: &crate::settings::Settings,
    release: &crate::ui::app::LibrarySeasonEntry,
) -> u32 {
    if !settings.new_content_initialized
        || !settings
            .acknowledged_release_ids
            .contains(&release.anime_id)
    {
        return 0;
    }
    let Some(current) = release.episodes_count else {
        return 0;
    };
    settings
        .seen_episode_counts
        .get(&release.anime_id)
        .map_or(0, |seen| current.saturating_sub(*seen))
}

fn new_content_label(
    settings: &crate::settings::Settings,
    release: &crate::ui::app::LibrarySeasonEntry,
) -> Option<String> {
    if !settings.new_content_initialized {
        return None;
    }
    if !settings
        .acknowledged_release_ids
        .contains(&release.anime_id)
    {
        return Some(
            match release.kind {
                LibraryReleaseKind::Season => "новий сезон",
                LibraryReleaseKind::Movie => "новий фільм",
                LibraryReleaseKind::Special => "новий спецвипуск",
                LibraryReleaseKind::Extra => "новий випуск",
            }
            .to_string(),
        );
    }
    new_episode_label(new_episode_count(settings, release))
}

fn new_content_summary(
    settings: &crate::settings::Settings,
    releases: &[crate::ui::app::LibrarySeasonEntry],
) -> Option<String> {
    if !settings.new_content_initialized {
        return None;
    }
    let new_releases = releases
        .iter()
        .filter(|release| {
            !settings
                .acknowledged_release_ids
                .contains(&release.anime_id)
        })
        .count();
    match new_releases {
        1 => return Some("новий випуск".to_string()),
        count if count > 1 => return Some(format!("{count} нових випусків")),
        _ => {}
    }

    new_episode_label(
        releases
            .iter()
            .map(|release| new_episode_count(settings, release))
            .sum(),
    )
}

fn new_episode_label(count: u32) -> Option<String> {
    match count {
        0 => None,
        1 => Some("нова серія".to_string()),
        count if (2..=4).contains(&(count % 10)) && !(12..=14).contains(&(count % 100)) => {
            Some(format!("{count} нові серії"))
        }
        count => Some(format!("{count} нових серій")),
    }
}

fn next_airing_label(release: &crate::ui::app::LibrarySeasonEntry) -> Option<String> {
    next_airing_label_at(release, chrono::Utc::now().timestamp())
}

fn next_airing_label_at(release: &crate::ui::app::LibrarySeasonEntry, now: i64) -> Option<String> {
    let episode = release.next_airing_episode?;
    let airing_at = release.next_airing_at?;
    if airing_at <= now {
        return None;
    }
    let local = Local.timestamp_opt(airing_at, 0).single()?;
    Some(format!("далі E{episode} · {}", local.format("%d.%m %H:%M")))
}

fn render_library_sidebar_title_area(f: &mut Frame, app: &AppState, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some(details) = &app.current_details {
        lines.push(
            Line::from(Span::styled(
                truncate_with_ellipsis(&details.title_ukrainian, area.width as usize),
                Style::default()
                    .fg(color_secondary())
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        );
        if let Some(eng) = &details.title_english {
            lines.push(
                Line::from(Span::styled(
                    truncate_with_ellipsis(eng, area.width as usize),
                    Style::default().fg(color_dim()),
                ))
                .alignment(Alignment::Center),
            );
        }
    } else if let Some(anime) = app.library_selected_anime() {
        lines.push(
            Line::from(Span::styled(
                truncate_with_ellipsis(&anime.anime_title, area.width as usize),
                Style::default()
                    .fg(color_secondary())
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        );
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn render_library_sidebar_details_area(
    f: &mut Frame,
    app: &AppState,
    area: Rect,
    include_title: bool,
) {
    let sep_w = area.width as usize;
    let mk_sep = || {
        Line::from(Span::styled(
            "─".repeat(sep_w),
            Style::default().fg(color_dim()),
        ))
    };
    let mut text: Vec<Line> = Vec::new();

    if let Some(details) = &app.current_details {
        if include_title {
            text.push(
                Line::from(Span::styled(
                    truncate_with_ellipsis(&details.title_ukrainian, area.width as usize),
                    Style::default()
                        .fg(color_secondary())
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            if let Some(eng) = &details.title_english {
                text.push(
                    Line::from(Span::styled(
                        truncate_with_ellipsis(eng, area.width as usize),
                        Style::default().fg(color_dim()),
                    ))
                    .alignment(Alignment::Center),
                );
            }
            text.push(mk_sep());
        }

        text.push(compact_metadata_line(
            &details.anime_type,
            details.year,
            details.rating,
            details.episodes_count.map(|episodes| episodes.to_string()),
        ));
        text.push(mk_sep());
        text.extend(library_tracking_lines(app));

        let has_extra = details.genres.as_ref().is_some_and(|g| !g.is_empty())
            || details
                .dubbing_studios
                .as_ref()
                .is_some_and(|s| !s.is_empty());
        if has_extra {
            text.push(mk_sep());
        }

        if let Some(studios) = &details.dubbing_studios {
            if !studios.is_empty() {
                let s = studios
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                text.push(Line::from(vec![
                    Span::styled("Озвучка: ", Style::default().fg(color_dim())),
                    Span::styled(s, Style::default().fg(color_success())),
                ]));
            }
        }

        if let Some(genres) = &details.genres {
            if !genres.is_empty() {
                text.push(Line::from(""));
                text.push(Line::from(vec![
                    Span::styled("Жанри: ", Style::default().fg(color_dim())),
                    Span::styled(
                        summarized_genres(genres),
                        Style::default().fg(color_highlight()),
                    ),
                ]));
            }
        }
    } else if let Some(anime) = app.library_selected_anime() {
        if include_title {
            text.push(
                Line::from(Span::styled(
                    truncate_with_ellipsis(&anime.anime_title, area.width as usize),
                    Style::default()
                        .fg(color_secondary())
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            text.push(mk_sep());
        }
        if app.loading {
            text.push(
                Line::from(Span::styled(
                    "Завантаження деталей…",
                    Style::default().fg(color_dim()),
                ))
                .alignment(Alignment::Center),
            );
        }
        text.extend(library_tracking_lines(app));
    } else {
        text.push(
            Line::from(Span::styled(
                app.activity_message
                    .as_deref()
                    .unwrap_or("Оберіть тайтл зі списку"),
                Style::default().fg(color_dim()),
            ))
            .alignment(Alignment::Center),
        );
    }

    f.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Left)
            .wrap(ratatui::widgets::Wrap { trim: true }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(episode: u32, watched: bool) -> crate::storage::WatchProgress {
        crate::storage::WatchProgress {
            anime_id: 42,
            anime_title: "Онгоінг".to_string(),
            season: 1,
            episode,
            studio_name: "Test".to_string(),
            timestamp: 0.0,
            duration: 0.0,
            watched,
            updated_at: i64::from(episode),
        }
    }

    fn ongoing_release() -> crate::ui::app::LibrarySeasonEntry {
        crate::ui::app::LibrarySeasonEntry {
            anime_id: 42,
            season: 1,
            part: Some(1),
            title: "Онгоінг".to_string(),
            kind: LibraryReleaseKind::Season,
            episodes_count: Some(8),
            first_episode: Some(1),
            airing_status: Some("RELEASING".to_string()),
            next_airing_episode: Some(9),
            next_airing_at: Some(2_000),
            status: AnimeStatus::Watching,
            episodes: Vec::new(),
        }
    }

    #[test]
    fn episode_badge_uses_the_acknowledged_count() {
        let release = ongoing_release();
        let seen = std::collections::BTreeMap::from([(42, 6)]);
        let mut settings = crate::settings::Settings {
            new_content_initialized: true,
            ..Default::default()
        };
        settings.acknowledged_release_ids.insert(release.anime_id);
        settings.seen_episode_counts = seen;
        assert_eq!(new_episode_count(&settings, &release), 2);

        let mut unknown_release = settings.clone();
        unknown_release.acknowledged_release_ids.clear();
        assert_eq!(new_episode_count(&unknown_release, &release), 0);
        assert_eq!(
            new_content_label(&unknown_release, &release).as_deref(),
            Some("новий сезон")
        );
    }

    #[test]
    fn new_episode_badge_uses_ukrainian_plural_forms() {
        assert_eq!(new_episode_label(0), None);
        assert_eq!(new_episode_label(1).as_deref(), Some("нова серія"));
        assert_eq!(new_episode_label(2).as_deref(), Some("2 нові серії"));
        assert_eq!(new_episode_label(5).as_deref(), Some("5 нових серій"));
        assert_eq!(new_episode_label(12).as_deref(), Some("12 нових серій"));
        assert_eq!(new_episode_label(22).as_deref(), Some("22 нові серії"));
    }

    #[test]
    fn future_airing_is_rendered_but_expired_airing_is_hidden() {
        let release = ongoing_release();
        assert!(next_airing_label_at(&release, 1_000).is_some());
        assert!(next_airing_label_at(&release, 2_000).is_none());
    }

    #[test]
    fn sidebar_progress_switches_between_franchise_and_selected_release() {
        let mut completed = ongoing_release();
        completed.episodes_count = Some(12);
        completed.status = AnimeStatus::Completed;

        let mut current = ongoing_release();
        current.anime_id = 43;
        current.season = 2;
        current.episodes = (1..=8).map(|episode| progress(episode, true)).collect();
        for entry in &mut current.episodes {
            entry.anime_id = 43;
            entry.season = 2;
        }
        current.episodes.push(crate::storage::WatchProgress {
            anime_id: 43,
            season: 2,
            episode: 13,
            ..progress(13, true)
        });
        current.episodes_count = Some(12);
        current.status = AnimeStatus::Watching;

        let anime = crate::ui::app::LibraryAnimeEntry {
            anime_ids: vec![42, 43],
            anime_title: "Франшиза".to_string(),
            latest_progress: current.episodes[0].clone(),
            seasons: vec![completed, current.clone()],
            status: AnimeStatus::Watching,
        };

        assert_eq!(
            displayed_library_progress(AppMode::Library, &anime, None),
            Some((20, 24))
        );
        assert_eq!(
            displayed_library_progress(AppMode::LibrarySeason, &anime, Some(&current)),
            Some((8, 12))
        );
    }
}
