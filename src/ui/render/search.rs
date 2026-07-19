//! Search-specific overlays.

use super::*;

pub(super) fn render_sort_popup(f: &mut Frame, app: &AppState) {
    let Some(selected) = app.search.ordering.popup else {
        return;
    };
    let actions = [
        ("Enter", "Застосувати / ↕", color_highlight()),
        ("Esc", "", color_dim()),
    ];
    let area = centered_fixed(f.area(), dialog_width_for(54, &actions), 10);
    let block = dialog_block(" Сортування пошуку ", color_highlight(), color_highlight());
    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let items = SearchSort::ALL
        .iter()
        .map(|sort| {
            let active = *sort == app.search.ordering.sort;
            let reversed = active && app.search.ordering.reversed;
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

fn search_sidebar_tracking_context(app: &AppState) -> (Vec<u32>, Option<u32>) {
    if app.focus == FocusPanel::SearchList {
        if let Some(group_index) = app.search.selected_group_index {
            let mut anime_ids = app
                .search
                .franchise_groups
                .get(group_index)
                .into_iter()
                .flatten()
                .filter_map(|index| app.search.results.get(*index).map(|anime| anime.id))
                .collect::<Vec<_>>();
            let mainline_ids = app
                .search
                .franchise_catalogs
                .get(group_index)
                .into_iter()
                .flat_map(|catalog| catalog.releases.iter())
                .filter(|release| {
                    release.availability == api::ReleaseAvailability::Available
                        && release.classification == api::ReleaseClassification::MainlineSeason
                })
                .filter_map(|release| release.anihub_id)
                .collect::<Vec<_>>();
            if !mainline_ids.is_empty() {
                anime_ids = mainline_ids;
            }
            anime_ids.sort_unstable();
            anime_ids.dedup();
            let totals = app
                .search
                .franchise_catalogs
                .get(group_index)
                .into_iter()
                .flat_map(|catalog| catalog.releases.iter())
                .filter(|release| {
                    release.classification == api::ReleaseClassification::MainlineSeason
                        && release.availability == api::ReleaseAvailability::Available
                })
                .filter_map(|release| release.episodes_count)
                .collect::<Vec<_>>();
            let total = (!totals.is_empty()).then(|| totals.into_iter().sum());
            return (anime_ids, total);
        }
    }

    if let Some(release) = selected_release_for_sidebar(app) {
        let anime_ids = release.anihub_id.into_iter().collect();
        return (anime_ids, release.episodes_count);
    }

    let anime_ids = app
        .sidebar_subject()
        .or_else(|| {
            app.search
                .selected_result_index
                .and_then(|index| app.search.results.get(index).map(|anime| anime.id))
        })
        .into_iter()
        .collect::<Vec<_>>();
    let total = anime_ids.first().and_then(|id| {
        app.details_cache
            .get(id)
            .and_then(|details| details.episodes_count)
            .or_else(|| {
                app.search
                    .results
                    .iter()
                    .find(|anime| anime.id == *id)
                    .and_then(|anime| anime.episodes_count)
            })
    });
    (anime_ids, total)
}

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

    let display_idx = app
        .sidebar_subject()
        .and_then(|anime_id| {
            app.search
                .results
                .iter()
                .position(|anime| anime.id == anime_id)
        })
        .or(app.search.selected_result_index);
    if display_idx.is_none()
        && selected_release_for_sidebar(app).is_none()
        && sidebar_details_override(app).is_none()
    {
        render_centered_sidebar_message(
            f,
            inner,
            app.activity_message
                .as_deref()
                .unwrap_or("Оберіть тайтл зі списку"),
        );
        return;
    }
    // Якщо поточний сезон — аніме не з пошуку (напр. S4 без ukr dub на сайті),
    // беремо `has_eng` з current_details, а не з search_results[representative].
    let has_eng = if selected_release_for_sidebar(app).is_some() {
        false
    } else if let Some(d) = sidebar_details_override(app) {
        d.title_english.is_some()
    } else {
        display_idx
            .and_then(|i| app.search.results.get(i))
            .and_then(|it| it.title_english.as_ref())
            .is_some()
    };
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

        render_sidebar_title_area(f, app, chunks[0], display_idx);

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

        render_sidebar_details_area(f, app, chunks[4], display_idx, false);
    } else {
        render_sidebar_details_area(f, app, inner, display_idx, true);
    }
}

fn render_sidebar_title_area(
    f: &mut Frame,
    app: &AppState,
    area: Rect,
    display_idx: Option<usize>,
) {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(release) = selected_release_for_sidebar(app) {
        lines.push(
            Line::from(Span::styled(
                truncate_with_ellipsis(&release.title, area.width as usize),
                Style::default()
                    .fg(color_secondary())
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        );
    } else if let Some(d) = sidebar_details_override(app) {
        lines.push(
            Line::from(Span::styled(
                truncate_with_ellipsis(&d.title_ukrainian, area.width as usize),
                Style::default()
                    .fg(color_secondary())
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        );
        if let Some(eng) = &d.title_english {
            lines.push(
                Line::from(Span::styled(
                    truncate_with_ellipsis(eng, area.width as usize),
                    Style::default().fg(color_dim()),
                ))
                .alignment(Alignment::Center),
            );
        }
    } else if let Some(idx) = display_idx {
        if let Some(item) = app.search.results.get(idx) {
            lines.push(
                Line::from(Span::styled(
                    truncate_with_ellipsis(&item.title_ukrainian, area.width as usize),
                    Style::default()
                        .fg(color_secondary())
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            if let Some(eng) = &item.title_english {
                lines.push(
                    Line::from(Span::styled(
                        truncate_with_ellipsis(eng, area.width as usize),
                        Style::default().fg(color_dim()),
                    ))
                    .alignment(Alignment::Center),
                );
            }
        }
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn render_sidebar_details_area(
    f: &mut Frame,
    app: &AppState,
    area: Rect,
    display_idx: Option<usize>,
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

    if let Some(release) = selected_release_for_sidebar(app) {
        if include_title {
            text.push(
                Line::from(Span::styled(
                    truncate_with_ellipsis(&release.title, area.width as usize),
                    Style::default()
                        .fg(color_secondary())
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            text.push(mk_sep());
        }
        let episodes = release.episodes_count.map(|episodes| {
            release
                .available_episodes
                .filter(|available| *available < episodes)
                .map_or_else(
                    || episodes.to_string(),
                    |available| format!("{available}/{episodes}"),
                )
        });
        text.push(compact_metadata_line(
            &release.anime_type,
            release.year,
            release.rating,
            episodes,
        ));
        let (anime_ids, total) = search_sidebar_tracking_context(app);
        text.push(mk_sep());
        text.extend(tracking_lines(app, &anime_ids, None, total));
        if release.availability == api::ReleaseAvailability::Unavailable {
            text.push(
                Line::from(Span::styled(
                    "⚠ Недоступно на AniHub",
                    Style::default()
                        .fg(color_error())
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
        }
        if let Some(genres) = &release.genres {
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
        f.render_widget(
            Paragraph::new(text).wrap(ratatui::widgets::Wrap { trim: true }),
            area,
        );
        return;
    }

    // Якщо обраний сезон — аніме не з пошуку (напр. S4 без ukr dub на anihub),
    // відображаємо дані прямо з current_details замість search_results[representative].
    if let Some(d) = sidebar_details_override(app) {
        if include_title {
            text.push(
                Line::from(Span::styled(
                    truncate_with_ellipsis(&d.title_ukrainian, area.width as usize),
                    Style::default()
                        .fg(color_secondary())
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            if let Some(eng) = &d.title_english {
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
            &d.anime_type,
            d.year,
            d.rating,
            d.episodes_count.map(|episodes| episodes.to_string()),
        ));
        let (anime_ids, total) = search_sidebar_tracking_context(app);
        text.push(mk_sep());
        text.extend(tracking_lines(app, &anime_ids, None, total));

        let studio_names = sidebar_studio_names(app, &d);
        let has_extra =
            d.genres.as_ref().is_some_and(|g| !g.is_empty()) || !studio_names.is_empty();
        if has_extra {
            text.push(mk_sep());
        }

        if !studio_names.is_empty() {
            text.push(Line::from(vec![
                Span::styled("Озвучка: ", Style::default().fg(color_dim())),
                Span::styled(
                    studio_names.join(", "),
                    Style::default().fg(color_success()),
                ),
            ]));
        }

        if let Some(genres) = &d.genres {
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
    } else if let Some(idx) = display_idx {
        if let Some(item) = app.search.results.get(idx) {
            if include_title {
                text.push(
                    Line::from(Span::styled(
                        truncate_with_ellipsis(&item.title_ukrainian, area.width as usize),
                        Style::default()
                            .fg(color_secondary())
                            .add_modifier(Modifier::BOLD),
                    ))
                    .alignment(Alignment::Center),
                );
                if let Some(eng) = &item.title_english {
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

            let details = app.details_cache.get(&item.id).or_else(|| {
                if sidebar_is_representative(app) {
                    app.content.current_details.clone()
                } else {
                    None
                }
            });
            text.push(compact_metadata_line(
                &item.anime_type,
                item.year,
                details.as_ref().and_then(|details| details.rating),
                details
                    .as_ref()
                    .and_then(|details| details.episodes_count)
                    .or(item.episodes_count)
                    .map(|episodes| episodes.to_string()),
            ));
            let (anime_ids, total) = search_sidebar_tracking_context(app);
            text.push(mk_sep());
            text.extend(tracking_lines(app, &anime_ids, None, total));

            if let Some(d) = details {
                let studio_names = sidebar_studio_names(app, &d);
                let has_extra =
                    d.genres.as_ref().is_some_and(|g| !g.is_empty()) || !studio_names.is_empty();
                if has_extra {
                    text.push(mk_sep());
                }

                if !studio_names.is_empty() {
                    text.push(Line::from(vec![
                        Span::styled("Озвучка: ", Style::default().fg(color_dim())),
                        Span::styled(
                            studio_names.join(", "),
                            Style::default().fg(color_success()),
                        ),
                    ]));
                }

                if let Some(genres) = &d.genres {
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
            } else if app.loading {
                text.push(
                    Line::from(Span::styled(
                        "Завантаження деталей…",
                        Style::default().fg(color_dim()),
                    ))
                    .alignment(Alignment::Center),
                );
            }
        }
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

pub(super) fn render_lists(f: &mut Frame, app: &mut AppState, area: Rect) {
    // Якщо лише один випуск — не показуємо окрему панель "Випуски".
    let single_season = !app.has_release_catalog()
        && app.unique_seasons().len() <= 1
        && matches!(app.focus, FocusPanel::DubbingList | FocusPanel::EpisodeList);
    let compact = area.width < 90;

    let constraints = match app.focus {
        FocusPanel::SearchList => vec![Constraint::Percentage(100)],
        FocusPanel::ReleaseList if compact => {
            vec![Constraint::Percentage(25), Constraint::Percentage(75)]
        }
        FocusPanel::ReleaseList => vec![Constraint::Percentage(50), Constraint::Percentage(50)],
        FocusPanel::DubbingList => {
            if single_season {
                if compact {
                    vec![Constraint::Percentage(25), Constraint::Percentage(75)]
                } else {
                    vec![Constraint::Percentage(50), Constraint::Percentage(50)]
                }
            } else if compact {
                vec![
                    Constraint::Percentage(15),
                    Constraint::Percentage(25),
                    Constraint::Percentage(60),
                ]
            } else {
                vec![
                    Constraint::Percentage(33),
                    Constraint::Percentage(34),
                    Constraint::Percentage(33),
                ]
            }
        }
        FocusPanel::EpisodeList => {
            if single_season {
                if compact {
                    vec![
                        Constraint::Percentage(15),
                        Constraint::Percentage(25),
                        Constraint::Percentage(60),
                    ]
                } else {
                    vec![
                        Constraint::Percentage(33),
                        Constraint::Percentage(34),
                        Constraint::Percentage(33),
                    ]
                }
            } else if compact {
                vec![
                    Constraint::Percentage(10),
                    Constraint::Percentage(15),
                    Constraint::Percentage(20),
                    Constraint::Percentage(55),
                ]
            } else {
                vec![
                    Constraint::Percentage(25),
                    Constraint::Percentage(25),
                    Constraint::Percentage(25),
                    Constraint::Percentage(25),
                ]
            }
        }
    };
    let chunk_count = constraints.len();

    let list_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    if chunk_count >= 1 {
        let list_width = list_chunks[0].width.saturating_sub(6) as usize;
        let search_title = format!(
            " Результати · {} {} ",
            app.search.ordering.sort.label(),
            app.search
                .ordering
                .sort
                .direction_symbol(app.search.ordering.reversed)
        );
        let mut items: Vec<ListItem> = Vec::new();
        for (group_index, group) in app.search.franchise_groups.iter().enumerate() {
            let Some(&representative_index) = group.first() else {
                continue;
            };
            let name = app.search.franchise_catalogs.get(group_index).map_or_else(
                || api::franchise_display_name(&app.search.results, group),
                |catalog| catalog.canonical_title.as_str(),
            );
            let rep = &app.search.results[representative_index];
            let (tv, other) = count_seasons(&app.search.results, group);
            let mut metadata = Vec::new();
            if tv > 0 {
                metadata.push(season_count_label(tv));
            } else if other > 0 {
                metadata.push(format!("{other} дод."));
            }
            let episodes = rep.episodes_count.or_else(|| {
                app.details_cache
                    .get(&rep.id)
                    .and_then(|details| details.episodes_count)
            });
            if let Some(episodes) = episodes {
                metadata.push(format!("{episodes} сер."));
            } else if let Some(year) = rep.year {
                metadata.push(year.to_string());
            }
            let title = label_with_metadata(name, &metadata);
            items.push(ListItem::new(truncate_with_ellipsis(&title, list_width)));
        }

        if items.is_empty() {
            let (message, loading) = if let Some(activity) = &app.activity_message {
                (activity.as_str(), true)
            } else if app.search.last_query.is_empty() {
                ("Натисніть / щоб шукати", false)
            } else {
                ("Нічого не знайдено", false)
            };
            render_list_message(
                f,
                list_chunks[0],
                &search_title,
                app.focus == FocusPanel::SearchList,
                message,
                loading,
            );
        } else {
            let list = create_list(&search_title, items, app.focus == FocusPanel::SearchList);
            f.render_stateful_widget(list, list_chunks[0], &mut app.search.result_list_state);
        }
    }

    // Визначаємо індекси чанків з урахуванням single_season
    // single_season: [SearchList, DubbingList, EpisodeList?]
    // normal:        [SearchList, ReleaseList?, DubbingList?, EpisodeList?]
    let season_chunk_idx: Option<usize> = if single_season {
        None
    } else if chunk_count >= 2 {
        Some(1)
    } else {
        None
    };
    let dubbing_chunk_idx: Option<usize> = if single_season {
        if chunk_count >= 2 { Some(1) } else { None }
    } else if chunk_count >= 3 {
        Some(2)
    } else {
        None
    };
    let episode_chunk_idx: Option<usize> = if single_season {
        if chunk_count >= 3 { Some(2) } else { None }
    } else if chunk_count >= 4 {
        Some(3)
    } else {
        None
    };

    if let Some(idx) = season_chunk_idx {
        if let Some(catalog) = app.selected_franchise_catalog() {
            let (items, release_rows) =
                release_catalog_items(catalog, list_chunks[idx].width.saturating_sub(6) as usize);
            if items.is_empty() {
                render_list_message(
                    f,
                    list_chunks[idx],
                    " Випуски ",
                    app.focus == FocusPanel::ReleaseList,
                    if app.loading {
                        "Завантаження випусків…"
                    } else {
                        "Випусків не знайдено"
                    },
                    app.loading,
                );
            } else {
                let mut visual_state = ratatui::widgets::ListState::default();
                visual_state.select(
                    app.content
                        .season_list_state
                        .selected()
                        .and_then(|release_index| release_rows.get(release_index).copied()),
                );
                let list = create_list(" Випуски ", items, app.focus == FocusPanel::ReleaseList);
                f.render_stateful_widget(list, list_chunks[idx], &mut visual_state);
            }
        } else {
            let items: Vec<ListItem> = app
                .unique_seasons()
                .iter()
                .map(|&sn| {
                    let count = app.dubbing_choices_for_season(sn).len();
                    let year_str = season_year(app, sn).map(|y| y.to_string());
                    let mut metadata = year_str.into_iter().collect::<Vec<_>>();
                    if count > 0 {
                        metadata.push(dubbing_count_label(count));
                    }
                    let label = label_with_metadata(&format!("Сезон {sn}"), &metadata);
                    let marker = season_marker_for_search(app, sn);
                    ListItem::new(with_right_marker(
                        &label,
                        marker.unwrap_or(""),
                        list_chunks[idx].width.saturating_sub(6) as usize,
                    ))
                })
                .collect();
            if items.is_empty() {
                render_list_message(
                    f,
                    list_chunks[idx],
                    " Випуски ",
                    app.focus == FocusPanel::ReleaseList,
                    if app.loading {
                        "Завантаження випусків…"
                    } else {
                        "Випусків не знайдено"
                    },
                    app.loading,
                );
            } else {
                let list = create_list(" Випуски ", items, app.focus == FocusPanel::ReleaseList);
                f.render_stateful_widget(
                    list,
                    list_chunks[idx],
                    &mut app.content.season_list_state,
                );
            }
        }
    }

    if let Some(idx) = dubbing_chunk_idx {
        let items: Vec<ListItem> = if let Some(sn) = app.selected_season_num() {
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
        if items.is_empty() {
            let (message, loading) = if app.loading {
                ("Завантаження озвучок…", true)
            } else if app.selected_season_num().is_none() {
                ("Оберіть випуск", false)
            } else {
                ("Озвучок не знайдено", false)
            };
            render_list_message(
                f,
                list_chunks[idx],
                " Озвучки ",
                app.focus == FocusPanel::DubbingList,
                message,
                loading,
            );
        } else {
            let list = create_list(" Озвучки ", items, app.focus == FocusPanel::DubbingList);
            f.render_stateful_widget(list, list_chunks[idx], &mut app.content.dubbing_list_state);
        }
    }

    if let Some(idx) = episode_chunk_idx {
        let items: Vec<ListItem> = if app
            .selected_dubbing_choice()
            .is_some_and(|choice| choice.is_moonanime())
        {
            let episode_owner = selected_search_anime_id(app);
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
                    if episode_owner.zip(season).is_some_and(|(anime_id, season)| {
                        episode_is_watched(app, anime_id, season, episode.episode_number())
                    }) {
                        metadata.push("✓".to_string());
                    }
                    ListItem::new(label_with_metadata(&label, &metadata))
                })
                .collect()
        } else if let Some(studio) = app.selected_studio() {
            let episode_owner = selected_search_anime_id(app);
            studio
                .episodes
                .iter()
                .map(|ep| {
                    // Показуємо display_episode_number якщо відрізняється від episode_number
                    // (наприклад 12.5 для "Серія 12 Частина 2")
                    let ep_label = if let Some(disp) = ep.display_episode_number {
                        let whole = ep.episode_number as f32;
                        if (disp - whole).abs() > 0.01 {
                            format!("{}", disp)
                        } else {
                            format!("{}", ep.episode_number)
                        }
                    } else {
                        format!("{}", ep.episode_number)
                    };
                    // Якщо title містить "частина" або "part" — додаємо як підказку
                    let title_lower = ep.title.to_lowercase();
                    if title_lower.contains("частина")
                        || title_lower.contains("chastina")
                        || title_lower.contains("part")
                    {
                        // Витягуємо лише хвіст після останнього " - " або "(частина...)"
                        let suffix = ep
                            .title
                            .rfind(" - ")
                            .map(|p| ep.title[p + 3..].trim())
                            .or_else(|| ep.title.rfind('(').map(|p| ep.title[p..].trim()))
                            .unwrap_or("");
                        if !suffix.is_empty() {
                            let label = format!("Серія {} ({})", ep_label, suffix);
                            let mut metadata = Vec::new();
                            if let Some(t) = episode_owner.and_then(|anime_id| {
                                episode_progress_timestamp(
                                    app,
                                    anime_id,
                                    studio.season_number,
                                    ep.episode_number,
                                )
                            }) {
                                metadata.push(format!("⏱ {}", format_elapsed_timestamp(t)));
                            }
                            if episode_owner.is_some_and(|anime_id| {
                                episode_is_watched(
                                    app,
                                    anime_id,
                                    studio.season_number,
                                    ep.episode_number,
                                )
                            }) {
                                metadata.push("✓".to_string());
                            }
                            return ListItem::new(label_with_metadata(&label, &metadata));
                        }
                    }
                    let label = format!("Серія {}", ep_label);
                    let mut metadata = Vec::new();
                    if let Some(t) = episode_owner.and_then(|anime_id| {
                        episode_progress_timestamp(
                            app,
                            anime_id,
                            studio.season_number,
                            ep.episode_number,
                        )
                    }) {
                        metadata.push(format!("⏱ {}", format_elapsed_timestamp(t)));
                    }
                    if episode_owner.is_some_and(|anime_id| {
                        episode_is_watched(app, anime_id, studio.season_number, ep.episode_number)
                    }) {
                        metadata.push("✓".to_string());
                    }
                    ListItem::new(label_with_metadata(&label, &metadata))
                })
                .collect()
        } else {
            vec![]
        };
        if items.is_empty() {
            let (message, loading) = if app.loading {
                ("Завантаження серій…", true)
            } else if app.selected_dubbing_choice().is_none() {
                ("Оберіть озвучку", false)
            } else {
                ("Серій не знайдено", false)
            };
            render_list_message(
                f,
                list_chunks[idx],
                " Серії ",
                app.focus == FocusPanel::EpisodeList,
                message,
                loading,
            );
        } else {
            let list = create_list(" Серії ", items, app.focus == FocusPanel::EpisodeList);
            f.render_stateful_widget(list, list_chunks[idx], &mut app.content.episode_list_state);
        }
    }
}
