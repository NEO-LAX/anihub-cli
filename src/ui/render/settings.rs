//! Settings tab and its modal renderers.

use super::*;

pub(super) fn render(f: &mut Frame, app: &AppState, area: Rect) {
    match app.settings_tab {
        SettingsTab::General => render_general_settings(f, app, area),
        SettingsTab::Themes => render_theme_settings(f, app, area),
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
        .border_style(Style::default().fg(color_highlight()))
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
        settings_item(
            "Discord Rich Presence",
            on_off(app.settings.discord_presence),
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
        .highlight_style(selection_style());
    f.render_stateful_widget(list, area, &mut state);
}

fn render_theme_settings(f: &mut Frame, app: &AppState, area: Rect) {
    let chunks = if area.width >= 86 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(13), Constraint::Min(6)])
            .split(area)
    };

    let mode = app.settings.color_mode();
    let swatch_mode = if mode == ColorMode::AniHubRgb {
        ColorMode::Ansi256
    } else {
        mode
    };
    let on_off = |enabled| {
        if enabled {
            "увімкнено"
        } else {
            "вимкнено"
        }
    };
    let controls_block = Block::default()
        .borders(Borders::ALL)
        .title(" Оформлення ")
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(color_highlight()))
        .padding(Padding::horizontal(2));
    let controls_width = controls_block.inner(chunks[0]).width.saturating_sub(4) as usize;
    let mut items = vec![
        settings_item("Колірний режим", mode.label(), controls_width),
        settings_item(
            "Основа інтерфейсу",
            app.settings.surface_mode.label(),
            controls_width,
        ),
        settings_item(
            "Прозорий фон",
            on_off(app.settings.transparent_background),
            controls_width,
        ),
    ];
    items.push(ListItem::new(Line::from(Span::styled(
        release_section_line("ANSI-КОЛЬОРИ", controls_width),
        Style::default().fg(color_dim()),
    ))));
    items.extend(ThemePreset::ALL.into_iter().map(|theme| {
        let palette = palette_for_mode(swatch_mode, theme);
        let enabled = mode != ColorMode::AniHubRgb;
        let label_color = if enabled { color_text() } else { color_dim() };
        let swatch = |color| if enabled { color } else { color_dim() };
        ListItem::new(Line::from(vec![
            Span::styled(
                if theme == app.settings.theme {
                    "● "
                } else {
                    "  "
                },
                Style::default().fg(swatch(palette.highlight)),
            ),
            Span::styled(
                format!("{:<18}", theme.label()),
                Style::default().fg(label_color),
            ),
            Span::styled("■ ", Style::default().fg(swatch(palette.primary))),
            Span::styled("■ ", Style::default().fg(swatch(palette.secondary))),
            Span::styled("■ ", Style::default().fg(swatch(palette.highlight))),
            Span::styled("■", Style::default().fg(swatch(palette.error))),
        ]))
    }));
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(theme_settings_display_row(app.settings_selected)));
    let disabled_theme_selected = mode == ColorMode::AniHubRgb && app.settings_selected >= 3;
    let highlight_style = if disabled_theme_selected {
        Style::default()
            .fg(color_dim())
            .add_modifier(Modifier::BOLD)
    } else {
        selection_style()
    };
    f.render_stateful_widget(
        List::new(items)
            .block(controls_block)
            .highlight_symbol(">> ")
            .highlight_style(highlight_style),
        chunks[0],
        &mut state,
    );

    let hovered_theme = selected_theme_preview(app.settings_selected);
    let preview_theme = hovered_theme.unwrap_or(app.settings.theme);
    let preview_mode = if hovered_theme.is_some() && mode == ColorMode::AniHubRgb {
        ColorMode::Ansi256
    } else {
        mode
    };
    let preview_palette = palette_for_mode(preview_mode, preview_theme);
    let preview_light_surface = surface_prefers_light(app.settings.surface_mode);
    let preview_text = surface_text(
        preview_palette,
        app.settings.surface_mode,
        preview_light_surface,
    );
    let preview_dim = if preview_light_surface {
        preview_palette.light_dim
    } else {
        preview_palette.dim
    };
    let preview_background = surface_background(
        preview_palette,
        app.settings.transparent_background,
        preview_light_surface,
    );
    let preview_label = format!(
        "{} · {} · {}",
        preview_theme.label(),
        preview_mode.label(),
        app.settings.surface_mode.label()
    );
    let preview = vec![
        Line::from(vec![
            Span::styled(
                " 1 · Пошук ",
                selection_style_for(preview_mode, preview_palette),
            ),
            Span::styled("  |  ", Style::default().fg(preview_dim)),
            Span::styled(
                "2 · Бібліотека",
                Style::default()
                    .fg(preview_palette.secondary)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            "Результати пошуку",
            Style::default()
                .fg(preview_palette.secondary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![Span::styled(
            ">> Каґуя-сама: Кохання як війна ",
            selection_style_for(preview_mode, preview_palette),
        )]),
        Line::from(vec![
            Span::styled("TV · 2022 · ", Style::default().fg(preview_text)),
            Span::styled("★ 8.7", Style::default().fg(preview_palette.warning)),
            Span::styled(" · 12 сер.", Style::default().fg(preview_text)),
        ]),
        Line::from(vec![
            Span::styled(
                "✓ Переглянуто",
                Style::default().fg(preview_palette.success),
            ),
            Span::styled("  ·  ", Style::default().fg(preview_dim)),
            Span::styled("FanVoxUA", Style::default().fg(preview_palette.highlight)),
        ]),
        Line::from(vec![
            Span::styled("Помилка мережі", Style::default().fg(preview_palette.error)),
            Span::styled("  ·  підказка", Style::default().fg(preview_dim)),
        ]),
    ];
    f.render_widget(
        Paragraph::new(preview).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Попередній перегляд · {preview_label} "))
                .title_alignment(Alignment::Center)
                .border_style(Style::default().fg(preview_palette.highlight))
                .style(Style::default().bg(preview_background))
                .padding(Padding::horizontal(2)),
        ),
        chunks[1],
    );
}

pub(super) fn theme_settings_display_row(selected_row: usize) -> usize {
    if selected_row >= 3 {
        selected_row + 1
    } else {
        selected_row
    }
}

pub(super) fn selected_theme_preview(selected_row: usize) -> Option<ThemePreset> {
    selected_row
        .checked_sub(3)
        .and_then(|index| ThemePreset::ALL.get(index).copied())
}

fn render_about_settings(f: &mut Frame, app: &AppState, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(5)])
        .split(area);
    let action_width = chunks[0].width.saturating_sub(8) as usize;
    let actions = vec![
        settings_item("Тека даних", "", action_width),
        settings_item("GitHub", "", action_width),
        settings_item("Перевірити оновлення", "", action_width),
        settings_item("Очистити кеш постерів", "", action_width),
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
                    .border_style(Style::default().fg(color_highlight()))
                    .padding(Padding::horizontal(2)),
            )
            .highlight_symbol(">> ")
            .highlight_style(selection_style()),
        chunks[0],
        &mut state,
    );

    let diagnostics = vec![
        Line::from(vec![
            Span::styled("Версія: ", Style::default().fg(color_dim())),
            Span::styled(env!("CARGO_PKG_VERSION"), Style::default().fg(color_text())),
        ]),
        Line::from(vec![
            Span::styled("GitHub: ", Style::default().fg(color_dim())),
            Span::styled(
                crate::settings::GITHUB_URL,
                Style::default().fg(color_highlight()),
            ),
        ]),
        Line::from(vec![
            Span::styled("History: ", Style::default().fg(color_dim())),
            Span::styled(
                app.settings_store.history_path().display().to_string(),
                Style::default().fg(color_text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("Settings: ", Style::default().fg(color_dim())),
            Span::styled(
                app.settings_store.settings_path().display().to_string(),
                Style::default().fg(color_text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("Cache: ", Style::default().fg(color_dim())),
            Span::styled(
                app.metadata_cache.path().display().to_string(),
                Style::default().fg(color_text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("Posters: ", Style::default().fg(color_dim())),
            Span::styled(
                app.poster_disk_cache.path().display().to_string(),
                Style::default().fg(color_text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("Discord: ", Style::default().fg(color_dim())),
            Span::styled(
                if !app.settings.discord_presence {
                    "вимкнено"
                } else {
                    "увімкнено · AniHub"
                },
                Style::default().fg(if app.settings.discord_presence {
                    color_secondary()
                } else {
                    color_dim()
                }),
            ),
        ]),
        Line::from(vec![
            Span::styled("mpv: ", Style::default().fg(color_dim())),
            Span::styled(
                if app.mpv_available {
                    "знайдено"
                } else {
                    "не знайдено"
                },
                Style::default().fg(if app.mpv_available {
                    color_success()
                } else {
                    color_error()
                }),
            ),
            Span::styled(" · image: ", Style::default().fg(color_dim())),
            Span::styled(
                app.image_protocol.clone(),
                Style::default().fg(color_text()),
            ),
        ]),
    ];
    f.render_widget(
        Paragraph::new(diagnostics)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Про / шляхи ")
                    .title_alignment(Alignment::Center)
                    .border_style(Style::default().fg(color_dim()))
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

pub(super) fn render_choice_popup(f: &mut Frame, app: &AppState) {
    let Some(editor) = app.settings_choice.as_ref() else {
        return;
    };
    let labels = editor.kind.option_labels();
    let rows = labels.len() as u16;
    let height = rows.saturating_add(5).max(8);
    let actions = [
        ("↑/↓", "Вибір", color_secondary()),
        ("Enter", "OK", color_highlight()),
        ("Esc", "", color_dim()),
    ];
    let min_width = match editor.kind {
        SettingsChoiceKind::StartScreen => 36,
        SettingsChoiceKind::LibraryFilter => 40,
    };
    let area = centered_fixed(f.area(), dialog_width_for(min_width, &actions), height);
    let block = dialog_block(editor.kind.title(), color_primary(), color_secondary());
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
                selection_style()
            } else {
                Style::default().fg(color_dim())
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

pub(super) fn render_text_popup(f: &mut Frame, app: &AppState) {
    let Some(kind) = app.settings_input else {
        return;
    };
    let (title, hint) = match kind {
        SettingsInput::MpvPath => (" Шлях до mpv ", "Порожнє значення скинеться на «mpv»"),
        SettingsInput::MpvArgs => (" Аргументи mpv ", "Наприклад: --fs --hwdec=auto"),
    };
    let actions = [
        ("Enter", "Зберегти", color_highlight()),
        ("Esc", "", color_dim()),
    ];
    let area = centered_fixed(f.area(), dialog_width_for(56, &actions), 10);
    let block = dialog_block(title, color_highlight(), color_secondary());
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
        Paragraph::new(Span::styled(hint, Style::default().fg(color_dim())))
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
        .border_style(Style::default().fg(color_highlight()))
        .style(Style::default().bg(color_background()));
    let field_inner = field.inner(layout[1]);
    f.render_widget(field, layout[1]);
    f.render_widget(
        Paragraph::new(Span::styled(
            value,
            Style::default()
                .fg(color_text())
                .add_modifier(Modifier::BOLD),
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

pub(super) fn render_update_popup(f: &mut Frame, app: &AppState) {
    let actions_probe = [
        ("Enter", "Відкрити реліз", color_highlight()),
        ("Esc", "", color_dim()),
    ];
    let area = centered_fixed(f.area(), dialog_width_for(44, &actions_probe), 11);
    let block = dialog_block(" Оновлення ", color_primary(), color_secondary());
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
            Style::default().fg(color_dim()),
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
                        .fg(color_secondary())
                        .add_modifier(Modifier::BOLD),
                )),
            ],
            vec![("Esc", "", color_dim())],
        ),
        UpdateState::Current(version) => (
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "У вас актуальна версія",
                    Style::default()
                        .fg(color_text())
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    version.clone(),
                    Style::default().fg(color_secondary()),
                )),
            ],
            vec![
                ("Enter", "Ще раз", color_highlight()),
                ("Esc", "", color_dim()),
            ],
        ),
        UpdateState::Available(update) => (
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Доступна нова версія",
                    Style::default()
                        .fg(color_text())
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    update.latest_version.clone(),
                    Style::default()
                        .fg(color_secondary())
                        .add_modifier(Modifier::BOLD),
                )),
            ],
            vec![
                ("Enter", "Реліз", color_highlight()),
                ("Esc", "", color_dim()),
            ],
        ),
        UpdateState::Failed(error) => (
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Не вдалося перевірити",
                    Style::default()
                        .fg(color_error())
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    truncate_middle(error, 42),
                    Style::default().fg(color_dim()),
                )),
            ],
            vec![
                ("Enter", "Ще раз", color_highlight()),
                ("Esc", "", color_dim()),
            ],
        ),
    };

    f.render_widget(Paragraph::new(body).alignment(Alignment::Center), layout[1]);
    f.render_widget(
        Paragraph::new(action_footer_line(&actions)).alignment(Alignment::Center),
        layout[3],
    );
}

pub(super) fn render_threshold_popup(f: &mut Frame, app: &AppState) {
    let Some(editor) = app.settings_threshold.as_ref() else {
        return;
    };
    let actions = [("Enter", "OK", color_highlight()), ("Esc", "", color_dim())];
    let area = centered_fixed(f.area(), dialog_width_for(46, &actions), 11);
    let block = dialog_block(
        " Позначати переглянутим ",
        color_primary(),
        color_secondary(),
    );
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
            Style::default().fg(color_dim()),
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
                    color_secondary()
                } else {
                    color_dim()
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
            Style::default().fg(color_dim()),
        ))
        .alignment(Alignment::Center),
        layout[3],
    );

    f.render_widget(
        Paragraph::new(action_footer_line(&actions)).alignment(Alignment::Center),
        layout[5],
    );
}
