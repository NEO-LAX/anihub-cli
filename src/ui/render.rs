use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph},
};
use ratatui_image::{StatefulImage, protocol::StatefulProtocol};

use crate::api;
use crate::storage::{AnimeStatus, LibraryReleaseKind};
use crate::ui::app::{
    AppMode, AppState, FocusPanel, LibraryFilter, PrimaryTab, SettingsChoiceKind, SettingsInput,
    SettingsTab, StatusKind, THRESHOLD_BAR_WIDTH, UpdateState,
};

const COLOR_PRIMARY: Color = Color::Rgb(147, 51, 234);
const COLOR_SECONDARY: Color = Color::Rgb(168, 85, 247);
const COLOR_BG_DARK: Color = Color::Rgb(17, 24, 39);
const COLOR_BG_TRANSPARENT: Color = Color::Reset;
const COLOR_TEXT: Color = Color::Rgb(243, 244, 246);
const COLOR_HIGHLIGHT: Color = Color::Rgb(59, 130, 246);
const COLOR_ERROR: Color = Color::Rgb(239, 68, 68);
const COLOR_DIM: Color = Color::Rgb(107, 114, 128);

pub fn render(f: &mut Frame, app: &mut AppState) {
    let size = f.area();
    // One tab row plus a compact context field. Breadcrumbs intentionally stay
    // out of the chrome: the active columns already show the same hierarchy.
    let header_h: u16 = if size.height >= 16 { 4 } else { 3 };
    let footer_h: u16 = if size.height >= 12 { 2 } else { 1 };

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Min(0),
            Constraint::Length(footer_h),
        ])
        .split(size);

    // Respect the terminal's own background/opacity instead of painting an
    // opaque full-frame wash.
    f.render_widget(
        Block::default().style(Style::default().bg(COLOR_BG_TRANSPARENT)),
        size,
    );

    render_header(f, app, main_chunks[0]);

    if app.mode == AppMode::Settings {
        render_settings(f, app, main_chunks[1]);
    } else if size.width >= 110 {
        let body_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
            .split(main_chunks[1]);
        render_sidebar(f, app, body_chunks[0]);
        render_lists(f, app, body_chunks[1]);
    } else {
        render_lists(f, app, main_chunks[1]);
    }
    render_status_bar(f, app, main_chunks[2]);

    if let Some((message, StatusKind::Error)) = app.status_message.clone() {
        render_error_popup(f, &message);
    } else if let Some((title, _)) = app.moonanime_browser_prompt.clone() {
        render_moonanime_popup(f, &title);
    } else if app.status_editor.is_some() {
        render_status_editor_popup(f, app);
    } else if app.clear_library_confirmation {
        render_clear_library_popup(f);
    } else if app.settings_update_popup {
        render_settings_update_popup(f, app);
    } else if app.settings_input.is_some() {
        render_settings_text_popup(f, app);
    } else if app.settings_threshold.is_some() {
        render_settings_threshold_popup(f, app);
    } else if app.settings_choice.is_some() {
        render_settings_choice_popup(f, app);
    } else if let Some((_, anime_title)) = app.pending_delete_confirmation.clone() {
        render_delete_popup(f, &anime_title);
    } else if app.show_help {
        render_help_popup(f);
    }
}

fn render_header(f: &mut Frame, app: &AppState, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(2)])
        .split(area);

    let mut top: Vec<Span> = Vec::new();
    for (index, tab) in PrimaryTab::ALL.iter().enumerate() {
        if index > 0 {
            top.push(Span::styled(" | ", Style::default().fg(COLOR_DIM)));
        }
        let active = *tab == app.primary_tab();
        top.push(Span::styled(
            format!(" {} · {} ", index + 1, tab.label()),
            if active {
                Style::default()
                    .fg(COLOR_TEXT)
                    .bg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_DIM)
            },
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(top))
            .alignment(Alignment::Center)
            .style(Style::default().bg(COLOR_BG_TRANSPARENT)),
        rows[0],
    );

    let editing = app.mode == AppMode::SearchInput || app.library_search_editing;
    let (title, context, alignment) = match app.primary_tab() {
        PrimaryTab::Search => (
            if app.settings.search_mode.is_extended() {
                " Пошук · / · розширений "
            } else {
                " Пошук · / "
            },
            search_header_context(app),
            // Left while typing so the cursor matches the glyph under it;
            // center when idle so the field looks framed and balanced.
            if editing {
                Alignment::Left
            } else {
                Alignment::Center
            },
        ),
        PrimaryTab::Library
            if app.library_search_editing || !app.library_search_query.is_empty() =>
        {
            (
                " Пошук у бібліотеці · / ",
                library_search_header_context(app),
                if app.library_search_editing {
                    Alignment::Left
                } else {
                    Alignment::Center
                },
            )
        }
        PrimaryTab::Library => (
            " Категорії · Tab ",
            library_filter_context(app),
            Alignment::Center,
        ),
        PrimaryTab::Settings => (
            " Вкладки · Tab ",
            settings_tabs_context(app),
            Alignment::Center,
        ),
    };

    let context_border = if editing { COLOR_HIGHLIGHT } else { COLOR_DIM };

    let context_area = rows[1];
    if context_area.height >= 3 {
        f.render_widget(
            Paragraph::new(context)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(title)
                        .title_alignment(Alignment::Center)
                        .border_style(Style::default().fg(context_border))
                        .padding(Padding::horizontal(1))
                        .style(Style::default().bg(COLOR_BG_TRANSPARENT)),
                )
                .alignment(alignment)
                .style(Style::default().bg(COLOR_BG_TRANSPARENT).fg(COLOR_TEXT)),
            context_area,
        );
        if editing {
            // Cursor sits after the left border + one padding space.
            let visible = active_search_cursor(app);
            #[allow(clippy::cast_possible_truncation)]
            let col = 1u16
                .saturating_add(1)
                .saturating_add(visible as u16)
                .min(context_area.width.saturating_sub(2));
            f.set_cursor_position((context_area.x + col, context_area.y + 1));
        }
    } else {
        f.render_widget(
            Paragraph::new(context)
                .alignment(Alignment::Center)
                .style(Style::default().bg(COLOR_BG_TRANSPARENT)),
            context_area,
        );
        if editing {
            let visible = active_search_cursor(app);
            #[allow(clippy::cast_possible_truncation)]
            let col = visible as u16;
            f.set_cursor_position((
                context_area.x + col.min(context_area.width.saturating_sub(1)),
                context_area.y,
            ));
        }
    }
}

fn active_search_cursor(app: &AppState) -> usize {
    if app.library_search_editing {
        app.library_search_cursor
    } else {
        app.search_cursor
    }
}

fn settings_tabs_context(app: &AppState) -> Line<'static> {
    let tabs = [SettingsTab::General, SettingsTab::About];
    let mut spans = Vec::new();
    for (index, tab) in tabs.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  |  ", Style::default().fg(COLOR_DIM)));
        }
        let active = tab == app.settings_tab;
        spans.push(Span::styled(
            format!(" {} ", tab.label()),
            if active {
                Style::default()
                    .fg(COLOR_TEXT)
                    .bg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_DIM)
            },
        ));
    }
    Line::from(spans)
}

fn search_header_context(app: &AppState) -> Line<'static> {
    let query = if app.mode == AppMode::SearchInput {
        app.search_query.as_str()
    } else {
        app.last_search_query.as_str()
    };
    if query.is_empty() {
        Line::from(Span::styled(
            if app.mode == AppMode::SearchInput {
                "введіть назву аніме…"
            } else {
                "Введіть назву аніме…"
            },
            Style::default().fg(COLOR_DIM),
        ))
    } else {
        Line::from(Span::styled(
            query.to_string(),
            Style::default()
                .fg(COLOR_TEXT)
                .add_modifier(if app.mode == AppMode::SearchInput {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ))
    }
}

fn library_search_header_context(app: &AppState) -> Line<'static> {
    if app.library_search_query.is_empty() {
        Line::from(Span::styled(
            "введіть назву аніме у бібліотеці…",
            Style::default().fg(COLOR_DIM),
        ))
    } else {
        Line::from(Span::styled(
            app.library_search_query.clone(),
            Style::default()
                .fg(COLOR_TEXT)
                .add_modifier(if app.library_search_editing {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ))
    }
}

fn library_filter_context(app: &AppState) -> Line<'static> {
    let spans = LibraryFilter::ALL
        .iter()
        .flat_map(|filter| {
            let active = *filter == app.library_filter;
            let label = filter.label();
            let style = if active {
                Style::default()
                    .fg(COLOR_TEXT)
                    .bg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_DIM)
            };
            [
                Span::styled(format!("  {label}  "), style),
                Span::styled("  ", Style::default().fg(COLOR_DIM)),
            ]
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn search_sidebar_tracking_context(app: &AppState) -> (Vec<u32>, Option<u32>) {
    if app.focus == FocusPanel::SearchList {
        if let Some(group_index) = app.selected_group_index {
            let mut anime_ids = app
                .franchise_groups
                .get(group_index)
                .into_iter()
                .flatten()
                .filter_map(|index| app.search_results.get(*index).map(|anime| anime.id))
                .collect::<Vec<_>>();
            let mainline_ids = app
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
            app.selected_result_index
                .and_then(|index| app.search_results.get(index).map(|anime| anime.id))
        })
        .into_iter()
        .collect::<Vec<_>>();
    let total = anime_ids.first().and_then(|id| {
        app.details_cache
            .get(id)
            .and_then(|details| details.episodes_count)
            .or_else(|| {
                app.search_results
                    .iter()
                    .find(|anime| anime.id == *id)
                    .and_then(|anime| anime.episodes_count)
            })
    });
    (anime_ids, total)
}

fn library_sidebar_tracking_context(
    app: &AppState,
) -> (Vec<u32>, Option<AnimeStatus>, Option<u32>) {
    let Some(anime) = app.library_selected_anime() else {
        return (Vec::new(), None, None);
    };
    let totals = anime
        .anime_ids
        .iter()
        .filter_map(|anime_id| {
            app.details_cache
                .get(anime_id)
                .and_then(|details| details.episodes_count)
        })
        .collect::<Vec<_>>();
    let total = if totals.is_empty() {
        app.current_details
            .as_ref()
            .and_then(|details| details.episodes_count)
    } else {
        Some(totals.into_iter().sum())
    };
    (anime.anime_ids.clone(), Some(anime.status), total)
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
            .and_then(|i| app.search_results.get(i))
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
                truncate_with_ellipsis(&release.title, area.width as usize),
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        );
    } else if let Some(d) = sidebar_details_override(app) {
        lines.push(
            Line::from(Span::styled(
                truncate_with_ellipsis(&d.title_ukrainian, area.width as usize),
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        );
        if let Some(eng) = &d.title_english {
            lines.push(
                Line::from(Span::styled(
                    truncate_with_ellipsis(eng, area.width as usize),
                    Style::default().fg(COLOR_DIM),
                ))
                .alignment(Alignment::Center),
            );
        }
    } else if let Some(idx) = display_idx {
        if let Some(item) = app.search_results.get(idx) {
            lines.push(
                Line::from(Span::styled(
                    truncate_with_ellipsis(&item.title_ukrainian, area.width as usize),
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            if let Some(eng) = &item.title_english {
                lines.push(
                    Line::from(Span::styled(
                        truncate_with_ellipsis(eng, area.width as usize),
                        Style::default().fg(COLOR_DIM),
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
            Style::default().fg(COLOR_DIM),
        ))
    };
    let mut text: Vec<Line> = Vec::new();

    if let Some(release) = selected_release_for_sidebar(app) {
        if include_title {
            text.push(
                Line::from(Span::styled(
                    truncate_with_ellipsis(&release.title, area.width as usize),
                    Style::default()
                        .fg(COLOR_SECONDARY)
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
                        .fg(COLOR_ERROR)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
        }
        if let Some(genres) = &release.genres {
            if !genres.is_empty() {
                text.push(Line::from(""));
                text.push(Line::from(vec![
                    Span::styled("Жанри: ", Style::default().fg(COLOR_DIM)),
                    Span::styled(
                        summarized_genres(genres),
                        Style::default().fg(COLOR_HIGHLIGHT),
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
                        .fg(COLOR_SECONDARY)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            if let Some(eng) = &d.title_english {
                text.push(
                    Line::from(Span::styled(
                        truncate_with_ellipsis(eng, area.width as usize),
                        Style::default().fg(COLOR_DIM),
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
                    Span::styled(
                        summarized_genres(genres),
                        Style::default().fg(COLOR_HIGHLIGHT),
                    ),
                ]));
            }
        }
    } else if let Some(idx) = display_idx {
        if let Some(item) = app.search_results.get(idx) {
            if include_title {
                text.push(
                    Line::from(Span::styled(
                        truncate_with_ellipsis(&item.title_ukrainian, area.width as usize),
                        Style::default()
                            .fg(COLOR_SECONDARY)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .alignment(Alignment::Center),
                );
                if let Some(eng) = &item.title_english {
                    text.push(
                        Line::from(Span::styled(
                            truncate_with_ellipsis(eng, area.width as usize),
                            Style::default().fg(COLOR_DIM),
                        ))
                        .alignment(Alignment::Center),
                    );
                }
                text.push(mk_sep());
            }

            let details = app.details_cache.get(&item.id).or_else(|| {
                if sidebar_is_representative(app) {
                    app.current_details.clone()
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
                            Span::styled(
                                summarized_genres(genres),
                                Style::default().fg(COLOR_HIGHLIGHT),
                            ),
                        ]));
                    }
                }
            } else if app.loading {
                text.push(
                    Line::from(Span::styled(
                        "Завантаження деталей…",
                        Style::default().fg(COLOR_DIM),
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
                    .bg(COLOR_BG_TRANSPARENT)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center),
        rows[0],
    );

    if rows.len() >= 2 {
        // "anihub-cli | vX" — hub accented in app purple.
        let brand_w = format!(" anihub-cli | v{} ", env!("CARGO_PKG_VERSION"))
            .chars()
            .count()
            .max(18) as u16;

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(brand_w),
                Constraint::Min(1),
                Constraint::Length(brand_w),
            ])
            .split(rows[1]);

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    " ani",
                    Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "hub",
                    Style::default()
                        .fg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "-cli",
                    Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" | v{} ", env!("CARGO_PKG_VERSION")),
                    Style::default().fg(COLOR_DIM),
                ),
            ]))
            .style(Style::default().bg(COLOR_BG_TRANSPARENT))
            .alignment(Alignment::Left),
            columns[0],
        );
        // Centered framed keybinds: │ Enter Далі  ·  e Статус │
        f.render_widget(
            Paragraph::new(framed_shortcuts_line(&context_shortcuts(app)))
                .style(Style::default().bg(COLOR_BG_TRANSPARENT))
                .alignment(Alignment::Center),
            columns[1],
        );
        // Symmetric empty side keeps the bind strip visually centered.
        f.render_widget(
            Paragraph::new("").style(Style::default().bg(COLOR_BG_TRANSPARENT)),
            columns[2],
        );
    }
}

/// Build `│  key Action  ·  key Action  │` with purple rails.
fn framed_shortcuts_line(shortcuts: &str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> =
        vec![Span::styled("│ ", Style::default().fg(COLOR_PRIMARY))];
    let parts: Vec<&str> = shortcuts
        .split("  ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(COLOR_DIM)));
        }
        // First token is the key chord, rest is the label.
        let mut tokens = part.splitn(2, ' ');
        let key = tokens.next().unwrap_or("");
        let label = tokens.next().unwrap_or("");
        spans.push(Span::styled(
            key.to_string(),
            Style::default()
                .fg(COLOR_SECONDARY)
                .add_modifier(Modifier::BOLD),
        ));
        if !label.is_empty() {
            spans.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(COLOR_DIM),
            ));
        }
    }
    spans.push(Span::styled(" │", Style::default().fg(COLOR_PRIMARY)));
    Line::from(spans)
}

fn context_shortcuts(app: &AppState) -> String {
    if app.mode == AppMode::Settings {
        if app.settings_update_popup {
            return "Enter Дія  Esc Закрити  Ctrl+C Вихід".to_string();
        }
        if app.settings_input.is_some() {
            return "Enter Зберегти  Esc Скасувати  Ctrl+C Вихід".to_string();
        }
        return "↑↓ Вибір  Space Змінити  Enter Дія  Tab Вкладка  Esc Назад".to_string();
    }
    if app.mode == AppMode::SearchInput {
        return "Enter Знайти  Esc Скасувати  Alt+2 Бібліотека  Ctrl+C Вихід".to_string();
    }
    if app.library_search_editing {
        return "Enter Застосувати  Esc Очистити  Tab Категорія  Ctrl+C Вихід".to_string();
    }
    if app.is_library_mode() {
        return match app.mode {
            AppMode::Library => {
                "Tab Категорія  Enter Відкрити  e Статус  c Продовжити  / Пошук бібліотеки"
                    .to_string()
            }
            AppMode::LibrarySeason | AppMode::LibraryDubbing => {
                "Enter Далі  Space Переглянуто  e Статус  Esc Назад".to_string()
            }
            AppMode::LibraryEpisode => {
                if app
                    .selected_dubbing_choice()
                    .is_some_and(|choice| choice.is_moonanime())
                {
                    "Enter Embed  e Статус  Esc Назад".to_string()
                } else {
                    "Enter Відтворити  Space Переглянуто  Backspace Таймкод  e Статус".to_string()
                }
            }
            _ => String::new(),
        };
    }
    match app.focus {
        FocusPanel::SearchList => {
            "Enter Далі  e Статус  c Продовжити  / Пошук  2 Бібліотека".to_string()
        }
        FocusPanel::ReleaseList
            if app.has_release_catalog() && !app.selected_release_available() =>
        {
            "Недоступно на AniHub  Esc Назад".to_string()
        }
        FocusPanel::ReleaseList | FocusPanel::DubbingList => {
            "Enter Далі  Space Переглянуто  e Статус  Esc Назад".to_string()
        }
        FocusPanel::EpisodeList => {
            if app
                .selected_dubbing_choice()
                .is_some_and(|choice| choice.is_moonanime())
            {
                "Enter Embed  e Статус  Esc Назад".to_string()
            } else {
                "Enter Відтворити  Space Переглянуто  Backspace Таймкод  e Статус".to_string()
            }
        }
    }
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
            let focused = app.focus == FocusPanel::SearchList;
            let border_style = if focused {
                Style::default().fg(COLOR_HIGHLIGHT)
            } else {
                Style::default().fg(COLOR_DIM)
            };
            let title_style = if focused {
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_DIM)
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" Результати пошуку ", title_style))
                .title_alignment(Alignment::Center)
                .border_style(border_style);
            let inner = block.inner(list_chunks[0]);
            f.render_widget(block, list_chunks[0]);

            let message = if app.activity_message.is_some() {
                "Шукаємо…"
            } else if app.last_search_query.is_empty() {
                "Натисніть / щоб шукати"
            } else {
                "Нічого не знайдено"
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
                Paragraph::new(Span::styled(message, Style::default().fg(COLOR_DIM)))
                    .alignment(Alignment::Center),
                centered[1],
            );
        } else {
            let list = create_list(
                " Результати пошуку ",
                items,
                app.focus == FocusPanel::SearchList,
            );
            f.render_stateful_widget(list, list_chunks[0], &mut app.result_list_state);
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
                    let year_str = season_year(app, sn).map(|y| y.to_string());
                    let mut metadata = year_str.into_iter().collect::<Vec<_>>();
                    if count > 1 {
                        metadata.push(format!("{count} озвучок"));
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
            let list = create_list(" Випуски ", items, app.focus == FocusPanel::ReleaseList);
            f.render_stateful_widget(list, list_chunks[idx], &mut app.season_list_state);
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
        let list = create_list(" Озвучки ", items, app.focus == FocusPanel::DubbingList);
        f.render_stateful_widget(list, list_chunks[idx], &mut app.dubbing_list_state);
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

    let library_title = format!(" {} ", app.library_filter.label());
    if app.library_items.is_empty() {
        let border_style = if app.mode == AppMode::Library {
            Style::default().fg(COLOR_HIGHLIGHT)
        } else {
            Style::default().fg(COLOR_DIM)
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
            Paragraph::new(Span::styled(message, Style::default().fg(COLOR_DIM)))
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
                let mut metadata = release
                    .episodes_count
                    .map(|count| format!("{count} сер."))
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
        let season_list = create_list(
            " Випуски ",
            season_items,
            app.mode == AppMode::LibrarySeason,
        );
        f.render_stateful_widget(season_list, chunks[1], &mut app.season_list_state);
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
                truncate_with_ellipsis(&details.title_ukrainian, area.width as usize),
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        );
        if let Some(eng) = &details.title_english {
            lines.push(
                Line::from(Span::styled(
                    truncate_with_ellipsis(eng, area.width as usize),
                    Style::default().fg(COLOR_DIM),
                ))
                .alignment(Alignment::Center),
            );
        }
    } else if let Some(anime) = app.library_selected_anime() {
        lines.push(
            Line::from(Span::styled(
                truncate_with_ellipsis(&anime.anime_title, area.width as usize),
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
                    truncate_with_ellipsis(&details.title_ukrainian, area.width as usize),
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
            if let Some(eng) = &details.title_english {
                text.push(
                    Line::from(Span::styled(
                        truncate_with_ellipsis(eng, area.width as usize),
                        Style::default().fg(COLOR_DIM),
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
                    Span::styled(
                        summarized_genres(genres),
                        Style::default().fg(COLOR_HIGHLIGHT),
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
                        .fg(COLOR_SECONDARY)
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
                    Style::default().fg(COLOR_DIM),
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

fn render_centered_sidebar_message(f: &mut Frame, area: Rect, message: &str) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new(Span::styled(
            truncate_with_ellipsis(message, area.width as usize),
            Style::default().fg(COLOR_DIM),
        ))
        .alignment(Alignment::Center),
        rows[1],
    );
}

fn format_elapsed_timestamp(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    if total >= 60 {
        format!("{}:{:02}", total / 60, total % 60)
    } else {
        format!("{}с", total)
    }
}

fn label_with_metadata(label: &str, metadata: &[String]) -> String {
    if metadata.is_empty() {
        label.to_string()
    } else {
        format!("{label} [{}]", metadata.join(" · "))
    }
}

fn sidebar_poster_height(inner: Rect, title_height: u16) -> u16 {
    // Keep the poster compact so its reserved row does not push status and
    // metadata halfway down a tall terminal.
    let width_based = inner.width / 2;
    let height_based = inner.height.saturating_sub(title_height + 8);
    width_based.min(height_based).max(3)
}

fn season_count_label(count: usize) -> String {
    let suffix = if count % 10 == 1 && count % 100 != 11 {
        "сезон"
    } else if (2..=4).contains(&(count % 10)) && !(12..=14).contains(&(count % 100)) {
        "сезони"
    } else {
        "сезонів"
    };
    format!("{count} {suffix}")
}

fn truncate_with_ellipsis(text: &str, width: usize) -> String {
    let length = text.chars().count();
    if length <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    format!("{}…", text.chars().take(width - 1).collect::<String>())
}

fn compact_metadata_line(
    anime_type: &str,
    year: Option<u32>,
    rating: Option<f32>,
    episodes: Option<String>,
) -> Line<'static> {
    let mut values: Vec<(String, Style)> =
        vec![(anime_type.to_uppercase(), Style::default().fg(COLOR_TEXT))];
    if let Some(year) = year {
        values.push((year.to_string(), Style::default().fg(COLOR_TEXT)));
    }
    if let Some(rating) = rating {
        values.push((
            format!("★ {rating:.1}"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(episodes) = episodes {
        values.push((format!("{episodes} сер."), Style::default().fg(COLOR_TEXT)));
    }

    let mut spans = Vec::new();
    for (index, (value, style)) in values.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(COLOR_DIM)));
        }
        spans.push(Span::styled(value, style));
    }
    Line::from(spans).alignment(Alignment::Center)
}

fn summarized_genres(genres: &[String]) -> String {
    const VISIBLE_GENRES: usize = 4;
    let visible = genres
        .iter()
        .take(VISIBLE_GENRES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" · ");
    let hidden = genres.len().saturating_sub(VISIBLE_GENRES);
    if hidden > 0 {
        format!("{visible} · +{hidden}")
    } else {
        visible
    }
}

fn latest_progress_for_ids<'a>(
    app: &'a AppState,
    anime_ids: &[u32],
) -> Option<&'a crate::storage::WatchProgress> {
    app.history
        .progress
        .values()
        .filter(|progress| anime_ids.contains(&progress.anime_id))
        .max_by_key(|progress| progress.updated_at)
}

fn watched_episode_count(app: &AppState, anime_ids: &[u32]) -> usize {
    app.history
        .progress
        .values()
        .filter(|progress| progress.watched && anime_ids.contains(&progress.anime_id))
        .map(|progress| (progress.anime_id, progress.season, progress.episode))
        .collect::<std::collections::HashSet<_>>()
        .len()
}

fn tracking_lines(
    app: &AppState,
    anime_ids: &[u32],
    explicit_status: Option<AnimeStatus>,
    total_episodes: Option<u32>,
) -> Vec<Line<'static>> {
    if anime_ids.is_empty() {
        return Vec::new();
    }

    let status = explicit_status
        .or_else(|| anime_status_for_ids(app, anime_ids))
        .unwrap_or(AnimeStatus::NotAdded);
    let watched = watched_episode_count(app, anime_ids);
    let watched_label = total_episodes.map_or_else(
        || format!("✓ {watched} сер."),
        |total| format!("✓ {watched}/{total} сер."),
    );
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} {}", anime_status_marker(status), status.label())
                    .trim_start()
                    .to_string(),
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(COLOR_DIM)),
            Span::styled(watched_label, Style::default().fg(COLOR_TEXT)),
        ])
        .alignment(Alignment::Center),
    ];
    if let Some(progress) = latest_progress_for_ids(app, anime_ids) {
        lines.push(
            Line::from(Span::styled(
                format!(
                    "⏱ S{}E{} · {}",
                    progress.season,
                    progress.episode,
                    format_timestamp(progress.timestamp)
                ),
                Style::default().fg(COLOR_HIGHLIGHT),
            ))
            .alignment(Alignment::Center),
        );
    }
    lines
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
    if app
        .history
        .library
        .get(&anime_id)
        .is_some_and(|record| record.status == AnimeStatus::Completed)
    {
        return true;
    }
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
    let title_style = if is_focused {
        Style::default()
            .fg(COLOR_SECONDARY)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_DIM)
    };

    List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(title, title_style))
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

fn render_settings(f: &mut Frame, app: &AppState, area: Rect) {
    match app.settings_tab {
        SettingsTab::General => render_general_settings(f, app, area),
        SettingsTab::About => render_about_settings(f, app, area),
    }
}

fn settings_item(label: &str, value: &str, width: usize) -> ListItem<'static> {
    let value_width = value.chars().count();
    let label_width = width.saturating_sub(value_width + 3).max(8);
    let label = truncate_with_ellipsis(label, label_width);
    ListItem::new(with_right_marker(&label, value, width))
}

fn render_general_settings(f: &mut Frame, app: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_HIGHLIGHT))
        .title(" Основні ")
        .title_alignment(Alignment::Center)
        .padding(Padding::horizontal(2));
    let inner_width = block.inner(area).width.saturating_sub(4) as usize;
    let on_off = |enabled| {
        if enabled {
            "увімкнено"
        } else {
            "вимкнено"
        }
    };
    let threshold = format_threshold_value(app.settings.watched_threshold_percent);
    let items = vec![
        settings_item(
            "Авто-продовження наступної серії",
            on_off(app.settings.autoplay_next),
            inner_width,
        ),
        settings_item(
            "Resume з таймкоду",
            on_off(app.settings.resume_from_timestamp),
            inner_width,
        ),
        settings_item("Позначати переглянутим", &threshold, inner_width),
        settings_item(
            "Режим пошуку",
            app.settings.search_mode.label(),
            inner_width,
        ),
        settings_item(
            "Стартовий екран",
            app.settings.start_screen.label(),
            inner_width,
        ),
        settings_item(
            "Фільтр бібліотеки за замовчуванням",
            app.settings.default_library_filter.label(),
            inner_width,
        ),
        settings_item(
            "Показувати постери",
            on_off(app.settings.show_posters),
            inner_width,
        ),
        settings_item("Шлях до mpv", &app.settings.mpv_path, inner_width),
        settings_item(
            "Додаткові аргументи mpv",
            if app.settings.mpv_extra_args.is_empty() {
                "—"
            } else {
                &app.settings.mpv_extra_args
            },
            inner_width,
        ),
    ];
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(
        app.settings_selected.min(items.len().saturating_sub(1)),
    ));
    let list = List::new(items)
        .block(block)
        .highlight_symbol(">> ")
        .highlight_style(
            Style::default()
                .bg(COLOR_PRIMARY)
                .fg(COLOR_TEXT)
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(list, area, &mut state);
}

fn render_about_settings(f: &mut Frame, app: &AppState, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(5)])
        .split(area);
    let action_width = chunks[0].width.saturating_sub(8) as usize;
    let actions = vec![
        settings_item("Тека даних", "", action_width),
        settings_item("GitHub", "", action_width),
        settings_item("Перевірити оновлення", "", action_width),
        settings_item("Очистити бібліотеку", "", action_width),
    ];
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(app.settings_selected.min(actions.len() - 1)));
    f.render_stateful_widget(
        List::new(actions)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Дії ")
                    .title_alignment(Alignment::Center)
                    .border_style(Style::default().fg(COLOR_HIGHLIGHT))
                    .padding(Padding::horizontal(2)),
            )
            .highlight_symbol(">> ")
            .highlight_style(
                Style::default()
                    .bg(COLOR_PRIMARY)
                    .fg(COLOR_TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
        chunks[0],
        &mut state,
    );

    let diagnostics = vec![
        Line::from(vec![
            Span::styled("Версія: ", Style::default().fg(COLOR_DIM)),
            Span::styled(env!("CARGO_PKG_VERSION"), Style::default().fg(COLOR_TEXT)),
        ]),
        Line::from(vec![
            Span::styled("GitHub: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                crate::settings::GITHUB_URL,
                Style::default().fg(COLOR_HIGHLIGHT),
            ),
        ]),
        Line::from(vec![
            Span::styled("History: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                app.settings_store.history_path().display().to_string(),
                Style::default().fg(COLOR_TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("Settings: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                app.settings_store.settings_path().display().to_string(),
                Style::default().fg(COLOR_TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("Cache: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                app.metadata_cache.path().display().to_string(),
                Style::default().fg(COLOR_TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("mpv: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                if app.mpv_available {
                    "знайдено"
                } else {
                    "не знайдено"
                },
                Style::default().fg(if app.mpv_available {
                    Color::Green
                } else {
                    COLOR_ERROR
                }),
            ),
            Span::styled(" · image: ", Style::default().fg(COLOR_DIM)),
            Span::styled(app.image_protocol.clone(), Style::default().fg(COLOR_TEXT)),
        ]),
    ];
    f.render_widget(
        Paragraph::new(diagnostics)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Про / шляхи ")
                    .title_alignment(Alignment::Center)
                    .border_style(Style::default().fg(COLOR_DIM))
                    .padding(Padding::horizontal(2)),
            )
            .wrap(ratatui::widgets::Wrap { trim: true }),
        chunks[1],
    );
}

fn format_threshold_value(percent: Option<u8>) -> String {
    match percent {
        None => "вимкнено".to_string(),
        Some(value) => format!(
            "{} {}%",
            threshold_bar(Some(value), THRESHOLD_BAR_WIDTH),
            value
        ),
    }
}

fn threshold_bar(percent: Option<u8>, width: usize) -> String {
    let width = width.max(4);
    match percent {
        None => format!("[{}]", "-".repeat(width)),
        Some(value) => {
            let filled = ((u16::from(value) * width as u16) / 100).min(width as u16) as usize;
            format!("[{}{}]", "=".repeat(filled), "-".repeat(width - filled))
        }
    }
}

fn render_settings_choice_popup(f: &mut Frame, app: &AppState) {
    let Some(editor) = app.settings_choice.as_ref() else {
        return;
    };
    let labels = editor.kind.option_labels();
    let rows = labels.len() as u16;
    let height = rows.saturating_add(5).max(8);
    let actions = [
        ("↑/↓", "Вибір", COLOR_SECONDARY),
        ("Enter", "OK", COLOR_HIGHLIGHT),
        ("Esc", "", COLOR_DIM),
    ];
    let min_width = match editor.kind {
        SettingsChoiceKind::StartScreen => 36,
        SettingsChoiceKind::LibraryFilter => 40,
    };
    let area = centered_fixed(f.area(), dialog_width_for(min_width, &actions), height);
    let block = dialog_block(editor.kind.title(), COLOR_PRIMARY, COLOR_SECONDARY);
    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(rows),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let label_w = labels
        .iter()
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0);
    let lines = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let selected = index == editor.selected;
            let radio = if selected { "●" } else { "○" };
            let style = if selected {
                Style::default()
                    .fg(COLOR_TEXT)
                    .bg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_DIM)
            };
            Line::from(Span::styled(
                format!(" {radio}  {} ", pad_display(label, label_w)),
                style,
            ))
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        layout[1],
    );
    f.render_widget(
        Paragraph::new(action_footer_line(&actions)).alignment(Alignment::Center),
        layout[3],
    );
}

fn render_settings_text_popup(f: &mut Frame, app: &AppState) {
    let Some(kind) = app.settings_input else {
        return;
    };
    let (title, hint) = match kind {
        SettingsInput::MpvPath => (" Шлях до mpv ", "Порожнє значення скинеться на «mpv»"),
        SettingsInput::MpvArgs => (" Аргументи mpv ", "Наприклад: --fs --hwdec=auto"),
    };
    let actions = [
        ("Enter", "Зберегти", COLOR_HIGHLIGHT),
        ("Esc", "", COLOR_DIM),
    ];
    let area = centered_fixed(f.area(), dialog_width_for(56, &actions), 10);
    let block = dialog_block(title, COLOR_HIGHLIGHT, COLOR_SECONDARY);
    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(Span::styled(hint, Style::default().fg(COLOR_DIM)))
            .alignment(Alignment::Center),
        layout[0],
    );

    let value = if app.settings_input_value.is_empty() {
        " ".to_string()
    } else {
        app.settings_input_value.clone()
    };
    let field = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_HIGHLIGHT))
        .style(Style::default().bg(COLOR_BG_TRANSPARENT));
    let field_inner = field.inner(layout[1]);
    f.render_widget(field, layout[1]);
    f.render_widget(
        Paragraph::new(Span::styled(
            value,
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        )),
        field_inner,
    );
    #[allow(clippy::cast_possible_truncation)]
    f.set_cursor_position((
        field_inner.x + (app.settings_input_cursor as u16).min(field_inner.width.saturating_sub(1)),
        field_inner.y,
    ));

    f.render_widget(
        Paragraph::new(action_footer_line(&actions)).alignment(Alignment::Center),
        layout[3],
    );
}

fn render_settings_update_popup(f: &mut Frame, app: &AppState) {
    let actions_probe = [
        ("Enter", "Відкрити реліз", COLOR_HIGHLIGHT),
        ("Esc", "", COLOR_DIM),
    ];
    let area = centered_fixed(f.area(), dialog_width_for(44, &actions_probe), 11);
    let block = dialog_block(" Оновлення ", COLOR_PRIMARY, COLOR_SECONDARY);
    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(Span::styled(
            format!("Поточна версія: {}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(COLOR_DIM),
        ))
        .alignment(Alignment::Center),
        layout[0],
    );

    let (body, actions): (Vec<Line>, Vec<(&str, &str, Color)>) = match &app.update_state {
        UpdateState::Checking | UpdateState::Idle => (
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Перевіряємо оновлення…",
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .add_modifier(Modifier::BOLD),
                )),
            ],
            vec![("Esc", "", COLOR_DIM)],
        ),
        UpdateState::Current(version) => (
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "У вас актуальна версія",
                    Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    version.clone(),
                    Style::default().fg(COLOR_SECONDARY),
                )),
            ],
            vec![("Enter", "Ще раз", COLOR_HIGHLIGHT), ("Esc", "", COLOR_DIM)],
        ),
        UpdateState::Available(update) => (
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Доступна нова версія",
                    Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    update.latest_version.clone(),
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .add_modifier(Modifier::BOLD),
                )),
            ],
            vec![("Enter", "Реліз", COLOR_HIGHLIGHT), ("Esc", "", COLOR_DIM)],
        ),
        UpdateState::Failed(error) => (
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Не вдалося перевірити",
                    Style::default()
                        .fg(COLOR_ERROR)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    truncate_middle(error, 42),
                    Style::default().fg(COLOR_DIM),
                )),
            ],
            vec![("Enter", "Ще раз", COLOR_HIGHLIGHT), ("Esc", "", COLOR_DIM)],
        ),
    };

    f.render_widget(Paragraph::new(body).alignment(Alignment::Center), layout[1]);
    f.render_widget(
        Paragraph::new(action_footer_line(&actions)).alignment(Alignment::Center),
        layout[3],
    );
}

fn render_settings_threshold_popup(f: &mut Frame, app: &AppState) {
    let Some(editor) = app.settings_threshold.as_ref() else {
        return;
    };
    let actions = [("Enter", "OK", COLOR_HIGHLIGHT), ("Esc", "", COLOR_DIM)];
    let area = centered_fixed(f.area(), dialog_width_for(46, &actions), 11);
    let block = dialog_block(" Позначати переглянутим ", COLOR_PRIMARY, COLOR_SECONDARY);
    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(Span::styled(
            "Поріг прогресу серії",
            Style::default().fg(COLOR_DIM),
        ))
        .alignment(Alignment::Center),
        layout[0],
    );

    let bar = threshold_bar(editor.percent, THRESHOLD_BAR_WIDTH);
    let value = match editor.percent {
        Some(p) => format!("{bar}  {p}%"),
        None => format!("{bar}  вимкнено"),
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            value,
            Style::default()
                .fg(if editor.percent.is_some() {
                    COLOR_SECONDARY
                } else {
                    COLOR_DIM
                })
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
        layout[2],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            if editor.percent.is_some() {
                "←/→ крок 5%   Space — вимкнути"
            } else {
                "Space — увімкнути   ←/→ — виставити"
            },
            Style::default().fg(COLOR_DIM),
        ))
        .alignment(Alignment::Center),
        layout[3],
    );

    f.render_widget(
        Paragraph::new(action_footer_line(&actions)).alignment(Alignment::Center),
        layout[5],
    );
}

fn render_moonanime_popup(f: &mut Frame, episode_title: &str) {
    let body = vec![
        Line::from(Span::styled(
            truncate_middle(episode_title, 44),
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Епізод відкриється в браузері",
            Style::default().fg(COLOR_DIM),
        )),
        Line::from(Span::styled(
            "(MoonAnime embed)",
            Style::default().fg(COLOR_DIM),
        )),
    ];
    render_confirm_dialog(
        f,
        " MoonAnime ",
        COLOR_HIGHLIGHT,
        &body,
        &[
            ("Enter", "Відкрити", COLOR_HIGHLIGHT),
            ("Esc", "", COLOR_DIM),
        ],
        48,
        9,
    );
}

fn render_delete_popup(f: &mut Frame, anime_title: &str) {
    let body = vec![
        Line::from(Span::styled(
            "Видалити весь прогрес для",
            Style::default().fg(COLOR_DIM),
        )),
        Line::from(Span::styled(
            truncate_middle(anime_title, 42),
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Цю дію не можна скасувати.",
            Style::default().fg(COLOR_ERROR),
        )),
    ];
    render_confirm_dialog(
        f,
        " Підтвердження ",
        COLOR_ERROR,
        &body,
        &[("Enter", "Видалити", COLOR_ERROR), ("Esc", "", COLOR_DIM)],
        46,
        9,
    );
}

fn render_clear_library_popup(f: &mut Frame) {
    let body = vec![
        Line::from(Span::styled(
            "Очистити всю бібліотеку?",
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Статуси, переглянуті серії та таймкоди буде видалено.",
            Style::default().fg(COLOR_ERROR),
        )),
    ];
    render_confirm_dialog(
        f,
        " Очистити бібліотеку ",
        COLOR_ERROR,
        &body,
        &[("Enter", "Очистити", COLOR_ERROR), ("Esc", "", COLOR_DIM)],
        58,
        8,
    );
}

fn render_error_popup(f: &mut Frame, message: &str) {
    let chunks = wrap_text(message, 46);
    let mut body = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        body.push(Line::from(Span::styled(
            chunk,
            Style::default().fg(COLOR_TEXT),
        )));
    }
    let height = (body.len() as u16).saturating_add(4).clamp(6, 12);
    render_confirm_dialog(
        f,
        " Помилка ",
        COLOR_ERROR,
        &body,
        &[("Esc", "", COLOR_DIM)],
        50,
        height,
    );
}

fn render_status_editor_popup(f: &mut Frame, app: &AppState) {
    let Some(editor) = app.status_editor.as_ref() else {
        return;
    };
    let actions = [
        ("↑/↓", "Вибір", COLOR_SECONDARY),
        ("Enter", "OK", COLOR_HIGHLIGHT),
        ("Esc", "", COLOR_DIM),
    ];
    let area = centered_fixed(f.area(), dialog_width_for(40, &actions), 13);
    let block = dialog_block(" Статус аніме ", COLOR_PRIMARY, COLOR_SECONDARY);
    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(truncate_middle(&editor.title, 42))
            .alignment(Alignment::Center)
            .style(Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD)),
        rows[0],
    );
    // rows[1] = breathing room under the title

    // Fixed-width labels so radio circles share one vertical column when centered.
    let label_w = AnimeStatus::ALL
        .iter()
        .map(|status| status.label().chars().count())
        .max()
        .unwrap_or(0);
    let lines = AnimeStatus::ALL
        .iter()
        .enumerate()
        .map(|(index, status)| {
            let selected = index == editor.selected;
            let radio = if selected { "●" } else { "○" };
            let label = pad_display(status.label(), label_w);
            let style = if selected {
                Style::default()
                    .fg(COLOR_TEXT)
                    .bg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_DIM)
            };
            // Leading space + radio so the marker isn't glued to the left edge.
            Line::from(Span::styled(format!(" {radio}  {label}"), style))
        })
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), rows[2]);
    // rows[3] = breathing room above the footer

    f.render_widget(
        Paragraph::new(action_footer_line(&actions)).alignment(Alignment::Center),
        rows[4],
    );
}

fn render_help_popup(f: &mut Frame) {
    let area = centered_fixed(f.area(), 68, 16);
    let block = dialog_block(" Довідка ", COLOR_PRIMARY, COLOR_PRIMARY);
    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(body[0]);

    let section = |label: &'static str| {
        Line::from(Span::styled(
            format!(" {label} "),
            Style::default()
                .bg(COLOR_SECONDARY)
                .fg(COLOR_BG_DARK)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let row = |key: &'static str, desc: &'static str| {
        Line::from(vec![
            Span::styled(format!(" {key:<10}"), Style::default().fg(COLOR_SECONDARY)),
            Span::styled(desc, Style::default().fg(COLOR_TEXT)),
        ])
    };

    let left_col = vec![
        section("Глобальні"),
        row("1/2/3", "Вкладки"),
        row("Alt+1/2/3", "Вкладки в пошуку"),
        row("/", "Пошук у вкладці"),
        row("? / h", "Довідка"),
        row("q", "Вийти"),
        row("Ctrl+C", "Вийти будь-де"),
        Line::from(""),
        section("Навігація"),
        row("↑↓ j k", "Список"),
        row("PgUp/Dn", "Сторінка"),
        row("→ Enter", "Вперед"),
        row("← Esc", "Назад"),
    ];
    let right_col = vec![
        section("Дії"),
        row("Enter", "Відтворити (mpv)"),
        row("c", "Продовжити"),
        row("e", "Статус аніме"),
        row("Space", "Переглянуто"),
        row("Backsp.", "Очистити таймкод"),
        row("d", "Видалити прогрес"),
        row("o", "У браузері"),
        row("Tab", "Категорія"),
    ];

    f.render_widget(Paragraph::new(left_col), columns[0]);
    f.render_widget(Paragraph::new(right_col), columns[1]);
    f.render_widget(
        Paragraph::new(action_footer_line(&[("Esc", "", COLOR_DIM)])).alignment(Alignment::Center),
        body[1],
    );
}

/// Shared confirm/info dialog with centered body and key-action footer.
fn render_confirm_dialog(
    f: &mut Frame,
    title: &str,
    accent: Color,
    body: &[Line<'static>],
    actions: &[(&str, &str, Color)],
    width: u16,
    height: u16,
) {
    let area = centered_fixed(f.area(), dialog_width_for(width, actions), height);
    let block = dialog_block(title, accent, accent);
    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    f.render_widget(
        Paragraph::new(body.to_vec())
            .alignment(Alignment::Center)
            .wrap(ratatui::widgets::Wrap { trim: true }),
        rows[0],
    );
    f.render_widget(
        Paragraph::new(action_footer_line(actions)).alignment(Alignment::Center),
        rows[1],
    );
}

fn dialog_block(title: &str, border: Color, title_color: Color) -> Block<'_> {
    Block::default()
        .title(Span::styled(
            title.to_string(),
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        // Transparent fill so the terminal/app behind shows through (legacy look).
        .style(Style::default().bg(COLOR_BG_TRANSPARENT))
}

fn action_footer_line(actions: &[(&str, &str, Color)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (key, label, color)) in actions.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(COLOR_DIM)));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default().fg(*color).add_modifier(Modifier::BOLD),
        ));
        if !label.is_empty() {
            spans.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(COLOR_DIM),
            ));
        }
    }
    Line::from(spans)
}

/// Visible character width of a footer (must fit inside dialog inner width).
fn action_footer_width(actions: &[(&str, &str, Color)]) -> u16 {
    let mut width = 0usize;
    for (i, (key, label, _)) in actions.iter().enumerate() {
        if i > 0 {
            width += 3; // " · "
        }
        width += key.chars().count();
        if !label.is_empty() {
            width += 1 + label.chars().count();
        }
    }
    width as u16
}

/// Dialog outer width that fits borders + footer with a little padding.
fn dialog_width_for(min_width: u16, actions: &[(&str, &str, Color)]) -> u16 {
    // borders (2) + 2 cells slack so centered text never clips on the last glyph
    min_width.max(action_footer_width(actions).saturating_add(6))
}

/// Pixel-accurate centered rect with fixed width/height, clamped to the frame.
fn centered_fixed(frame: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(frame.width.saturating_sub(2)).max(20);
    let height = height.min(frame.height.saturating_sub(2)).max(5);
    let x = frame.x + frame.width.saturating_sub(width) / 2;
    let y = frame.y + frame.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

fn truncate_middle(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars || max_chars < 5 {
        return text.chars().take(max_chars).collect();
    }
    let keep = max_chars.saturating_sub(1) / 2;
    let left: String = text.chars().take(keep).collect();
    let right: String = text
        .chars()
        .rev()
        .take(max_chars.saturating_sub(keep + 1))
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{left}…{right}")
}

/// Right-pad a display string to `width` Unicode scalar values.
fn pad_display(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count >= width {
        text.chars().take(width).collect()
    } else {
        format!("{text}{}", " ".repeat(width - count))
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
            continue;
        }
        if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
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
    let unavailable = release.availability == api::ReleaseAvailability::Unavailable;
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
        metadata.push("⚠ недоступно".to_string());
    }
    let label = label_with_metadata(&release_label(catalog, release), &metadata);
    let item = ListItem::new(truncate_with_ellipsis(&label, width));
    if unavailable {
        item.style(
            Style::default()
                .fg(COLOR_ERROR)
                .add_modifier(Modifier::BOLD),
        )
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
                "ДОДАТКОВО"
            } else {
                "ОСНОВНА ІСТОРІЯ"
            };
            items.push(
                ListItem::new(Line::from(Span::styled(
                    release_section_line(label, width),
                    Style::default()
                        .fg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                )))
                .style(Style::default().bg(COLOR_BG_TRANSPARENT)),
            );
            previous_extra = Some(is_extra);
        }
        release_rows.push(items.len());
        items.push(release_list_item(catalog, release, width));
    }

    (items, release_rows)
}

fn release_section_line(label: &str, width: usize) -> String {
    let label = format!(" {label} ");
    let label_width = label.chars().count();
    if width <= label_width {
        return label.chars().take(width).collect();
    }

    let fill_width = width - label_width;
    let left_width = fill_width / 2;
    let right_width = fill_width - left_width;
    format!(
        "{}{}{}",
        "─".repeat(left_width),
        label,
        "─".repeat(right_width)
    )
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

    #[test]
    fn release_section_line_centers_label_and_fills_available_width() {
        assert_eq!(release_section_line("TEST", 12), "─── TEST ───");
        assert_eq!(release_section_line("TEST", 13), "─── TEST ────");
        assert_eq!(release_section_line("TEST", 4), " TES");
    }

    #[test]
    fn compact_labels_use_brackets_and_ukrainian_season_pluralization() {
        assert_eq!(season_count_label(1), "1 сезон");
        assert_eq!(season_count_label(2), "2 сезони");
        assert_eq!(season_count_label(5), "5 сезонів");
        assert_eq!(season_count_label(12), "12 сезонів");
        assert_eq!(
            label_with_metadata(
                "Клас убивць",
                &[season_count_label(2), "22 сер.".to_string()]
            ),
            "Клас убивць [2 сезони · 22 сер.]"
        );
    }

    #[test]
    fn sidebar_polish_truncates_titles_and_limits_genres() {
        assert_eq!(truncate_with_ellipsis("Клас убивць", 8), "Клас уб…");
        assert_eq!(truncate_with_ellipsis("Клас", 8), "Клас");
        assert_eq!(
            summarized_genres(&[
                "A".to_string(),
                "B".to_string(),
                "C".to_string(),
                "D".to_string(),
                "E".to_string(),
            ]),
            "A · B · C · D · +1"
        );
    }
}
