use ratatui::{
    layout::{Constraint, Direction, Layout, Rect, Alignment},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Clear},
    Frame,
};
use ratatui_image::{StatefulImage, protocol::StatefulProtocol};

use crate::ui::app::{AppMode, AppState, FocusPanel};
use crate::api;

const COLOR_PRIMARY:   Color = Color::Rgb(147, 51, 234);
const COLOR_SECONDARY: Color = Color::Rgb(168, 85, 247);
const COLOR_BG_DARK:   Color = Color::Rgb(17, 24, 39);
const COLOR_TEXT:      Color = Color::Rgb(243, 244, 246);
const COLOR_HIGHLIGHT: Color = Color::Rgb(59, 130, 246);
const COLOR_ERROR:     Color = Color::Rgb(239, 68, 68);
const COLOR_DIM:       Color = Color::Rgb(107, 114, 128);

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
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(70),
        ])
        .split(main_chunks[1]);

    render_sidebar(f, app, body_chunks[0]);
    render_lists(f, app, body_chunks[1]);
    render_status_bar(f, app, main_chunks[2]);

    if app.is_playing {
        render_popup(f, "▶ Відтворюється", "MPV запущено — виходь через q/Q в MPV", Color::Green);
    } else if app.loading {
        render_popup(f, "Завантаження...", "Будь ласка, зачекайте", COLOR_PRIMARY);
    } else if let Some(err) = app.error_msg.clone() {
        let msg = format!("{}\n\nEsc — закрити", err);
        render_popup(f, "Помилка", &msg, COLOR_ERROR);
    }
}

fn render_header(f: &mut Frame, app: &AppState, area: Rect) {
    let border_style = if app.mode == AppMode::SearchInput {
        Style::default().fg(COLOR_HIGHLIGHT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_PRIMARY)
    };

    let search_text = if app.search_query.is_empty() && app.mode != AppMode::SearchInput {
        vec![Span::styled("Натисніть '/' для пошуку аніме...", Style::default().fg(Color::DarkGray))]
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
                .title(Span::styled(" ANIHUB-CLI ", Style::default().fg(COLOR_PRIMARY).add_modifier(Modifier::BOLD)))
                .title_alignment(Alignment::Center),
        )
        .style(Style::default().bg(COLOR_BG_DARK))
        .alignment(search_align);

    f.render_widget(search_widget, area);

    if app.mode == AppMode::SearchInput {
        #[allow(clippy::cast_possible_truncation)]
        f.set_cursor_position((area.x + 4 + app.search_query.chars().count() as u16, area.y + 1));
    }
}

fn render_sidebar(f: &mut Frame, app: &mut AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_PRIMARY))
        .title(Span::styled(" Інформація ", Style::default().fg(COLOR_PRIMARY)))
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
            f.render_stateful_widget(StatefulImage::<StatefulProtocol>::default(), centered, poster);
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

fn render_sidebar_title_area(f: &mut Frame, app: &AppState, area: Rect, display_idx: Option<usize>) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some(idx) = display_idx {
        if let Some(item) = app.search_results.get(idx) {
            lines.push(
                Line::from(Span::styled(
                    item.title_ukrainian.as_str(),
                    Style::default().fg(COLOR_SECONDARY).add_modifier(Modifier::BOLD),
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
    let mk_sep = || Line::from(Span::styled("─".repeat(sep_w), Style::default().fg(COLOR_DIM)));
    let mut text: Vec<Line> = Vec::new();

    if let Some(idx) = display_idx {
        if let Some(item) = app.search_results.get(idx) {
            if include_title {
                text.push(
                    Line::from(Span::styled(
                        item.title_ukrainian.as_str(),
                        Style::default().fg(COLOR_SECONDARY).add_modifier(Modifier::BOLD),
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
                                    Span::styled(label, Style::default().fg(COLOR_SECONDARY).add_modifier(Modifier::BOLD)),
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
                        item.year.map(|y| y.to_string()).unwrap_or_else(|| "—".to_string()),
                        Style::default().fg(COLOR_TEXT),
                    ),
                ])
                .alignment(Alignment::Center),
            );

            let details = app.details_cache.get(&item.id)
                .or_else(|| if app.sidebar_anime_idx.is_none() { app.current_details.as_ref() } else { None });

            if let Some(d) = details {
                if let Some(rating) = d.rating {
                    text.push(
                        Line::from(vec![
                            Span::styled("Рейтинг: ", Style::default().fg(COLOR_DIM)),
                            Span::styled(
                                format!("{:.1} ⭐", rating),
                                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
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
                        let s = studios.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ");
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
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(22)])
        .split(area);

    let shortcuts = match app.focus {
        FocusPanel::SearchList  => "/ пошук  ↑↓ список  →/Enter вибір  q вихід",
        FocusPanel::SeasonList  => "↑↓ сезон  →/Enter озвучки  ←/Esc назад  q вихід",
        FocusPanel::DubbingList => "↑↓ озвучка  →/Enter серії  ←/Esc назад  q вихід",
        FocusPanel::EpisodeList => "↑↓ список  Enter відтворити  ←/Esc назад  q вихід",
    };

    let state = if app.is_playing {
        "▶ MPV"
    } else if app.loading {
        "⟳ Завантаження..."
    } else if app.prefetching {
        "⟳ Кешування..."
    } else {
        ""
    };

    f.render_widget(
        Paragraph::new(shortcuts)
            .style(Style::default().fg(COLOR_DIM).bg(COLOR_BG_DARK))
            .alignment(Alignment::Center),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(state)
            .style(Style::default().fg(COLOR_SECONDARY).bg(COLOR_BG_DARK))
            .alignment(Alignment::Right),
        chunks[1],
    );
}

fn render_lists(f: &mut Frame, app: &mut AppState, area: Rect) {
    let constraints = match app.focus {
        FocusPanel::SearchList  => vec![Constraint::Percentage(100)],
        FocusPanel::SeasonList  => vec![Constraint::Percentage(50), Constraint::Percentage(50)],
        FocusPanel::DubbingList => vec![Constraint::Percentage(33), Constraint::Percentage(34), Constraint::Percentage(33)],
        FocusPanel::EpisodeList => vec![Constraint::Percentage(25), Constraint::Percentage(25), Constraint::Percentage(25), Constraint::Percentage(25)],
    };
    let chunk_count = constraints.len();

    let list_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    if chunk_count >= 1 {
        let items: Vec<ListItem> = app.franchise_groups.iter().map(|group| {
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
            let mut lines = vec![Line::from(title)];
            if let Some(eng) = &rep.title_english {
                lines.push(Line::from(Span::styled(eng.as_str(), Style::default().fg(COLOR_DIM))));
            }
            ListItem::new(lines)
        }).collect();

        let list = create_list(" Результати пошуку ", items, app.focus == FocusPanel::SearchList);
        f.render_stateful_widget(list, list_chunks[0], &mut app.result_list_state);
    }

    if chunk_count >= 2 {
        let seasons = app.unique_seasons();
        let items: Vec<ListItem> = seasons.iter().map(|&sn| {
            let count = app.studios_for_season(sn).len();
            if count > 1 {
                ListItem::new(format!("Сезон {} ({} озвучок)", sn, count))
            } else {
                ListItem::new(format!("Сезон {}", sn))
            }
        }).collect();
        let list = create_list(" Сезони ", items, app.focus == FocusPanel::SeasonList);
        f.render_stateful_widget(list, list_chunks[1], &mut app.season_list_state);
    }

    if chunk_count >= 3 {
        let items: Vec<ListItem> = if let Some(sn) = app.selected_season_num() {
            app.studios_for_season(sn).iter().map(|s| {
                ListItem::new(format!("{} ({} серій)", s.studio_name, s.episodes_count))
            }).collect()
        } else {
            vec![]
        };
        let list = create_list(" Озвучки ", items, app.focus == FocusPanel::DubbingList);
        f.render_stateful_widget(list, list_chunks[2], &mut app.dubbing_list_state);
    }

    if chunk_count >= 4 {
        let items: Vec<ListItem> = if let Some(studio) = app.selected_studio() {
            studio.episodes.iter().map(|ep| {
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
                if title_lower.contains("частина") || title_lower.contains("chastina")
                    || title_lower.contains("part")
                {
                    // Витягуємо лише хвіст після останнього " - " або "(частина...)"
                    let suffix = ep.title
                        .rfind(" - ")
                        .map(|p| ep.title[p + 3..].trim())
                        .or_else(|| ep.title.rfind('(').map(|p| ep.title[p..].trim()))
                        .unwrap_or("");
                    if !suffix.is_empty() {
                        return ListItem::new(format!("Серія {} ({})", ep_label, suffix));
                    }
                }
                ListItem::new(format!("Серія {}", ep_label))
            }).collect()
        } else {
            vec![]
        };
        let list = create_list(" Серії ", items, app.focus == FocusPanel::EpisodeList);
        f.render_stateful_widget(list, list_chunks[3], &mut app.episode_list_state);
    }
}

fn create_list<'a>(title: &'a str, items: Vec<ListItem<'a>>, is_focused: bool) -> List<'a> {
    let border_style = if is_focused {
        Style::default().fg(COLOR_HIGHLIGHT)
    } else {
        Style::default().fg(COLOR_DIM)
    };

    List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title).border_style(border_style).title_alignment(Alignment::Center))
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
        if t.contains("ova") || t.contains("фільм") || t.contains("film")
            || t.contains("спец") || t.contains("special") || t.contains("movie")
        {
            other += 1;
        } else {
            tv += 1;
        }
    }
    (tv, other)
}
