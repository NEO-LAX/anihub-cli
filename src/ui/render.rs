use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use ratatui_image::{StatefulImage, protocol::StatefulProtocol};

use crate::api;
use crate::storage::AnimeStatus;
use crate::ui::app::{AppMode, AppState, FocusPanel, LibraryFilter, PrimaryTab, StatusKind};

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
            Constraint::Length(6),
            Constraint::Min(0),
            Constraint::Length(if size.height >= 12 { 2 } else { 1 }),
        ])
        .split(size);

    render_header(f, app, main_chunks[0]);

    if app.mode == AppMode::Settings {
        render_settings_placeholder(f, main_chunks[1]);
    } else if size.width >= 110 {
        let body_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(main_chunks[1]);
        render_sidebar(f, app, body_chunks[0]);
        render_lists(f, app, body_chunks[1]);
    } else {
        render_lists(f, app, main_chunks[1]);
    }
    render_status_bar(f, app, main_chunks[2]);

    if let Some((message, StatusKind::Error)) = app.status_message.clone() {
        let msg = format!("{}\n\nEsc — закрити", message);
        render_popup(f, "Помилка", &msg, COLOR_ERROR);
    } else if let Some((title, _)) = app.moonanime_browser_prompt.clone() {
        let msg = format!(
            "«{title}»\n\nЦей епізод відкриється напряму в MoonAnime embed.\n\nEnter — відкрити    Esc — скасувати"
        );
        render_popup(f, "MoonAnime", &msg, COLOR_HIGHLIGHT);
    } else if app.status_editor.is_some() {
        render_status_editor_popup(f, app);
    } else if let Some((_, anime_title)) = app.pending_delete_confirmation.clone() {
        let msg = format!("Видалити прогрес для\n{}\n\n[y/n]", anime_title);
        render_popup(f, "Підтвердження", &msg, COLOR_ERROR);
    } else if app.show_help {
        render_help_popup(f);
    }
}

fn render_header(f: &mut Frame, app: &AppState, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    f.render_widget(
        Paragraph::new(Span::styled(
            "ANIHUB-CLI",
            Style::default()
                .fg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
        rows[0],
    );

    let tabs = PrimaryTab::ALL
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let active = *tab == app.primary_tab();
            Span::styled(
                format!("  {} {}  ", index + 1, tab.label()),
                if active {
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                } else {
                    Style::default().fg(COLOR_DIM)
                },
            )
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(Line::from(tabs))
            .alignment(Alignment::Center)
            .style(Style::default().bg(COLOR_BG_DARK)),
        rows[1],
    );

    let (title, context, alignment) = match app.primary_tab() {
        PrimaryTab::Search => (" Пошук ", search_header_context(app), Alignment::Left),
        PrimaryTab::Library => (
            " Категорії · Tab / Shift+Tab ",
            library_filter_context(app),
            Alignment::Center,
        ),
        PrimaryTab::Settings => (
            " Налаштування ",
            Line::from(Span::styled(
                "Розділ поки в розробці",
                Style::default().fg(COLOR_DIM),
            )),
            Alignment::Center,
        ),
    };
    f.render_widget(
        Paragraph::new(context)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .title_alignment(Alignment::Center)
                    .border_style(Style::default().fg(COLOR_DIM)),
            )
            .alignment(alignment)
            .style(Style::default().bg(COLOR_BG_DARK)),
        rows[2],
    );

    let mut breadcrumb = match app.primary_tab() {
        PrimaryTab::Search => {
            let path = search_breadcrumb(app);
            if path.is_empty() {
                "Пошук".to_string()
            } else {
                format!("Пошук  ›  {path}")
            }
        }
        PrimaryTab::Library => format!("Бібліотека  ›  {}", library_breadcrumb(app)),
        PrimaryTab::Settings => "Налаштування".to_string(),
    };
    if let Some(now) = &app.now_playing {
        breadcrumb.push_str(&format!(
            "   ·   ▶ S{}E{} · {}",
            now.season,
            now.episode,
            format_elapsed_timestamp(now.position)
        ));
    }
    f.render_widget(
        Paragraph::new(breadcrumb)
            .alignment(Alignment::Center)
            .style(Style::default().fg(COLOR_DIM)),
        rows[3],
    );

    if app.mode == AppMode::SearchInput {
        #[allow(clippy::cast_possible_truncation)]
        f.set_cursor_position((
            rows[2].x + (4 + app.search_cursor as u16).min(rows[2].width.saturating_sub(2)),
            rows[2].y + 1,
        ));
    }
}

fn search_header_context(app: &AppState) -> Line<'static> {
    let query = if app.mode == AppMode::SearchInput {
        app.search_query.as_str()
    } else {
        app.last_search_query.as_str()
    };
    let mut spans = vec![Span::styled("🔎 ", Style::default().fg(COLOR_SECONDARY))];
    if query.is_empty() {
        spans.push(Span::styled(
            "Натисніть / та введіть назву аніме",
            Style::default().fg(COLOR_DIM),
        ));
    } else {
        spans.push(Span::styled(
            query.to_string(),
            Style::default().fg(COLOR_TEXT),
        ));
    }
    Line::from(spans)
}

fn library_filter_context(app: &AppState) -> Line<'static> {
    let spans = LibraryFilter::ALL
        .iter()
        .map(|filter| {
            let text = format!("  {}  ", filter.label());
            if *filter == app.library_filter {
                Span::styled(
                    text,
                    Style::default()
                        .fg(COLOR_TEXT)
                        .bg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(text, Style::default().fg(COLOR_DIM))
            }
        })
        .collect::<Vec<_>>();
    Line::from(spans)
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

    let display_idx = app
        .sidebar_subject()
        .and_then(|anime_id| {
            app.search_results
                .iter()
                .position(|anime| anime.id == anime_id)
        })
        .or(app.selected_result_index);
    // Якщо поточний сезон — аніме не з пошуку (напр. S4 без ukr dub на сайті),
    // беремо `has_eng` з current_details, а не з search_results[representative].
    let has_eng = if selected_release_for_sidebar(app).is_some() {
        false
    } else if let Some(d) = sidebar_details_override(app) {
        d.title_english.is_some()
    } else {
        display_idx
            .and_then(|i| app.search_results.get(i))
            .and_then(|it| it.title_english.as_ref())
            .is_some()
    };
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

    if let Some(release) = selected_release_for_sidebar(app) {
        lines.push(
            Line::from(Span::styled(
                release.title.clone(),
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        );
    } else if let Some(d) = sidebar_details_override(app) {
        lines.push(
            Line::from(Span::styled(
                d.title_ukrainian.clone(),
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        );
        if let Some(eng) = &d.title_english {
            lines.push(
                Line::from(Span::styled(eng.clone(), Style::default().fg(COLOR_DIM)))
                    .alignment(Alignment::Center),
            );
        }
    } else if let Some(idx) = display_idx {
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
                    Line::from(Span::styled(eng.clone(), Style::default().fg(COLOR_DIM)))
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

    if let Some(release) = selected_release_for_sidebar(app) {
        if include_title {
            text.push(
                Line::from(Span::styled(
                    release.title.clone(),
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            text.push(mk_sep());
        }
        text.push(
            Line::from(vec![
                Span::styled("Тип: ", Style::default().fg(COLOR_DIM)),
                Span::styled(release.anime_type.clone(), Style::default().fg(COLOR_TEXT)),
            ])
            .alignment(Alignment::Center),
        );
        text.push(
            Line::from(vec![
                Span::styled("Рік: ", Style::default().fg(COLOR_DIM)),
                Span::styled(
                    release
                        .year
                        .map(|year| year.to_string())
                        .unwrap_or_else(|| "—".to_string()),
                    Style::default().fg(COLOR_TEXT),
                ),
            ])
            .alignment(Alignment::Center),
        );
        if let Some(rating) = release.rating {
            text.push(
                Line::from(vec![
                    Span::styled("Рейтинг: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(
                        format!("{rating:.1} ⭐"),
                        Style::default().fg(Color::Yellow),
                    ),
                ])
                .alignment(Alignment::Center),
            );
        }
        if let Some(episodes) = release.episodes_count {
            let episodes = release
                .available_episodes
                .filter(|available| *available < episodes)
                .map_or_else(
                    || episodes.to_string(),
                    |available| format!("{available}/{episodes}"),
                );
            text.push(
                Line::from(vec![
                    Span::styled("Серій: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(episodes, Style::default().fg(COLOR_TEXT)),
                ])
                .alignment(Alignment::Center),
            );
        }
        if release.availability == api::ReleaseAvailability::Unavailable {
            text.push(Line::from(Span::styled(
                "Недоступно на AniHub",
                Style::default().fg(COLOR_DIM),
            )));
        }
        if let Some(genres) = &release.genres {
            if !genres.is_empty() {
                text.push(Line::from(""));
                text.push(Line::from(vec![
                    Span::styled("Жанри: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(genres.join(" · "), Style::default().fg(COLOR_HIGHLIGHT)),
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
                    d.title_ukrainian.clone(),
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            if let Some(eng) = &d.title_english {
                text.push(
                    Line::from(Span::styled(eng.clone(), Style::default().fg(COLOR_DIM)))
                        .alignment(Alignment::Center),
                );
            }
            text.push(mk_sep());
        }

        text.push(
            Line::from(vec![
                Span::styled("Тип: ", Style::default().fg(COLOR_DIM)),
                Span::styled(d.anime_type.clone(), Style::default().fg(COLOR_TEXT)),
            ])
            .alignment(Alignment::Center),
        );
        text.push(
            Line::from(vec![
                Span::styled("Рік: ", Style::default().fg(COLOR_DIM)),
                Span::styled(
                    d.year
                        .map(|y| y.to_string())
                        .unwrap_or_else(|| "—".to_string()),
                    Style::default().fg(COLOR_TEXT),
                ),
            ])
            .alignment(Alignment::Center),
        );

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

        let has_extra = d.genres.as_ref().is_some_and(|g| !g.is_empty())
            || d.dubbing_studios.as_ref().is_some_and(|s| !s.is_empty());
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
    } else if let Some(idx) = display_idx {
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
                        Line::from(Span::styled(eng.clone(), Style::default().fg(COLOR_DIM)))
                            .alignment(Alignment::Center),
                    );
                }
                text.push(mk_sep());
            }

            // Кількість сезонів у групі (тільки в SearchList)
            if sidebar_is_representative(app) {
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
                if sidebar_is_representative(app) {
                    app.current_details.clone()
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

                let has_extra = d.genres.as_ref().is_some_and(|g| !g.is_empty())
                    || d.dubbing_studios.as_ref().is_some_and(|s| !s.is_empty());
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
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if area.height >= 2 {
            vec![Constraint::Length(1), Constraint::Length(1)]
        } else {
            vec![Constraint::Length(1)]
        })
        .split(area);

    let state = app
        .status_message
        .as_ref()
        .and_then(|(message, kind)| match kind {
            StatusKind::Info => Some(message.clone()),
            StatusKind::Error => None,
        })
        .unwrap_or_else(|| {
            if let Some(activity) = &app.activity_message {
                format!("⟳ {}", activity)
            } else if let Some(now) = &app.now_playing {
                let progress = if now.duration > 0.0 {
                    format!(
                        " · {}/{}",
                        format_elapsed_timestamp(now.position),
                        format_elapsed_timestamp(now.duration)
                    )
                } else if now.position > 0.0 {
                    format!(" · {}", format_elapsed_timestamp(now.position))
                } else {
                    String::new()
                };
                format!(
                    "▶ {} · S{}E{} · {}{}",
                    now.anime_title, now.season, now.episode, now.studio_name, progress
                )
            } else {
                String::new()
            }
        });

    f.render_widget(
        Paragraph::new(state)
            .style(
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .bg(COLOR_BG_DARK)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center),
        rows[0],
    );

    if rows.len() >= 2 {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(20), Constraint::Min(1)])
            .split(rows[1]);
        let (selected, total) = app.active_list_position();
        let position = if total > 0 {
            format!(
                " v{}  ·  {}/{}",
                env!("CARGO_PKG_VERSION"),
                selected + 1,
                total,
            )
        } else {
            format!(" v{}", env!("CARGO_PKG_VERSION"))
        };
        f.render_widget(
            Paragraph::new(position)
                .style(Style::default().fg(COLOR_DIM).bg(COLOR_BG_DARK))
                .alignment(Alignment::Left),
            columns[0],
        );
        f.render_widget(
            Paragraph::new(context_shortcuts(app))
                .style(Style::default().fg(COLOR_DIM).bg(COLOR_BG_DARK))
                .alignment(Alignment::Center),
            columns[1],
        );
    }
}

fn context_shortcuts(app: &AppState) -> String {
    if app.mode == AppMode::Settings {
        return " 1 Пошук   2 Бібліотека   Esc Назад   Ctrl+C Вихід ".to_string();
    }
    if app.mode == AppMode::SearchInput {
        return " Enter Пошук   ←/→ Курсор   Esc Скасувати   Ctrl+C Вихід ".to_string();
    }
    if app.is_library_mode() {
        return match app.mode {
            AppMode::Library => {
                " Tab Категорія   Enter Відкрити   e Статус   c Продовжити ".to_string()
            }
            AppMode::LibrarySeason | AppMode::LibraryDubbing => {
                " Tab Категорія   Enter Далі   Space Переглянуто   e Статус   Esc Назад "
                    .to_string()
            }
            AppMode::LibraryEpisode => {
                if app
                    .selected_dubbing_choice()
                    .is_some_and(|choice| choice.is_moonanime())
                {
                    " Tab Категорія   Enter Відкрити embed   e Статус   Esc Назад ".to_string()
                } else {
                    " Tab Категорія   Enter Відтворити   Space Переглянуто   Backspace Таймкод   e Статус "
                        .to_string()
                }
            }
            _ => String::new(),
        };
    }
    match app.focus {
        FocusPanel::SearchList => " Enter Випуски   e Статус   c Продовжити   / Пошук ".to_string(),
        FocusPanel::ReleaseList
            if app.has_release_catalog() && !app.selected_release_available() =>
        {
            " Недоступно на AniHub   Esc Назад ".to_string()
        }
        FocusPanel::ReleaseList | FocusPanel::DubbingList => {
            " Enter Далі   Space Переглянуто   e Статус   Esc Назад ".to_string()
        }
        FocusPanel::EpisodeList => {
            if app
                .selected_dubbing_choice()
                .is_some_and(|choice| choice.is_moonanime())
            {
                " Enter Відкрити embed   e Статус   Esc Назад ".to_string()
            } else {
                " Enter Відтворити   Space Переглянуто   Backspace Таймкод   e Статус ".to_string()
            }
        }
    }
}

fn search_breadcrumb(app: &AppState) -> String {
    let mut parts = Vec::new();
    if let Some(group_index) = app.selected_group_index {
        if let Some(catalog) = app.franchise_catalogs.get(group_index) {
            parts.push(catalog.canonical_title.clone());
        } else if let Some(group) = app.franchise_groups.get(group_index) {
            parts.push(api::franchise_display_name(&app.search_results, group).to_string());
        }
        let anime_ids = app
            .franchise_groups
            .get(group_index)
            .into_iter()
            .flatten()
            .filter_map(|index| app.search_results.get(*index).map(|anime| anime.id))
            .collect::<Vec<_>>();
        if let Some(status) = anime_status_for_ids(app, &anime_ids) {
            parts.push(status.label().to_string());
        }
    }
    if app.focus != FocusPanel::SearchList {
        if let (Some(catalog), Some(release)) =
            (app.selected_franchise_catalog(), app.selected_release())
        {
            parts.push(release_label(catalog, release));
        } else if let Some(season) = app.selected_season_num() {
            parts.push(format!("Сезон {}", season));
        }
    }
    if matches!(app.focus, FocusPanel::DubbingList | FocusPanel::EpisodeList) {
        if let Some(choice) = app.selected_dubbing_choice() {
            let suffix = if choice.is_moonanime() {
                " [MoonAnime]"
            } else {
                ""
            };
            parts.push(format!("{}{}", choice.studio_name(), suffix));
        }
    }
    if app.focus == FocusPanel::EpisodeList {
        if let Some(episode) = app
            .selected_episode_index
            .and_then(|index| app.selected_episode_choices().get(index).copied())
        {
            parts.push(format!("Серія {}", episode.episode_number()));
        }
    }
    parts.join(" › ")
}

fn library_breadcrumb(app: &AppState) -> String {
    let mut parts = vec![format!("[{}]", app.library_filter.label())];
    if let Some(anime) = app.library_selected_anime() {
        parts.push(anime.anime_title.clone());
        parts.push(anime.status.label().to_string());
    }
    if app.mode != AppMode::Library {
        if let Some(season) = app.selected_season_num() {
            parts.push(format!("Сезон {}", season));
        }
    }
    if matches!(app.mode, AppMode::LibraryDubbing | AppMode::LibraryEpisode) {
        if let Some(choice) = app.selected_dubbing_choice() {
            let suffix = if choice.is_moonanime() {
                " [MoonAnime]"
            } else {
                ""
            };
            parts.push(format!("{}{}", choice.studio_name(), suffix));
        }
    }
    if app.mode == AppMode::LibraryEpisode {
        if let Some(episode) = app
            .selected_episode_index
            .and_then(|index| app.selected_episode_choices().get(index).copied())
        {
            parts.push(format!("Серія {}", episode.episode_number()));
        }
    }
    parts.join(" › ")
}

fn render_lists(f: &mut Frame, app: &mut AppState, area: Rect) {
    if app.is_library_mode() {
        render_library_lists(f, app, area);
        return;
    }

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
        let mut items: Vec<ListItem> = Vec::new();
        for (group_index, group) in app.franchise_groups.iter().enumerate() {
            let Some(&representative_index) = group.first() else {
                continue;
            };
            let name = app.franchise_catalogs.get(group_index).map_or_else(
                || api::franchise_display_name(&app.search_results, group),
                |catalog| catalog.canonical_title.as_str(),
            );
            let rep = &app.search_results[representative_index];
            let (tv, other) = count_seasons(&app.search_results, group);
            let title = if let Some(summary) = app
                .franchise_catalogs
                .get(group_index)
                .and_then(catalog_summary)
            {
                format!("{} ({})", name, summary)
            } else if tv > 1 && other > 0 {
                format!("{} ({} сез. + {} спец.)", name, tv, other)
            } else if tv > 1 {
                format!("{} ({} сез.)", name, tv)
            } else {
                match rep.year {
                    Some(y) => format!("{} ({})", name, y),
                    None => name.to_string(),
                }
            };
            let mut marker = String::new();
            let group_ids = group
                .iter()
                .filter_map(|index| app.search_results.get(*index).map(|anime| anime.id))
                .collect::<Vec<_>>();
            if let Some(status) = anime_status_for_ids(app, &group_ids) {
                marker.push_str(anime_status_marker(status));
            }
            if franchise_is_complete(app, group) && !marker.contains('✓') {
                marker.push('✓');
            }
            let mut lines = vec![Line::from(with_right_marker(&title, &marker, list_width))];
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
                    eng.clone(),
                    Style::default().fg(COLOR_DIM),
                )));
            }
            items.push(ListItem::new(lines));
        }
        if items.is_empty() {
            let message = if app.activity_message.is_some() {
                "Шукаємо…"
            } else if app.last_search_query.is_empty() {
                "Натисніть / та введіть назву аніме"
            } else {
                "За цим запитом нічого не знайдено"
            };
            items.push(ListItem::new(Line::from(Span::styled(
                message,
                Style::default().fg(COLOR_DIM),
            ))));
        }

        let list = create_list(
            " Результати пошуку ",
            items,
            app.focus == FocusPanel::SearchList,
        );
        f.render_stateful_widget(list, list_chunks[0], &mut app.result_list_state);
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
            let mut visual_state = ratatui::widgets::ListState::default();
            visual_state.select(
                app.season_list_state
                    .selected()
                    .and_then(|release_index| release_rows.get(release_index).copied()),
            );
            let list = create_list(" Випуски ", items, app.focus == FocusPanel::ReleaseList);
            f.render_stateful_widget(list, list_chunks[idx], &mut visual_state);
        } else {
            let items = app
                .unique_seasons()
                .iter()
                .map(|&sn| {
                    let count = app.dubbing_choices_for_season(sn).len();
                    let year_str = season_year(app, sn)
                        .map(|y| format!(" · {}", y))
                        .unwrap_or_default();
                    let label = if count > 1 {
                        format!("Сезон {}{} ({} озвучок)", sn, year_str, count)
                    } else {
                        format!("Сезон {}{}", sn, year_str)
                    };
                    let marker = season_marker_for_search(app, sn);
                    ListItem::new(with_right_marker(
                        &label,
                        marker.unwrap_or(""),
                        list_chunks[idx].width.saturating_sub(6) as usize,
                    ))
                })
                .collect();
            let list = create_list(" Випуски ", items, app.focus == FocusPanel::ReleaseList);
            f.render_stateful_widget(list, list_chunks[idx], &mut app.season_list_state);
        }
    }

    if let Some(idx) = dubbing_chunk_idx {
        let items: Vec<ListItem> = if let Some(sn) = app.selected_season_num() {
            app.dubbing_choices_for_season(sn)
                .iter()
                .map(|choice| {
                    let provider = if choice.is_moonanime() {
                        " [MoonAnime]"
                    } else {
                        ""
                    };
                    ListItem::new(format!(
                        "{}{} ({} серій)",
                        choice.studio_name(),
                        provider,
                        choice.episodes_count()
                    ))
                })
                .collect()
        } else {
            vec![]
        };
        let list = create_list(" Озвучки ", items, app.focus == FocusPanel::DubbingList);
        f.render_stateful_widget(list, list_chunks[idx], &mut app.dubbing_list_state);
    }

    if let Some(idx) = episode_chunk_idx {
        let items: Vec<ListItem> = if app
            .selected_dubbing_choice()
            .is_some_and(|choice| choice.is_moonanime())
        {
            app.selected_episode_choices()
                .iter()
                .map(|episode| {
                    let title = episode.title();
                    let suffix = if title.is_empty() {
                        "".to_string()
                    } else {
                        format!(" · {title}")
                    };
                    ListItem::new(format!(
                        "Серія {}{} [браузер]",
                        episode.episode_number(),
                        suffix
                    ))
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
                                list_chunks[idx].width.saturating_sub(6) as usize,
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
                        list_chunks[idx].width.saturating_sub(6) as usize,
                    ))
                })
                .collect()
        } else {
            vec![]
        };
        let list = create_list(" Серії ", items, app.focus == FocusPanel::EpisodeList);
        f.render_stateful_widget(list, list_chunks[idx], &mut app.episode_list_state);
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

    let mut anime_items: Vec<ListItem> = app
        .library_items
        .iter()
        .map(|item| {
            let marker = anime_status_marker(item.status).to_string();
            let line_1 = with_right_marker(
                &item.anime_title,
                &marker,
                chunks[0].width.saturating_sub(6) as usize,
            );
            let line_2 = if item.seasons.is_empty() {
                item.status.label().to_string()
            } else {
                format!(
                    "{} · Сезон {} · Серія {} · ⏱ {}",
                    item.status.label(),
                    item.latest_progress.season,
                    item.latest_progress.episode,
                    format_timestamp(item.latest_progress.timestamp),
                )
            };
            ListItem::new(vec![
                Line::from(line_1),
                Line::from(Span::styled(line_2, Style::default().fg(COLOR_DIM))),
            ])
        })
        .collect();
    if anime_items.is_empty() {
        anime_items.push(ListItem::new(vec![
            Line::from(Span::styled(
                format!("У розділі «{}» поки порожньо", app.library_filter.label()),
                Style::default().fg(COLOR_DIM),
            )),
            Line::from(Span::styled(
                "Tab / Shift+Tab — змінити категорію",
                Style::default().fg(COLOR_HIGHLIGHT),
            )),
        ]));
    }
    let library_title = format!(" {} ", app.library_filter.label());
    let anime_list = create_list(&library_title, anime_items, app.mode == AppMode::Library);
    f.render_stateful_widget(anime_list, chunks[0], &mut app.library_anime_list_state);

    if chunks.len() >= 2 {
        let season_items: Vec<ListItem> = app
            .library_season_numbers()
            .iter()
            .map(|&season_num| {
                let count = app.dubbing_choices_for_season(season_num).len();
                let year_str = season_year(app, season_num)
                    .map(|y| format!(" · {}", y))
                    .unwrap_or_default();
                let label = if count > 1 {
                    format!("Сезон {}{} ({} озвучок)", season_num, year_str, count)
                } else {
                    format!("Сезон {}{}", season_num, year_str)
                };
                let marker = if app
                    .library_selected_anime_id()
                    .is_some_and(|anime_id| season_is_complete(app, anime_id, season_num))
                {
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
        let season_list = create_list(" Сезони ", season_items, app.mode == AppMode::LibrarySeason);
        f.render_stateful_widget(season_list, chunks[1], &mut app.season_list_state);
    }

    if chunks.len() >= 3 {
        let dubbing_items: Vec<ListItem> = if let Some(sn) = app.selected_season_num() {
            app.dubbing_choices_for_season(sn)
                .iter()
                .map(|choice| {
                    let provider = if choice.is_moonanime() {
                        " [MoonAnime]"
                    } else {
                        ""
                    };
                    ListItem::new(format!(
                        "{}{} ({} серій)",
                        choice.studio_name(),
                        provider,
                        choice.episodes_count()
                    ))
                })
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
        let episode_items: Vec<ListItem> = if app
            .selected_dubbing_choice()
            .is_some_and(|choice| choice.is_moonanime())
        {
            app.selected_episode_choices()
                .iter()
                .map(|episode| {
                    let title = episode.title();
                    let suffix = if title.is_empty() {
                        "".to_string()
                    } else {
                        format!(" · {title}")
                    };
                    ListItem::new(format!(
                        "Серія {}{} [браузер]",
                        episode.episode_number(),
                        suffix
                    ))
                })
                .collect()
        } else if let Some(studio) = app.selected_studio() {
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
                    let marker = if anime_id.is_some_and(|id| {
                        episode_is_watched(app, id, studio.season_number, episode.episode_number)
                    }) {
                        "✓"
                    } else {
                        ""
                    };
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
                Line::from(Span::styled(eng.clone(), Style::default().fg(COLOR_DIM)))
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
                    Line::from(Span::styled(eng.clone(), Style::default().fg(COLOR_DIM)))
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
    let mut anime_ids = Vec::with_capacity(group.len());
    for &idx in group {
        if let Some(item) = app.search_results.get(idx) {
            anime_ids.push(item.id);
        }
    }
    app.history
        .progress
        .values()
        .filter(|progress| anime_ids.contains(&progress.anime_id))
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

fn season_marker_for_search(app: &AppState, season_num: u32) -> Option<&str> {
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
    let source_key = app.source_key_for_anime_id(anime_id);
    let Some(sources) = app.sources_cache.get(&source_key) else {
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

fn anime_status_for_ids(app: &AppState, anime_ids: &[u32]) -> Option<AnimeStatus> {
    if let Some(status) = anime_ids
        .iter()
        .filter_map(|anime_id| app.history.library.get(anime_id))
        .max_by_key(|record| record.updated_at)
        .map(|record| record.status)
    {
        return (status != AnimeStatus::NotAdded).then_some(status);
    }
    let progress = app
        .history
        .progress
        .values()
        .filter(|progress| anime_ids.contains(&progress.anime_id))
        .collect::<Vec<_>>();
    if progress.is_empty() {
        None
    } else if progress.iter().all(|progress| progress.watched) {
        Some(AnimeStatus::Completed)
    } else {
        Some(AnimeStatus::Watching)
    }
}

const fn anime_status_marker(status: AnimeStatus) -> &'static str {
    match status {
        AnimeStatus::NotAdded => "",
        AnimeStatus::Planned => "+",
        AnimeStatus::Watching => "▶",
        AnimeStatus::Completed => "✓",
        AnimeStatus::OnHold => "Ⅱ",
        AnimeStatus::Dropped => "×",
    }
}

fn season_is_complete(app: &AppState, anime_id: u32, season_num: u32) -> bool {
    let source_key = api::EpisodeSourcesKey::new(anime_id, season_num);
    let Some(sources) = app.sources_cache.get(&source_key) else {
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

/// Повертає рік виходу сезону season_num через studio_anime_ids → details_cache.
fn season_year(app: &AppState, season_num: u32) -> Option<u32> {
    let sources = app.current_sources.as_ref()?;
    let studio_idx = sources
        .ashdi
        .iter()
        .position(|s| s.season_number == season_num)?;
    let anime_id = app.studio_anime_ids.get(studio_idx).copied()?;
    app.details_cache
        .get(&anime_id)
        .and_then(|d| d.year)
        .or_else(|| {
            app.search_results
                .iter()
                .find(|a| a.id == anime_id)
                .and_then(|a| a.year)
        })
}

fn episode_is_watched(app: &AppState, anime_id: u32, season_num: u32, episode_num: u32) -> bool {
    app.watched_index
        .contains(&(anime_id, season_num, episode_num))
}

fn episode_progress_timestamp(
    app: &AppState,
    anime_id: u32,
    season_num: u32,
    episode_num: u32,
) -> Option<f64> {
    app.progress_index
        .get(&(anime_id, season_num, episode_num))
        .copied()
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

fn render_settings_placeholder(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_DIM))
        .title(" Налаштування ")
        .title_alignment(Alignment::Center);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Цей розділ поки в розробці",
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "1 / 2 — перейти до пошуку або бібліотеки",
                Style::default().fg(COLOR_DIM),
            )),
        ])
        .block(block)
        .alignment(Alignment::Center),
        area,
    );
}

fn render_status_editor_popup(f: &mut Frame, app: &AppState) {
    let Some(editor) = app.status_editor.as_ref() else {
        return;
    };
    let area = centered_rect(42, 55, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            " Статус аніме ",
            Style::default()
                .fg(COLOR_SECONDARY)
                .add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_PRIMARY));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(inner);
    f.render_widget(
        Paragraph::new(editor.title.as_str())
            .alignment(Alignment::Center)
            .style(Style::default().fg(COLOR_TEXT)),
        rows[0],
    );
    let items = AnimeStatus::ALL
        .iter()
        .enumerate()
        .map(|(index, status)| {
            let radio = if index == editor.selected {
                "●"
            } else {
                "○"
            };
            ListItem::new(format!(" {}  {}", radio, status.label()))
        })
        .collect::<Vec<_>>();
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(editor.selected));
    let list = List::new(items).highlight_style(
        Style::default()
            .fg(COLOR_TEXT)
            .bg(COLOR_PRIMARY)
            .add_modifier(Modifier::BOLD),
    );
    f.render_stateful_widget(list, rows[1], &mut state);
    f.render_widget(
        Paragraph::new("↑/↓ Вибір   Enter Зберегти   Esc Скасувати")
            .alignment(Alignment::Center)
            .style(Style::default().fg(COLOR_DIM)),
        rows[2],
    );
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

fn render_help_popup(f: &mut Frame) {
    let title = " Довідка (Гарячі клавіші) ";
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_PRIMARY))
        .bg(COLOR_BG_DARK);

    let area = centered_rect(65, 55, f.area());
    f.render_widget(Clear, area);
    let inner_area = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .margin(1)
        .split(inner_area);

    let left_col = vec![
        Line::from(vec![Span::styled(
            " Глобальні ",
            Style::default()
                .bg(COLOR_SECONDARY)
                .fg(COLOR_BG_DARK)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  1/2/3  — Пошук/Бібліотека/Налаштування"),
        Line::from("  /      — Редагувати пошук"),
        Line::from("  ? / h  — Довідка"),
        Line::from("  q      — Вийти"),
        Line::from("  Ctrl+C — Вийти будь-де"),
        Line::from(""),
        Line::from(vec![Span::styled(
            " Навігація ",
            Style::default()
                .bg(COLOR_SECONDARY)
                .fg(COLOR_BG_DARK)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  ↑ / ↓  — Список"),
        Line::from("  j / k  — Список"),
        Line::from("  PgUp/Dn— Сторінка"),
        Line::from("  Home/End— Початок/кінець"),
        Line::from("  → / ↵  — Вперед"),
        Line::from("  ← / Esc— Назад"),
    ];

    let right_col = vec![
        Line::from(vec![Span::styled(
            " Дії з аніме ",
            Style::default()
                .bg(COLOR_SECONDARY)
                .fg(COLOR_BG_DARK)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  Enter  — Відтворити (mpv)"),
        Line::from("  c      — Продовжити"),
        Line::from("  e      — Статус аніме"),
        Line::from("  Space  — Переглянуто"),
        Line::from("  Backsp.— Очистити таймкод"),
        Line::from("  d      — Видалити прогрес"),
        Line::from("  o      — Відкрити в браузері"),
        Line::from("  Tab/⇧Tab— Категорія бібліотеки"),
    ];

    f.render_widget(Paragraph::new(left_col), chunks[0]);
    f.render_widget(Paragraph::new(right_col), chunks[1]);

    // Footer hint centered at the bottom of the popup
    let footer_area = Rect::new(area.x, area.y + area.height - 2, area.width, 1);
    f.render_widget(
        Paragraph::new(Span::styled(
            " Натисніть будь-яку клавішу щоб закрити ",
            Style::default().fg(COLOR_DIM),
        ))
        .alignment(Alignment::Center),
        footer_area,
    );
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

fn release_label(catalog: &api::FranchiseCatalog, release: &api::ReleaseEntry) -> String {
    match release.classification {
        api::ReleaseClassification::MainlineSeason => {
            let season = release.conceptual_season.unwrap_or(1);
            let same_season_count = catalog
                .releases
                .iter()
                .filter(|candidate| {
                    candidate.classification == api::ReleaseClassification::MainlineSeason
                        && candidate.conceptual_season == Some(season)
                })
                .count();
            if same_season_count > 1 || release.part.unwrap_or(1) > 1 {
                format!("Сезон {} · Частина {}", season, release.part.unwrap_or(1))
            } else {
                format!("Сезон {}", season)
            }
        }
        api::ReleaseClassification::MainlineMovie => format!("Фільм · {}", release.title),
        api::ReleaseClassification::MainlineSpecial => {
            format!("Спецвипуск · {}", release.title)
        }
        api::ReleaseClassification::Extra => {
            let kind = release.anime_type.to_lowercase();
            let prefix = if kind.contains("ova") {
                "OVA"
            } else if kind.contains("movie") || kind.contains("film") || kind.contains("фільм")
            {
                "Фільм"
            } else {
                "Додатково"
            };
            format!("{} · {}", prefix, release.title)
        }
    }
}

fn release_list_item(
    catalog: &api::FranchiseCatalog,
    release: &api::ReleaseEntry,
    width: usize,
) -> ListItem<'static> {
    let mut lines = Vec::new();
    let unavailable = release.availability == api::ReleaseAvailability::Unavailable;
    lines.push(Line::from(with_right_marker(
        &release_label(catalog, release),
        if unavailable { "×" } else { "" },
        width,
    )));
    let mut metadata = Vec::new();
    if let Some(year) = release.year {
        metadata.push(year.to_string());
    }
    if let Some(episodes) = release.episodes_count {
        let episodes = release
            .available_episodes
            .filter(|available| *available < episodes)
            .map_or_else(
                || episodes.to_string(),
                |available| format!("{available}/{episodes}"),
            );
        metadata.push(format!("{} сер.", episodes));
    }
    if unavailable {
        metadata.push("недоступно на AniHub".to_string());
    }
    if !metadata.is_empty() {
        lines.push(Line::from(Span::styled(
            metadata.join(" · "),
            Style::default().fg(COLOR_DIM),
        )));
    }

    let item = ListItem::new(lines);
    if unavailable {
        item.style(Style::default().fg(COLOR_DIM))
    } else {
        item
    }
}

fn release_catalog_items(
    catalog: &api::FranchiseCatalog,
    width: usize,
) -> (Vec<ListItem<'static>>, Vec<usize>) {
    let mut items = Vec::new();
    let mut release_rows = Vec::with_capacity(catalog.releases.len());
    let mut previous_extra = None;

    for release in &catalog.releases {
        let is_extra = release.classification == api::ReleaseClassification::Extra;
        if previous_extra != Some(is_extra) {
            let label = if is_extra {
                "── ДОДАТКОВО ──"
            } else {
                "── ОСНОВНА ІСТОРІЯ ──"
            };
            items.push(
                ListItem::new(Line::from(Span::styled(
                    label,
                    Style::default()
                        .fg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                )))
                .style(Style::default().bg(COLOR_BG_DARK)),
            );
            previous_extra = Some(is_extra);
        }
        release_rows.push(items.len());
        items.push(release_list_item(catalog, release, width));
    }

    (items, release_rows)
}

fn catalog_summary(catalog: &api::FranchiseCatalog) -> Option<String> {
    if catalog.releases.is_empty() {
        return None;
    }
    let mut seasons = Vec::new();
    let mut season_releases = 0usize;
    let mut movies = 0usize;
    let mut extras = 0usize;
    for release in &catalog.releases {
        match release.classification {
            api::ReleaseClassification::MainlineSeason => {
                season_releases += 1;
                if let Some(season) = release.conceptual_season {
                    if !seasons.contains(&season) {
                        seasons.push(season);
                    }
                }
            }
            api::ReleaseClassification::MainlineMovie => movies += 1,
            api::ReleaseClassification::MainlineSpecial | api::ReleaseClassification::Extra => {
                extras += 1
            }
        }
    }
    let mut parts = Vec::new();
    if !seasons.is_empty() {
        parts.push(format!("{} сез.", seasons.len()));
    }
    if season_releases > seasons.len() {
        parts.push(format!("{} част.", season_releases));
    }
    if movies > 0 {
        parts.push(format!("{} фільм.", movies));
    }
    if extras > 0 {
        parts.push(format!("{} дод.", extras));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn selected_release_for_sidebar(app: &AppState) -> Option<&api::ReleaseEntry> {
    if app.is_library_mode() {
        return None;
    }
    if app.focus != FocusPanel::SearchList {
        return app.selected_release();
    }
    let catalog = app.selected_franchise_catalog()?;
    catalog
        .anchor_anilist_id
        .and_then(|anchor| {
            catalog
                .releases
                .iter()
                .find(|release| release.anilist_id == Some(anchor))
        })
        .or_else(|| catalog.releases.first())
}

/// Повертає metadata точного sidebar subject, коли він відрізняється
/// від канонічного представника франшизи.
fn sidebar_details_override(app: &AppState) -> Option<api::AnimeDetails> {
    let subject_id = app.sidebar_subject()?;
    let rep_id = app
        .selected_result_index
        .and_then(|i| app.search_results.get(i))
        .map(|a| a.id);
    if rep_id == Some(subject_id) {
        return None;
    }
    app.details_cache.get(&subject_id).or_else(|| {
        app.current_details
            .as_ref()
            .filter(|details| details.id == subject_id)
            .cloned()
    })
}

fn sidebar_is_representative(app: &AppState) -> bool {
    if app.focus == FocusPanel::SearchList {
        return true;
    }
    let representative_id = app
        .selected_result_index
        .and_then(|index| app.search_results.get(index))
        .map(|anime| anime.id);
    app.sidebar_subject() == representative_id
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

#[cfg(test)]
mod tests {
    use super::*;

    fn release(
        title: &str,
        season: Option<u32>,
        part: Option<u32>,
        classification: api::ReleaseClassification,
    ) -> api::ReleaseEntry {
        api::ReleaseEntry {
            anihub_id: Some(1),
            anilist_id: Some(10),
            title: title.to_string(),
            anime_type: "TV".to_string(),
            year: Some(2024),
            poster_url: None,
            episodes_count: Some(12),
            available_episodes: Some(12),
            description: None,
            rating: None,
            genres: None,
            dubbing_studios: None,
            conceptual_season: season,
            part,
            classification,
            availability: api::ReleaseAvailability::Available,
        }
    }

    #[test]
    fn split_cours_use_conceptual_season_and_part_labels() {
        let catalog = api::FranchiseCatalog {
            anchor_anilist_id: Some(10),
            canonical_title: "Test".to_string(),
            canonical_poster_url: None,
            unresolved_anilist_ids: Vec::new(),
            releases: vec![
                release(
                    "Cour 1",
                    Some(1),
                    Some(1),
                    api::ReleaseClassification::MainlineSeason,
                ),
                release(
                    "Cour 2",
                    Some(1),
                    Some(2),
                    api::ReleaseClassification::MainlineSeason,
                ),
            ],
        };

        assert_eq!(
            release_label(&catalog, &catalog.releases[0]),
            "Сезон 1 · Частина 1"
        );
        assert_eq!(
            release_label(&catalog, &catalog.releases[1]),
            "Сезон 1 · Частина 2"
        );
    }

    #[test]
    fn catalog_summary_separates_seasons_movies_and_extras() {
        let mut releases = vec![release(
            "Season",
            Some(1),
            Some(1),
            api::ReleaseClassification::MainlineSeason,
        )];
        releases.push(release(
            "Movie",
            None,
            None,
            api::ReleaseClassification::MainlineMovie,
        ));
        releases.push(release(
            "OVA",
            None,
            None,
            api::ReleaseClassification::Extra,
        ));
        let catalog = api::FranchiseCatalog {
            anchor_anilist_id: Some(10),
            canonical_title: "Test".to_string(),
            canonical_poster_url: None,
            unresolved_anilist_ids: Vec::new(),
            releases,
        };

        assert_eq!(
            catalog_summary(&catalog).as_deref(),
            Some("1 сез. · 1 фільм. · 1 дод.")
        );
    }

    #[test]
    fn release_sections_are_separate_non_release_rows() {
        let catalog = api::FranchiseCatalog {
            anchor_anilist_id: Some(10),
            canonical_title: "Test".to_string(),
            canonical_poster_url: None,
            unresolved_anilist_ids: Vec::new(),
            releases: vec![
                release(
                    "Season",
                    Some(1),
                    Some(1),
                    api::ReleaseClassification::MainlineSeason,
                ),
                release(
                    "Movie",
                    None,
                    None,
                    api::ReleaseClassification::MainlineMovie,
                ),
                release("OVA", None, None, api::ReleaseClassification::Extra),
            ],
        };

        let (rows, release_rows) = release_catalog_items(&catalog, 40);

        assert_eq!(rows.len(), 5);
        assert_eq!(release_rows, vec![1, 2, 4]);
    }
}
