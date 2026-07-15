use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
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
            Constraint::Length(if size.height >= 12 { 2 } else { 1 }),
        ])
        .split(size);

    render_header(f, app, main_chunks[0]);

    if size.width >= 110 {
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
    } else if let Some((_, anime_title)) = app.pending_delete_confirmation.clone() {
        let msg = format!("Видалити прогрес для\n{}\n\n[y/n]", anime_title);
        render_popup(f, "Підтвердження", &msg, COLOR_ERROR);
    } else if app.show_help {
        render_help_popup(f);
    }
}

fn render_header(f: &mut Frame, app: &AppState, area: Rect) {
    if app.is_library_mode() {
        let breadcrumb = library_breadcrumb(app);
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                "Бібліотека",
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  ·  {}", breadcrumb),
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

    let visible_query = if app.mode == AppMode::SearchInput {
        app.search_query.as_str()
    } else {
        app.last_search_query.as_str()
    };
    let search_text = if visible_query.is_empty() && app.mode != AppMode::SearchInput {
        vec![Span::styled(
            "Натисніть '/' для пошуку аніме…",
            Style::default().fg(Color::DarkGray),
        )]
    } else {
        let mut spans = vec![
            Span::styled("🔍 ", Style::default().fg(COLOR_SECONDARY)),
            Span::styled(visible_query, Style::default().fg(COLOR_TEXT)),
        ];
        if app.mode != AppMode::SearchInput {
            let breadcrumb = search_breadcrumb(app);
            if !breadcrumb.is_empty() {
                spans.push(Span::styled(
                    format!("  ·  {}", breadcrumb),
                    Style::default().fg(COLOR_DIM),
                ));
            }
        }
        spans
    };

    let search_align = if visible_query.is_empty() && app.mode != AppMode::SearchInput {
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
            area.x + (4 + app.search_cursor as u16).min(area.width.saturating_sub(2)),
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
    // Якщо поточний сезон — аніме не з пошуку (напр. S4 без ukr dub на сайті),
    // беремо `has_eng` з current_details, а не з search_results[representative].
    let has_eng = if let Some(d) = sidebar_details_override(app) {
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

    // Якщо обраний сезон належить аніме не з пошуку — показуємо його назву з current_details
    if let Some(d) = sidebar_details_override(app) {
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
            .constraints([Constraint::Min(1), Constraint::Length(18)])
            .split(rows[1]);
        f.render_widget(
            Paragraph::new(context_shortcuts(app))
                .style(Style::default().fg(COLOR_DIM).bg(COLOR_BG_DARK))
                .alignment(Alignment::Left),
            columns[0],
        );
        let (selected, total) = app.active_list_position();
        let position = if total > 0 {
            format!(
                "{}/{}  ·  v{} ",
                selected + 1,
                total,
                env!("CARGO_PKG_VERSION")
            )
        } else {
            format!("v{} ", env!("CARGO_PKG_VERSION"))
        };
        f.render_widget(
            Paragraph::new(position)
                .style(Style::default().fg(COLOR_DIM).bg(COLOR_BG_DARK))
                .alignment(Alignment::Right),
            columns[1],
        );
    }
}

fn context_shortcuts(app: &AppState) -> String {
    if app.mode == AppMode::SearchInput {
        return " Enter Пошук   ←/→ Курсор   Esc Скасувати   Ctrl+C Вихід ".to_string();
    }
    if app.is_library_mode() {
        return match app.mode {
            AppMode::Library => {
                " Tab Розділ   Enter Відкрити   c Продовжити   d Видалити   / Пошук ".to_string()
            }
            AppMode::LibrarySeason | AppMode::LibraryDubbing => {
                " Tab Розділ   Enter Далі   x Переглянуто   b Закладка   Esc Назад ".to_string()
            }
            AppMode::LibraryEpisode => {
                " Enter Відтворити   x Переглянуто   b Закладка   Esc Назад ".to_string()
            }
            _ => String::new(),
        };
    }
    match app.focus {
        FocusPanel::SearchList => {
            " Enter Сезони   c Продовжити   l Бібліотека   / Пошук   h Довідка ".to_string()
        }
        FocusPanel::SeasonList | FocusPanel::DubbingList => {
            " Enter Далі   x Переглянуто   b Закладка   Esc Назад ".to_string()
        }
        FocusPanel::EpisodeList => {
            " Enter Відтворити   x Переглянуто   b Закладка   Esc Назад ".to_string()
        }
    }
}

fn search_breadcrumb(app: &AppState) -> String {
    let mut parts = Vec::new();
    if let Some(group_index) = app.selected_group_index {
        if let Some(group) = app.franchise_groups.get(group_index) {
            parts.push(api::franchise_display_name(&app.search_results, group).to_string());
        }
    }
    if app.focus != FocusPanel::SearchList {
        if let Some(season) = app.selected_season_num() {
            parts.push(format!("Сезон {}", season));
        }
    }
    if matches!(app.focus, FocusPanel::DubbingList | FocusPanel::EpisodeList) {
        if let Some(studio) = app.selected_studio() {
            parts.push(studio.studio_name.clone());
        }
    }
    if app.focus == FocusPanel::EpisodeList {
        if let (Some(studio), Some(index)) = (app.selected_studio(), app.selected_episode_index) {
            if let Some(episode) = studio.episodes.get(index) {
                parts.push(format!("Серія {}", episode.episode_number));
            }
        }
    }
    parts.join(" › ")
}

fn library_breadcrumb(app: &AppState) -> String {
    let mut parts = vec![format!("[{}]", app.library_filter.label())];
    if let Some(anime) = app.library_selected_anime() {
        parts.push(anime.anime_title.clone());
    }
    if app.mode != AppMode::Library {
        if let Some(season) = app.selected_season_num() {
            parts.push(format!("Сезон {}", season));
        }
    }
    if matches!(app.mode, AppMode::LibraryDubbing | AppMode::LibraryEpisode) {
        if let Some(studio) = app.selected_studio() {
            parts.push(studio.studio_name.clone());
        }
    }
    if app.mode == AppMode::LibraryEpisode {
        if let (Some(studio), Some(index)) = (app.selected_studio(), app.selected_episode_index) {
            if let Some(episode) = studio.episodes.get(index) {
                parts.push(format!("Серія {}", episode.episode_number));
            }
        }
    }
    parts.join(" › ")
}

fn render_lists(f: &mut Frame, app: &mut AppState, area: Rect) {
    if app.is_library_mode() {
        render_library_lists(f, app, area);
        return;
    }

    // Якщо лише один сезон (фільм або однасезонний) — не показуємо панель "Сезони"
    let single_season = app.unique_seasons().len() <= 1
        && matches!(app.focus, FocusPanel::DubbingList | FocusPanel::EpisodeList);
    let compact = area.width < 90;

    let constraints = match app.focus {
        FocusPanel::SearchList => vec![Constraint::Percentage(100)],
        FocusPanel::SeasonList if compact => {
            vec![Constraint::Percentage(25), Constraint::Percentage(75)]
        }
        FocusPanel::SeasonList => vec![Constraint::Percentage(50), Constraint::Percentage(50)],
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
            let mut marker = String::new();
            if group.iter().any(|index| {
                app.search_results
                    .get(*index)
                    .is_some_and(|anime| app.history.bookmarks.contains(&anime.id))
            }) {
                marker.push('★');
            }
            if franchise_is_complete(app, group) {
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
    // normal:        [SearchList, SeasonList?, DubbingList?, EpisodeList?]
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
        let seasons = app.unique_seasons();
        let items: Vec<ListItem> = seasons
            .iter()
            .map(|&sn| {
                let count = app.studios_for_season(sn).len();
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
        let list = create_list(" Сезони ", items, app.focus == FocusPanel::SeasonList);
        f.render_stateful_widget(list, list_chunks[idx], &mut app.season_list_state);
    }

    if let Some(idx) = dubbing_chunk_idx {
        let items: Vec<ListItem> = if let Some(sn) = app.selected_season_num() {
            app.studios_for_season(sn)
                .iter()
                .map(|s| ListItem::new(format!("{} ({} серій)", s.studio_name, s.episodes_count)))
                .collect()
        } else {
            vec![]
        };
        let list = create_list(" Озвучки ", items, app.focus == FocusPanel::DubbingList);
        f.render_stateful_widget(list, list_chunks[idx], &mut app.dubbing_list_state);
    }

    if let Some(idx) = episode_chunk_idx {
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
            let mut marker = String::new();
            if item
                .anime_ids
                .iter()
                .any(|anime_id| app.history.bookmarks.contains(anime_id))
            {
                marker.push('★');
            }
            if library_anime_is_complete(item) {
                marker.push('✓');
            }
            let line_1 = with_right_marker(
                &item.anime_title,
                &marker,
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
    if anime_items.is_empty() {
        anime_items.push(ListItem::new(vec![
            Line::from(Span::styled(
                format!("У розділі «{}» поки порожньо", app.library_filter.label()),
                Style::default().fg(COLOR_DIM),
            )),
            Line::from(Span::styled(
                "Tab — змінити розділ · / — пошук",
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
                let count = app.studios_for_season(season_num).len();
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
        Line::from("  /      — Пошук"),
        Line::from("  l      — Бібліотека"),
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
        Line::from("  x      — Переглянуто"),
        Line::from("  b      — Закладка ★"),
        Line::from("  d      — Видалити прогрес"),
        Line::from("  o      — Відкрити в браузері"),
        Line::from("  Tab    — Розділ бібліотеки"),
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

/// Повертає `current_details` якщо поточний сезон належить аніме, якого немає в `search_results`
/// (наприклад, S4 доданий на anihub але без `has_ukrainian_dub`, тому не потрапив у пошук).
/// Використовується в sidebar для відображення правильних метаданих замість репрезентанта.
fn sidebar_details_override(app: &AppState) -> Option<api::AnimeDetails> {
    // Якщо sidebar_anime_idx встановлений — аніме є в search_results, нічого перевизначати
    if app.sidebar_anime_idx.is_some() {
        return None;
    }
    let details = app.current_details.clone()?;
    // ID репрезентанта групи (той, що показується по замовчуванню)
    let rep_id = app
        .selected_result_index
        .and_then(|i| app.search_results.get(i))
        .map(|a| a.id);
    // Якщо current_details для того самого аніме — перевизначення не потрібне
    if rep_id == Some(details.id) {
        return None;
    }
    Some(details)
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
