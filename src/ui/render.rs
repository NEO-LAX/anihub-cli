use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use ratatui_image::{StatefulImage, protocol::StatefulProtocol};

use crate::api;
use crate::ui::app::{AppMode, AppState, FocusPanel, StatusKind};

const COLOR_PRIMARY: Color = Color::Rgb(147, 51, 234);
const COLOR_SECONDARY: Color = Color::Rgb(168, 85, 247);
const COLOR_BG_DARK: Color = Color::Rgb(17, 24, 39);
const COLOR_TEXT: Color = Color::Rgb(243, 244, 246);
const COLOR_HIGHLIGHT: Color = Color::Rgb(59, 130, 246);
const COLOR_ERROR: Color = Color::Rgb(239, 68, 68);
const COLOR_DIM: Color = Color::Rgb(107, 114, 128);

pub fn render(f: &mut Frame, app: &mut AppState) {
    let size = f.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(size);

    render_header(f, app, main_chunks[0]);

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(main_chunks[1]);

    render_sidebar(f, app, body_chunks[0]);
    render_lists(f, app, body_chunks[1]);
    render_status_bar(f, app, main_chunks[2]);

    if let Some((message, StatusKind::Error)) = app.status_message.clone() {
        let msg = format!("{}\n\nEsc — закрити", message);
        render_popup(f, "Помилка", &msg, COLOR_ERROR);
    } else if let Some((_, anime_title)) = app.pending_delete_confirmation.clone() {
        let msg = format!("Видалити прогрес для\n{}\n\n[y/n]", anime_title);
        render_popup(f, "Підтвердження", &msg, COLOR_ERROR);
    }
}

fn render_header(f: &mut Frame, app: &AppState, area: Rect) {
    if app.is_library_mode() {
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                "Бібліотека",
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Натисни Esc щоб повернутись",
                Style::default().fg(COLOR_DIM),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_PRIMARY))
                .title(Span::styled(
                    " ANIHUB-CLI ",
                    Style::default()
                        .fg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ))
                .title_alignment(Alignment::Center),
        )
        .alignment(Alignment::Center)
        .style(Style::default().bg(COLOR_BG_DARK));
        f.render_widget(title, area);
        return;
    }

    let border_style = if app.mode == AppMode::SearchInput {
        Style::default()
            .fg(COLOR_HIGHLIGHT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_PRIMARY)
    };

    let search_text = if app.search_query.is_empty() && app.mode != AppMode::SearchInput {
        vec![Span::styled(
            "Натисніть '/' для пошуку аніме...",
            Style::default().fg(Color::DarkGray),
        )]
    } else {
        vec![
            Span::styled("🔍 ", Style::default().fg(COLOR_SECONDARY)),
            Span::styled(&app.search_query, Style::default().fg(COLOR_TEXT)),
        ]
    };

    let search_align = if app.search_query.is_empty() && app.mode != AppMode::SearchInput {
        Alignment::Center
    } else {
        Alignment::Left
    };

    let search_widget = Paragraph::new(Line::from(search_text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(Span::styled(
                    " ANIHUB-CLI ",
                    Style::default()
                        .fg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ))
                .title_alignment(Alignment::Center),
        )
        .style(Style::default().bg(COLOR_BG_DARK))
        .alignment(search_align);

    f.render_widget(search_widget, area);

    if app.mode == AppMode::SearchInput {
        #[allow(clippy::cast_possible_truncation)]
        f.set_cursor_position((
            area.x + 4 + app.search_query.chars().count() as u16,
            area.y + 1,
        ));
    }
}

fn render_sidebar(f: &mut Frame, app: &mut AppState, area: Rect) {
    if app.is_library_mode() {
        render_library_sidebar(f, app, area);
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_PRIMARY))
        .title(Span::styled(
            " Інформація ",
            Style::default().fg(COLOR_PRIMARY),
        ))
        .title_alignment(Alignment::Center);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let display_idx = app.sidebar_anime_idx.or(app.selected_result_index);
    let has_eng = display_idx
        .and_then(|i| app.search_results.get(i))
        .and_then(|it| it.title_english.as_ref())
        .is_some();
    let title_h: u16 = if has_eng { 2 } else { 1 };

    if app.current_poster.is_some() && inner.height > title_h + 5 {
        let poster_h = ((inner.height.saturating_sub(title_h + 2)) * 40 / 100).max(3);
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

        let sep_style = Style::default().fg(COLOR_DIM);
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
    if let Some(idx) = display_idx {
        if let Some(item) = app.search_results.get(idx) {
            lines.push(
                Line::from(Span::styled(
                    item.title_ukrainian.as_str(),
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            if let Some(eng) = &item.title_english {
                lines.push(
                    Line::from(Span::styled(eng.as_str(), Style::default().fg(COLOR_DIM)))
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
            Style::default().fg(COLOR_DIM),
        ))
    };
    let mut text: Vec<Line> = Vec::new();

    if let Some(idx) = display_idx {
        if let Some(item) = app.search_results.get(idx) {
            if include_title {
                text.push(
                    Line::from(Span::styled(
                        item.title_ukrainian.as_str(),
                        Style::default()
                            .fg(COLOR_SECONDARY)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .alignment(Alignment::Center),
                );
                if let Some(eng) = &item.title_english {
                    text.push(
                        Line::from(Span::styled(eng.as_str(), Style::default().fg(COLOR_DIM)))
                            .alignment(Alignment::Center),
                    );
                }
                text.push(mk_sep());
            }

            // Кількість сезонів у групі (тільки в SearchList)
            if app.sidebar_anime_idx.is_none() {
                if let Some(g_idx) = app.selected_group_index {
                    if let Some(group) = app.franchise_groups.get(g_idx) {
                        let (tv, other) = count_seasons(&app.search_results, group);
                        if tv > 1 || other > 0 {
                            let label = if tv > 1 && other > 0 {
                                format!("{} сез. + {} спец.", tv, other)
                            } else if tv > 1 {
                                format!("{} сезони", tv)
                            } else {
                                format!("1 сез. + {} спец.", other)
                            };
                            text.push(
                                Line::from(vec![
                                    Span::styled("Сезонів: ", Style::default().fg(COLOR_DIM)),
                                    Span::styled(
                                        label,
                                        Style::default()
                                            .fg(COLOR_SECONDARY)
                                            .add_modifier(Modifier::BOLD),
                                    ),
                                ])
                                .alignment(Alignment::Center),
                            );
                        }
                    }
                }
            }

            text.push(
                Line::from(vec![
                    Span::styled("Тип: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(item.anime_type.as_str(), Style::default().fg(COLOR_TEXT)),
                ])
                .alignment(Alignment::Center),
            );
            text.push(
                Line::from(vec![
                    Span::styled("Рік: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(
                        item.year
                            .map(|y| y.to_string())
                            .unwrap_or_else(|| "—".to_string()),
                        Style::default().fg(COLOR_TEXT),
                    ),
                ])
                .alignment(Alignment::Center),
            );

            let details = app.details_cache.get(&item.id).or_else(|| {
                if app.sidebar_anime_idx.is_none() {
                    app.current_details.as_ref()
                } else {
                    None
                }
            });

            if let Some(d) = details {
                if let Some(rating) = d.rating {
                    text.push(
                        Line::from(vec![
                            Span::styled("Рейтинг: ", Style::default().fg(COLOR_DIM)),
                            Span::styled(
                                format!("{:.1} ⭐", rating),
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ])
                        .alignment(Alignment::Center),
                    );
                }
                if let Some(ep_count) = d.episodes_count {
                    text.push(
                        Line::from(vec![
                            Span::styled("Серій: ", Style::default().fg(COLOR_DIM)),
                            Span::styled(ep_count.to_string(), Style::default().fg(COLOR_TEXT)),
                        ])
                        .alignment(Alignment::Center),
                    );
                }

                let has_extra = d.genres.as_ref().map_or(false, |g| !g.is_empty())
                    || d.dubbing_studios.as_ref().map_or(false, |s| !s.is_empty());
                if has_extra {
                    text.push(mk_sep());
                }

                if let Some(studios) = &d.dubbing_studios {
                    if !studios.is_empty() {
                        let s = studios
                            .iter()
                            .map(|s| s.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        text.push(Line::from(vec![
                            Span::styled("Озвучка: ", Style::default().fg(COLOR_DIM)),
                            Span::styled(s, Style::default().fg(Color::Green)),
                        ]));
                    }
                }

                if let Some(genres) = &d.genres {
                    if !genres.is_empty() {
                        text.push(Line::from(""));
                        text.push(Line::from(vec![
                            Span::styled("Жанри: ", Style::default().fg(COLOR_DIM)),
                            Span::styled(genres.join(" · "), Style::default().fg(COLOR_HIGHLIGHT)),
                        ]));
                    }
                }
            }
        }
    } else {
        text.push(
            Line::from(Span::styled(
                "Виберіть аніме зі списку...",
                Style::default().fg(COLOR_DIM),
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

fn render_status_bar(f: &mut Frame, app: &AppState, area: Rect) {
    let state_area = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(36)])
        .split(area)[1];

    let shortcuts = if app.is_library_mode() {
        match app.mode {
            AppMode::Library => "c продовжити  d видалити  ↑↓ список  →/Enter сезони",
            AppMode::LibrarySeason => "x toggle сезон  ↑↓ сезон  →/Enter озвучки  Esc/← назад",
            AppMode::LibraryDubbing => "x toggle сезон  ↑↓ озвучка  →/Enter серії  Esc/← назад",
            AppMode::LibraryEpisode => "x toggle серію  ↑↓ серія  Esc/← назад  q вихід",
            _ => "",
        }
    } else {
        match app.focus {
            FocusPanel::SearchList => "/ пошук  l бібліотека  ↑↓ список  →/Enter вибір  q вихід",
            FocusPanel::SeasonList => "↑↓ сезон  →/Enter озвучки  ←/Esc назад  q вихід",
            FocusPanel::DubbingList => "↑↓ озвучка  →/Enter серії  ←/Esc назад  q вихід",
            FocusPanel::EpisodeList => "↑↓ список  Enter відтворити  ←/Esc назад  q вихід",
        }
    };

    let state = app
        .status_message
        .as_ref()
        .and_then(|(message, kind)| match kind {
            StatusKind::Info => Some(message.clone()),
            StatusKind::Error => None,
        })
        .unwrap_or_else(|| {
            if app.is_playing {
                "▶ Відтворюється в mpv".to_string()
            } else if app.loading {
                "⟳ Завантаження...".to_string()
            } else if app.prefetching {
                "⟳ Кешування...".to_string()
            } else {
                String::new()
            }
        });

    f.render_widget(
        Paragraph::new(shortcuts)
            .style(Style::default().fg(COLOR_DIM).bg(COLOR_BG_DARK))
            .alignment(Alignment::Center),
        area,
    );
    f.render_widget(
        Paragraph::new(state)
            .style(Style::default().fg(COLOR_SECONDARY).bg(COLOR_BG_DARK))
            .alignment(Alignment::Right),
        state_area,
    );
}

fn render_lists(f: &mut Frame, app: &mut AppState, area: Rect) {
    if app.is_library_mode() {
        render_library_lists(f, app, area);
        return;
    }

    let constraints = match app.focus {
        FocusPanel::SearchList => vec![Constraint::Percentage(100)],
        FocusPanel::SeasonList => vec![Constraint::Percentage(50), Constraint::Percentage(50)],
        FocusPanel::DubbingList => vec![
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ],
        FocusPanel::EpisodeList => vec![
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ],
    };
    let chunk_count = constraints.len();

    let list_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    if chunk_count >= 1 {
        let list_width = list_chunks[0].width.saturating_sub(6) as usize;
        let mut items: Vec<ListItem> = Vec::new();
        for group in &app.franchise_groups {
            let name = api::franchise_display_name(&app.search_results, group);
            let rep = &app.search_results[api::representative_idx(&app.search_results, group)];
            let (tv, other) = count_seasons(&app.search_results, group);
            let title = if tv > 1 && other > 0 {
                format!("{} ({} сез. + {} спец.)", name, tv, other)
            } else if tv > 1 {
                format!("{} ({} сез.)", name, tv)
            } else {
                match rep.year {
                    Some(y) => format!("{} ({})", name, y),
                    None => name.to_string(),
                }
            };
            let marker = if franchise_is_complete(app, group) {
                "✓"
            } else {
                ""
            };
            let mut lines = vec![Line::from(with_right_marker(&title, marker, list_width))];
            if let Some(progress) = latest_progress_for_group(app, group) {
                lines.push(Line::from(Span::styled(
                    format!(
                        "⏱ Сезон {} · Серія {} · {}",
                        progress.season,
                        progress.episode,
                        format_timestamp(progress.timestamp)
                    ),
                    Style::default().fg(COLOR_HIGHLIGHT),
                )));
            }
            if let Some(eng) = &rep.title_english {
                lines.push(Line::from(Span::styled(
                    eng.as_str(),
                    Style::default().fg(COLOR_DIM),
                )));
            }
            items.push(ListItem::new(lines));
        }

        let list = create_list(
            " Результати пошуку ",
            items,
            app.focus == FocusPanel::SearchList,
        );
        f.render_stateful_widget(list, list_chunks[0], &mut app.result_list_state);
    }

    if chunk_count >= 2 {
        let seasons = app.unique_seasons();
        let items: Vec<ListItem> = seasons
            .iter()
            .map(|&sn| {
                let count = app.studios_for_season(sn).len();
                let label = if count > 1 {
                    format!("Сезон {} ({} озвучок)", sn, count)
                } else {
                    format!("Сезон {}", sn)
                };
                let marker = season_marker_for_search(app, sn);
                ListItem::new(with_right_marker(
                    &label,
                    marker.unwrap_or(""),
                    list_chunks[1].width.saturating_sub(6) as usize,
                ))
            })
            .collect();
        let list = create_list(" Сезони ", items, app.focus == FocusPanel::SeasonList);
        f.render_stateful_widget(list, list_chunks[1], &mut app.season_list_state);
    }

    if chunk_count >= 3 {
        let items: Vec<ListItem> = if let Some(sn) = app.selected_season_num() {
            app.studios_for_season(sn)
                .iter()
                .map(|s| ListItem::new(format!("{} ({} серій)", s.studio_name, s.episodes_count)))
                .collect()
        } else {
            vec![]
        };
        let list = create_list(" Озвучки ", items, app.focus == FocusPanel::DubbingList);
        f.render_stateful_widget(list, list_chunks[2], &mut app.dubbing_list_state);
    }

    if chunk_count >= 4 {
        let items: Vec<ListItem> = if let Some(studio) = app.selected_studio() {
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
                            let mut label = format!("Серія {} ({})", ep_label, suffix);
                            if let Some(t) = episode_owner.and_then(|anime_id| {
                                episode_progress_timestamp(
                                    app,
                                    anime_id,
                                    studio.season_number,
                                    ep.episode_number,
                                )
                            }) {
                                label.push_str(&format!(" · ⏱ {}", format_elapsed_timestamp(t)));
                            }
                            let marker = if episode_owner.is_some_and(|anime_id| {
                                episode_is_watched(
                                    app,
                                    anime_id,
                                    studio.season_number,
                                    ep.episode_number,
                                )
                            }) {
                                "✓"
                            } else {
                                ""
                            };
                            return ListItem::new(with_right_marker(
                                &label,
                                marker,
                                list_chunks[3].width.saturating_sub(6) as usize,
                            ));
                        }
                    }
                    let mut label = format!("Серія {}", ep_label);
                    if let Some(t) = episode_owner.and_then(|anime_id| {
                        episode_progress_timestamp(
                            app,
                            anime_id,
                            studio.season_number,
                            ep.episode_number,
                        )
                    }) {
                        label.push_str(&format!(" · ⏱ {}", format_elapsed_timestamp(t)));
                    }
                    let marker = if episode_owner.is_some_and(|anime_id| {
                        episode_is_watched(app, anime_id, studio.season_number, ep.episode_number)
                    }) {
                        "✓"
                    } else {
                        ""
                    };
                    ListItem::new(with_right_marker(
                        &label,
                        marker,
                        list_chunks[3].width.saturating_sub(6) as usize,
                    ))
                })
                .collect()
        } else {
            vec![]
        };
        let list = create_list(" Серії ", items, app.focus == FocusPanel::EpisodeList);
        f.render_stateful_widget(list, list_chunks[3], &mut app.episode_list_state);
    }
}

fn render_library_sidebar(f: &mut Frame, app: &mut AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_PRIMARY))
        .title(Span::styled(
            " Інформація ",
            Style::default().fg(COLOR_PRIMARY),
        ))
        .title_alignment(Alignment::Center);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let has_eng = app
        .current_details
        .as_ref()
        .and_then(|d| d.title_english.as_ref())
        .is_some();
    let title_h: u16 = if has_eng { 2 } else { 1 };

    if app.current_poster.is_some() && inner.height > title_h + 5 {
        let poster_h = ((inner.height.saturating_sub(title_h + 2)) * 40 / 100).max(3);
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

        let sep_style = Style::default().fg(COLOR_DIM);
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

fn render_library_lists(f: &mut Frame, app: &mut AppState, area: Rect) {
    let constraints = match app.mode {
        AppMode::Library => vec![Constraint::Percentage(100)],
        AppMode::LibrarySeason => vec![Constraint::Percentage(50), Constraint::Percentage(50)],
        AppMode::LibraryDubbing => vec![
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ],
        AppMode::LibraryEpisode => vec![
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

    let anime_items: Vec<ListItem> = app
        .library_items
        .iter()
        .map(|item| {
            let marker = if library_anime_is_complete(item) {
                "✓"
            } else {
                ""
            };
            let line_1 = with_right_marker(
                &item.anime_title,
                marker,
                chunks[0].width.saturating_sub(6) as usize,
            );
            let line_2 = format!(
                "Сезон {} · Серія {} · ⏱ {}",
                item.latest_progress.season,
                item.latest_progress.episode,
                format_timestamp(item.latest_progress.timestamp),
            );
            ListItem::new(vec![
                Line::from(line_1),
                Line::from(Span::styled(line_2, Style::default().fg(COLOR_DIM))),
            ])
        })
        .collect();
    let anime_list = create_list(" Бібліотека ", anime_items, app.mode == AppMode::Library);
    f.render_stateful_widget(anime_list, chunks[0], &mut app.library_anime_list_state);

    if chunks.len() >= 2 {
        let season_items: Vec<ListItem> = app
            .library_season_numbers()
            .iter()
            .map(|&season_num| {
                let count = app.studios_for_season(season_num).len();
                let label = if count > 1 {
                    format!("Сезон {} ({} озвучок)", season_num, count)
                } else {
                    format!("Сезон {}", season_num)
                };
                let marker = app
                    .library_selected_anime_id()
                    .is_some_and(|anime_id| season_is_complete(app, anime_id, season_num))
                    .then_some("✓")
                    .unwrap_or("");
                ListItem::new(with_right_marker(
                    &label,
                    marker,
                    chunks[1].width.saturating_sub(6) as usize,
                ))
            })
            .collect();
        let season_list = create_list(" Сезони ", season_items, app.mode == AppMode::LibrarySeason);
        f.render_stateful_widget(season_list, chunks[1], &mut app.season_list_state);
    }

    if chunks.len() >= 3 {
        let dubbing_items: Vec<ListItem> = if let Some(sn) = app.selected_season_num() {
            app.studios_for_season(sn)
                .iter()
                .map(|s| ListItem::new(format!("{} ({} серій)", s.studio_name, s.episodes_count)))
                .collect()
        } else {
            vec![]
        };
        let dubbing_list = create_list(
            " Озвучки ",
            dubbing_items,
            app.mode == AppMode::LibraryDubbing,
        );
        f.render_stateful_widget(dubbing_list, chunks[2], &mut app.dubbing_list_state);
    }

    if chunks.len() >= 4 {
        let episode_items: Vec<ListItem> = if let Some(studio) = app.selected_studio() {
            let anime_id = app.library_selected_anime_id();
            studio
                .episodes
                .iter()
                .map(|episode| {
                    let mut label = format!("Серія {}", episode.episode_number);
                    if let Some(t) = anime_id.and_then(|id| {
                        episode_progress_timestamp(
                            app,
                            id,
                            studio.season_number,
                            episode.episode_number,
                        )
                    }) {
                        label.push_str(&format!(" · ⏱ {}", format_elapsed_timestamp(t)));
                    }
                    let marker = anime_id
                        .is_some_and(|id| {
                            episode_is_watched(
                                app,
                                id,
                                studio.season_number,
                                episode.episode_number,
                            )
                        })
                        .then_some("✓")
                        .unwrap_or("");
                    ListItem::new(with_right_marker(
                        &label,
                        marker,
                        chunks[3].width.saturating_sub(6) as usize,
                    ))
                })
                .collect()
        } else {
            vec![]
        };
        let episode_list = create_list(
            " Серії ",
            episode_items,
            app.mode == AppMode::LibraryEpisode,
        );
        f.render_stateful_widget(episode_list, chunks[3], &mut app.episode_list_state);
    }
}

fn render_library_sidebar_title_area(f: &mut Frame, app: &AppState, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some(details) = &app.current_details {
        lines.push(
            Line::from(Span::styled(
                details.title_ukrainian.as_str(),
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        );
        if let Some(eng) = &details.title_english {
            lines.push(
                Line::from(Span::styled(eng.as_str(), Style::default().fg(COLOR_DIM)))
                    .alignment(Alignment::Center),
            );
        }
    } else if let Some(anime) = app.library_selected_anime() {
        lines.push(
            Line::from(Span::styled(
                anime.anime_title.as_str(),
                Style::default()
                    .fg(COLOR_SECONDARY)
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
            Style::default().fg(COLOR_DIM),
        ))
    };
    let mut text: Vec<Line> = Vec::new();

    if let Some(details) = &app.current_details {
        if include_title {
            text.push(
                Line::from(Span::styled(
                    details.title_ukrainian.as_str(),
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            if let Some(eng) = &details.title_english {
                text.push(
                    Line::from(Span::styled(eng.as_str(), Style::default().fg(COLOR_DIM)))
                        .alignment(Alignment::Center),
                );
            }
            text.push(mk_sep());
        }

        text.push(
            Line::from(vec![
                Span::styled("Тип: ", Style::default().fg(COLOR_DIM)),
                Span::styled(details.anime_type.as_str(), Style::default().fg(COLOR_TEXT)),
            ])
            .alignment(Alignment::Center),
        );
        text.push(
            Line::from(vec![
                Span::styled("Рік: ", Style::default().fg(COLOR_DIM)),
                Span::styled(
                    details
                        .year
                        .map(|y| y.to_string())
                        .unwrap_or_else(|| "—".to_string()),
                    Style::default().fg(COLOR_TEXT),
                ),
            ])
            .alignment(Alignment::Center),
        );

        if let Some(rating) = details.rating {
            text.push(
                Line::from(vec![
                    Span::styled("Рейтинг: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(
                        format!("{:.1} ⭐", rating),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
                .alignment(Alignment::Center),
            );
        }
        if let Some(ep_count) = details.episodes_count {
            text.push(
                Line::from(vec![
                    Span::styled("Серій: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(ep_count.to_string(), Style::default().fg(COLOR_TEXT)),
                ])
                .alignment(Alignment::Center),
            );
        }

        let has_extra = details.genres.as_ref().map_or(false, |g| !g.is_empty())
            || details
                .dubbing_studios
                .as_ref()
                .map_or(false, |s| !s.is_empty());
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
                    Span::styled("Озвучка: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(s, Style::default().fg(Color::Green)),
                ]));
            }
        }

        if let Some(genres) = &details.genres {
            if !genres.is_empty() {
                text.push(Line::from(""));
                text.push(Line::from(vec![
                    Span::styled("Жанри: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(genres.join(" · "), Style::default().fg(COLOR_HIGHLIGHT)),
                ]));
            }
        }

        if let Some(anime) = app.library_selected_anime() {
            let latest = &anime.latest_progress;
            text.push(mk_sep());
            text.push(
                Line::from(vec![
                    Span::styled("Останнє: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(
                        format!("Сезон {} · Серія {}", latest.season, latest.episode),
                        Style::default().fg(COLOR_TEXT),
                    ),
                ])
                .alignment(Alignment::Center),
            );
            text.push(
                Line::from(vec![
                    Span::styled("Таймінг: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(
                        format!("⏱ {}", format_timestamp(latest.timestamp)),
                        Style::default().fg(COLOR_TEXT),
                    ),
                ])
                .alignment(Alignment::Center),
            );
        }
    } else {
        text.push(
            Line::from(Span::styled(
                "Виберіть аніме зі списку...",
                Style::default().fg(COLOR_DIM),
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

fn format_timestamp(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

fn format_elapsed_timestamp(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    if total >= 60 {
        format!("{}:{:02}", total / 60, total % 60)
    } else {
        format!("{}с", total)
    }
}

fn with_right_marker(left: &str, marker: &str, width: usize) -> String {
    if marker.is_empty() {
        return left.to_string();
    }

    let left_len = left.chars().count();
    let marker_len = marker.chars().count();
    let spaces = width.saturating_sub(left_len + marker_len).max(1);
    format!("{left}{}{marker}", " ".repeat(spaces))
}

fn latest_progress_for_group<'a>(
    app: &'a AppState,
    group: &[usize],
) -> Option<&'a crate::storage::WatchProgress> {
    app.history
        .progress
        .values()
        .filter(|progress| {
            group.iter().any(|&idx| {
                app.search_results
                    .get(idx)
                    .is_some_and(|item| item.id == progress.anime_id)
            })
        })
        .max_by_key(|progress| progress.updated_at)
}

fn franchise_is_complete(app: &AppState, group: &[usize]) -> bool {
    !group.is_empty()
        && group.iter().all(|&idx| {
            app.search_results
                .get(idx)
                .is_some_and(|item| anime_is_complete(app, item.id))
        })
}

fn season_marker_for_search<'a>(app: &'a AppState, season_num: u32) -> Option<&'a str> {
    let anime_id = app
        .current_sources
        .as_ref()
        .and_then(|sources| {
            sources
                .ashdi
                .iter()
                .position(|studio| studio.season_number == season_num)
        })
        .and_then(|idx| app.studio_anime_ids.get(idx))
        .copied()?;
    season_is_complete(app, anime_id, season_num).then_some("✓")
}

fn selected_search_anime_id(app: &AppState) -> Option<u32> {
    let season_num = app.selected_season_num()?;
    let dub_idx = app.selected_dubbing_index?;
    app.current_sources.as_ref().and_then(|sources| {
        sources
            .ashdi
            .iter()
            .enumerate()
            .filter(|(_, studio)| studio.season_number == season_num)
            .nth(dub_idx)
            .and_then(|(idx, _)| app.studio_anime_ids.get(idx))
            .copied()
    })
}

fn anime_is_complete(app: &AppState, anime_id: u32) -> bool {
    let Some(sources) = app.sources_cache.get(&anime_id) else {
        return false;
    };

    let mut seasons: Vec<u32> = sources
        .ashdi
        .iter()
        .map(|studio| studio.season_number)
        .collect();
    seasons.sort();
    seasons.dedup();
    !seasons.is_empty()
        && seasons
            .into_iter()
            .all(|season_num| season_is_complete(app, anime_id, season_num))
}

fn library_anime_is_complete(anime: &crate::ui::app::LibraryAnimeEntry) -> bool {
    !anime.seasons.is_empty()
        && anime
            .seasons
            .iter()
            .all(|season| season.episodes.iter().all(|episode| episode.watched))
}

fn season_is_complete(app: &AppState, anime_id: u32, season_num: u32) -> bool {
    let Some(sources) = app.sources_cache.get(&anime_id) else {
        return false;
    };
    let Some(total) = sources
        .ashdi
        .iter()
        .filter(|studio| studio.season_number == season_num)
        .map(|studio| studio.episodes.len())
        .max()
    else {
        return false;
    };

    let watched = app
        .history
        .progress
        .values()
        .filter(|progress| {
            progress.anime_id == anime_id && progress.season == season_num && progress.watched
        })
        .count();

    watched >= total && total > 0
}

fn episode_is_watched(app: &AppState, anime_id: u32, season_num: u32, episode_num: u32) -> bool {
    app.history.progress.values().any(|progress| {
        progress.anime_id == anime_id
            && progress.season == season_num
            && progress.episode == episode_num
            && progress.watched
    })
}

fn episode_progress_timestamp(
    app: &AppState,
    anime_id: u32,
    season_num: u32,
    episode_num: u32,
) -> Option<f64> {
    app.history.progress.values().find_map(|progress| {
        (progress.anime_id == anime_id
            && progress.season == season_num
            && progress.episode == episode_num
            && !progress.watched
            && progress.timestamp >= 10.0)
            .then_some(progress.timestamp)
    })
}

fn create_list<'a>(title: &'a str, items: Vec<ListItem<'a>>, is_focused: bool) -> List<'a> {
    let border_style = if is_focused {
        Style::default().fg(COLOR_HIGHLIGHT)
    } else {
        Style::default().fg(COLOR_DIM)
    };

    List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style)
                .title_alignment(Alignment::Center),
        )
        .highlight_style(
            Style::default()
                .bg(COLOR_PRIMARY)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ")
}

fn render_popup(f: &mut Frame, title: &str, msg: &str, color: Color) {
    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", title),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color));
    let area = centered_rect(44, 28, f.area());
    f.render_widget(Clear, area);
    let text = Paragraph::new(msg)
        .block(block)
        .alignment(Alignment::Center)
        .wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(text, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn count_seasons(items: &[crate::api::AnimeItem], group: &[usize]) -> (usize, usize) {
    let mut tv = 0usize;
    let mut other = 0usize;
    for &i in group {
        let t = items[i].anime_type.to_lowercase();
        if t.contains("ova")
            || t.contains("фільм")
            || t.contains("film")
            || t.contains("спец")
            || t.contains("special")
            || t.contains("movie")
        {
            other += 1;
        } else {
            tv += 1;
        }
    }
    (tv, other)
}
