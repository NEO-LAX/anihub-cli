//! Library tab renderers.

use super::*;

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
        " {} · {} ",
        app.library_filter.label(),
        app.library_sort.label()
    );
    if app.library_items.is_empty() {
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

        let message = if app.library_search_query.is_empty() {
            format!("У категорії «{}» поки порожньо", app.library_filter.label())
        } else {
            format!("Нічого не знайдено за «{}»", app.library_search_query)
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
            .library_items
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
                ListItem::new(label_with_metadata(&item.anime_title, &metadata))
            })
            .collect();
        let anime_list = create_list(&library_title, anime_items, app.mode == AppMode::Library);
        f.render_stateful_widget(anime_list, chunks[0], &mut app.library_anime_list_state);
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
    let Some(selected) = app.library_sort_popup else {
        return;
    };
    let actions = [
        ("Enter", "Застосувати", color_highlight()),
        ("Esc", "", color_dim()),
    ];
    let area = centered_fixed(f.area(), dialog_width_for(42, &actions), 11);
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
        .map(|sort| ListItem::new(format!("  {}", sort.label())))
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
    let Some(confirmation) = &app.pending_library_watched_confirmation else {
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
        let (anime_ids, status, total) = library_sidebar_tracking_context(app);
        text.push(mk_sep());
        text.extend(tracking_lines(app, &anime_ids, status, total));

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
        let (anime_ids, status, total) = library_sidebar_tracking_context(app);
        text.extend(tracking_lines(app, &anime_ids, status, total));
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
