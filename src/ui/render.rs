use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph},
};
use ratatui_image::{StatefulImage, protocol::StatefulProtocol};

use crate::api;
use crate::settings::{ColorMode, SurfaceMode, ThemePreset};
use crate::storage::{AnimeStatus, LibraryReleaseKind};
use crate::ui::app::{
    AppMode, AppState, FocusPanel, LibraryFilter, LibrarySort, PrimaryTab, SearchSort,
    SettingsChoiceKind, SettingsInput, SettingsTab, StatusKind, THRESHOLD_BAR_WIDTH, UpdateState,
    canonical_studio_name,
};

mod library;
mod search;
mod settings;
#[cfg(test)]
use settings::{selected_theme_preview, theme_settings_display_row};

#[derive(Clone, Copy)]
struct ThemePalette {
    primary: Color,
    secondary: Color,
    highlight: Color,
    success: Color,
    warning: Color,
    error: Color,
    dim: Color,
    text: Color,
    on_primary: Color,
    dark: Color,
    light_text: Color,
    light_dim: Color,
    light: Color,
}

const ANIHUB_PALETTE: ThemePalette = ThemePalette {
    primary: Color::Rgb(147, 51, 234),
    secondary: Color::Rgb(168, 85, 247),
    highlight: Color::Rgb(59, 130, 246),
    success: Color::Rgb(34, 197, 94),
    warning: Color::Rgb(234, 179, 8),
    error: Color::Rgb(239, 68, 68),
    dim: Color::Rgb(107, 114, 128),
    text: Color::Rgb(243, 244, 246),
    on_primary: Color::Rgb(243, 244, 246),
    dark: Color::Rgb(17, 24, 39),
    light_text: Color::Rgb(31, 41, 55),
    light_dim: Color::Rgb(107, 114, 128),
    light: Color::Rgb(249, 250, 251),
};

const fn ansi16_palette(theme: ThemePreset) -> ThemePalette {
    let (primary, secondary, highlight, on_primary) = match theme {
        ThemePreset::CatppuccinMocha => (
            Color::LightMagenta,
            Color::Magenta,
            Color::LightBlue,
            Color::Black,
        ),
        ThemePreset::TokyoNight => (Color::Blue, Color::LightBlue, Color::Cyan, Color::White),
        ThemePreset::KanagawaWave => (Color::LightBlue, Color::Yellow, Color::Cyan, Color::Black),
        ThemePreset::RosePine => (
            Color::LightMagenta,
            Color::LightRed,
            Color::Magenta,
            Color::Black,
        ),
        ThemePreset::GruvboxDark => (
            Color::Yellow,
            Color::LightYellow,
            Color::Green,
            Color::Black,
        ),
        ThemePreset::EverforestDark => (Color::Green, Color::LightGreen, Color::Cyan, Color::Black),
        ThemePreset::AyuDark => (
            Color::Yellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::Black,
        ),
    };
    ThemePalette {
        primary,
        secondary,
        highlight,
        success: Color::LightGreen,
        warning: Color::LightYellow,
        error: Color::LightRed,
        dim: Color::DarkGray,
        text: Color::White,
        on_primary,
        // Kitty makes cells matching its default background translucent. A
        // fixed near-black 256-color surface remains opaque while accents stay
        // strictly terminal-native ANSI 16 colors.
        dark: Color::Indexed(234),
        light_text: Color::Black,
        light_dim: Color::DarkGray,
        light: Color::Indexed(255),
    }
}

const fn ansi256_palette(theme: ThemePreset) -> ThemePalette {
    match theme {
        ThemePreset::CatppuccinMocha => ThemePalette {
            primary: Color::Indexed(183),
            secondary: Color::Indexed(147),
            highlight: Color::Indexed(111),
            success: Color::Indexed(151),
            warning: Color::Indexed(223),
            error: Color::Indexed(211),
            dim: Color::Indexed(245),
            text: Color::Indexed(189),
            on_primary: Color::Indexed(235),
            dark: Color::Indexed(235),
            light_text: Color::Indexed(240),
            light_dim: Color::Indexed(242),
            light: Color::Indexed(255),
        },
        ThemePreset::TokyoNight => ThemePalette {
            primary: Color::Indexed(141),
            secondary: Color::Indexed(111),
            highlight: Color::Indexed(117),
            success: Color::Indexed(149),
            warning: Color::Indexed(179),
            error: Color::Indexed(210),
            dim: Color::Indexed(245),
            text: Color::Indexed(153),
            on_primary: Color::Indexed(234),
            dark: Color::Indexed(234),
            light_text: Color::Indexed(61),
            light_dim: Color::Indexed(103),
            light: Color::Indexed(254),
        },
        ThemePreset::KanagawaWave => ThemePalette {
            primary: Color::Indexed(103),
            secondary: Color::Indexed(180),
            highlight: Color::Indexed(110),
            success: Color::Indexed(107),
            warning: Color::Indexed(180),
            error: Color::Indexed(203),
            dim: Color::Indexed(244),
            text: Color::Indexed(187),
            on_primary: Color::Indexed(231),
            dark: Color::Indexed(235),
            light_text: Color::Indexed(240),
            light_dim: Color::Indexed(102),
            light: Color::Indexed(229),
        },
        ThemePreset::RosePine => ThemePalette {
            primary: Color::Indexed(182),
            secondary: Color::Indexed(181),
            highlight: Color::Indexed(152),
            success: Color::Indexed(108),
            warning: Color::Indexed(216),
            error: Color::Indexed(168),
            dim: Color::Indexed(245),
            text: Color::Indexed(189),
            on_primary: Color::Indexed(234),
            dark: Color::Indexed(234),
            light_text: Color::Indexed(60),
            light_dim: Color::Indexed(103),
            light: Color::Indexed(255),
        },
        ThemePreset::GruvboxDark => ThemePalette {
            primary: Color::Indexed(214),
            secondary: Color::Indexed(174),
            highlight: Color::Indexed(108),
            success: Color::Indexed(142),
            warning: Color::Indexed(214),
            error: Color::Indexed(203),
            dim: Color::Indexed(243),
            text: Color::Indexed(187),
            on_primary: Color::Indexed(235),
            dark: Color::Indexed(235),
            light_text: Color::Indexed(237),
            light_dim: Color::Indexed(241),
            light: Color::Indexed(230),
        },
        ThemePreset::EverforestDark => ThemePalette {
            primary: Color::Indexed(108),
            secondary: Color::Indexed(175),
            highlight: Color::Indexed(109),
            success: Color::Indexed(144),
            warning: Color::Indexed(180),
            error: Color::Indexed(174),
            dim: Color::Indexed(243),
            text: Color::Indexed(187),
            on_primary: Color::Indexed(236),
            dark: Color::Indexed(236),
            light_text: Color::Indexed(242),
            light_dim: Color::Indexed(244),
            light: Color::Indexed(230),
        },
        ThemePreset::AyuDark => ThemePalette {
            primary: Color::Indexed(179),
            secondary: Color::Indexed(75),
            highlight: Color::Indexed(183),
            success: Color::Indexed(149),
            warning: Color::Indexed(209),
            error: Color::Indexed(204),
            dim: Color::Indexed(244),
            text: Color::Indexed(250),
            on_primary: Color::Indexed(233),
            dark: Color::Indexed(233),
            light_text: Color::Indexed(241),
            light_dim: Color::Indexed(244),
            light: Color::Indexed(231),
        },
    }
}

const fn palette_for_mode(mode: ColorMode, theme: ThemePreset) -> ThemePalette {
    match mode {
        ColorMode::AniHubRgb => ANIHUB_PALETTE,
        ColorMode::Ansi16 => ansi16_palette(theme),
        ColorMode::Ansi256 => ansi256_palette(theme),
    }
}

fn colorfgbg_prefers_light(value: &str) -> Option<bool> {
    let background = value.rsplit(';').next()?.trim().parse::<u8>().ok()?;
    Some(matches!(background, 7 | 15))
}

fn surface_prefers_light(mode: SurfaceMode) -> bool {
    match mode {
        SurfaceMode::Dark => false,
        SurfaceMode::Light => true,
        SurfaceMode::Auto => std::env::var("COLORFGBG")
            .ok()
            .as_deref()
            .and_then(colorfgbg_prefers_light)
            .unwrap_or(false),
    }
}

fn surface_text(palette: ThemePalette, mode: SurfaceMode, light_surface: bool) -> Color {
    if mode == SurfaceMode::Auto {
        Color::Reset
    } else if light_surface {
        palette.light_text
    } else {
        palette.text
    }
}

fn surface_background(palette: ThemePalette, transparent: bool, light_surface: bool) -> Color {
    if transparent {
        Color::Reset
    } else if light_surface {
        palette.light
    } else {
        palette.dark
    }
}

thread_local! {
    static ACTIVE_COLOR_MODE: std::cell::Cell<ColorMode> = const {
        std::cell::Cell::new(ColorMode::AniHubRgb)
    };
    static ACTIVE_THEME: std::cell::Cell<ThemePreset> = const {
        std::cell::Cell::new(ThemePreset::CatppuccinMocha)
    };
    static ACTIVE_SURFACE_MODE: std::cell::Cell<SurfaceMode> = const {
        std::cell::Cell::new(SurfaceMode::Auto)
    };
    static ACTIVE_LIGHT_SURFACE: std::cell::Cell<bool> = const {
        std::cell::Cell::new(false)
    };
    static TRANSPARENT_BACKGROUND: std::cell::Cell<bool> = const {
        std::cell::Cell::new(true)
    };
}

fn set_active_theme(
    mode: ColorMode,
    theme: ThemePreset,
    surface_mode: SurfaceMode,
    transparent_background: bool,
) {
    ACTIVE_COLOR_MODE.set(mode);
    ACTIVE_THEME.set(theme);
    ACTIVE_SURFACE_MODE.set(surface_mode);
    ACTIVE_LIGHT_SURFACE.set(surface_prefers_light(surface_mode));
    TRANSPARENT_BACKGROUND.set(transparent_background);
}

fn active_palette() -> ThemePalette {
    palette_for_mode(ACTIVE_COLOR_MODE.get(), ACTIVE_THEME.get())
}

fn color_primary() -> Color {
    active_palette().primary
}
fn color_secondary() -> Color {
    active_palette().secondary
}
fn color_highlight() -> Color {
    active_palette().highlight
}
fn color_success() -> Color {
    active_palette().success
}
fn color_warning() -> Color {
    active_palette().warning
}
fn color_error() -> Color {
    active_palette().error
}
fn color_dim() -> Color {
    let palette = active_palette();
    if ACTIVE_LIGHT_SURFACE.get() {
        palette.light_dim
    } else {
        palette.dim
    }
}
fn color_text() -> Color {
    surface_text(
        active_palette(),
        ACTIVE_SURFACE_MODE.get(),
        ACTIVE_LIGHT_SURFACE.get(),
    )
}
fn color_background() -> Color {
    surface_background(
        active_palette(),
        TRANSPARENT_BACKGROUND.get(),
        ACTIVE_LIGHT_SURFACE.get(),
    )
}

fn selection_style_for(mode: ColorMode, palette: ThemePalette) -> Style {
    match mode {
        ColorMode::AniHubRgb => Style::default()
            .fg(palette.on_primary)
            .bg(palette.primary)
            .add_modifier(Modifier::BOLD),
        ColorMode::Ansi16 | ColorMode::Ansi256 => Style::default()
            .fg(palette.on_primary)
            .bg(palette.primary)
            .add_modifier(Modifier::BOLD),
    }
}

fn selection_style() -> Style {
    selection_style_for(ACTIVE_COLOR_MODE.get(), active_palette())
}

pub fn render(f: &mut Frame, app: &mut AppState) {
    set_active_theme(
        app.settings.color_mode(),
        app.settings.theme,
        app.settings.surface_mode,
        app.settings.transparent_background,
    );
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

    // Paint either the selected theme background or the terminal's own
    // background when transparency is enabled.
    f.render_widget(
        Block::default().style(Style::default().bg(color_background())),
        size,
    );

    render_header(f, app, main_chunks[0]);

    if app.mode == AppMode::Settings {
        settings::render(f, app, main_chunks[1]);
    } else if size.width >= 110 {
        let body_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
            .split(main_chunks[1]);
        if app.is_library_mode() {
            library::render_sidebar(f, app, body_chunks[0]);
            library::render_lists(f, app, body_chunks[1]);
        } else {
            search::render_sidebar(f, app, body_chunks[0]);
            search::render_lists(f, app, body_chunks[1]);
        }
    } else {
        if app.is_library_mode() {
            library::render_lists(f, app, main_chunks[1]);
        } else {
            search::render_lists(f, app, main_chunks[1]);
        }
    }
    render_status_bar(f, app, main_chunks[2]);

    if let Some((message, StatusKind::Error)) = app.status_message.clone() {
        render_error_popup(f, &message, app.status_retry_available);
    } else if let Some((title, _)) = app.moonanime_browser_prompt.clone() {
        render_moonanime_popup(f, &title);
    } else if app.status_editor.is_some() {
        render_status_editor_popup(f, app);
    } else if app.search.ordering.popup.is_some() {
        search::render_sort_popup(f, app);
    } else if app.library.sort_popup.is_some() {
        library::render_sort_popup(f, app);
    } else if app.library.pending_watched_confirmation.is_some() {
        library::render_watched_confirmation(f, app);
    } else if app.library.clear_confirmation {
        render_clear_library_popup(f);
    } else if app.settings_ui.update_popup {
        settings::render_update_popup(f, app);
    } else if app.settings_ui.input.is_some() {
        settings::render_text_popup(f, app);
    } else if app.settings_ui.threshold.is_some() {
        settings::render_threshold_popup(f, app);
    } else if app.settings_ui.choice.is_some() {
        settings::render_choice_popup(f, app);
    } else if let Some((_, anime_title)) = app.library.pending_delete_confirmation.clone() {
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
            top.push(Span::styled(" | ", Style::default().fg(color_dim())));
        }
        let active = *tab == app.primary_tab();
        top.push(Span::styled(
            format!(" {} · {} ", index + 1, tab.label()),
            if active {
                selection_style()
            } else {
                Style::default().fg(color_dim())
            },
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(top))
            .alignment(Alignment::Center)
            .style(Style::default().bg(color_background())),
        rows[0],
    );

    let editing = app.mode == AppMode::SearchInput || app.library.search_editing;
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
            if app.library.search_editing || !app.library.search_query.is_empty() =>
        {
            (
                " Пошук у бібліотеці · / ",
                library_search_header_context(app),
                if app.library.search_editing {
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

    let context_border = if editing {
        color_highlight()
    } else {
        color_dim()
    };

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
                        .style(Style::default().bg(color_background())),
                )
                .alignment(alignment)
                .style(Style::default().bg(color_background()).fg(color_text())),
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
                .style(Style::default().bg(color_background())),
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
    if app.library.search_editing {
        app.library.search_cursor
    } else {
        app.search.cursor
    }
}

fn settings_tabs_context(app: &AppState) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, tab) in SettingsTab::ALL.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  |  ", Style::default().fg(color_dim())));
        }
        let active = tab == app.settings_ui.tab;
        spans.push(Span::styled(
            format!(" {} ", tab.label()),
            if active {
                selection_style()
            } else {
                Style::default().fg(color_dim())
            },
        ));
    }
    Line::from(spans)
}

fn search_header_context(app: &AppState) -> Line<'static> {
    let query = if app.mode == AppMode::SearchInput {
        app.search.query.as_str()
    } else {
        app.search.last_query.as_str()
    };
    if query.is_empty() {
        Line::from(Span::styled(
            if app.mode == AppMode::SearchInput {
                "введіть назву аніме…"
            } else {
                "Введіть назву аніме…"
            },
            Style::default().fg(color_dim()),
        ))
    } else {
        Line::from(Span::styled(
            query.to_string(),
            Style::default()
                .fg(color_text())
                .add_modifier(if app.mode == AppMode::SearchInput {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ))
    }
}

fn library_search_header_context(app: &AppState) -> Line<'static> {
    if app.library.search_query.is_empty() {
        Line::from(Span::styled(
            "введіть назву аніме у бібліотеці…",
            Style::default().fg(color_dim()),
        ))
    } else {
        Line::from(Span::styled(
            app.library.search_query.clone(),
            Style::default()
                .fg(color_text())
                .add_modifier(if app.library.search_editing {
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
            let active = *filter == app.library.filter;
            let label = filter.label();
            let style = if active {
                selection_style()
            } else {
                Style::default().fg(color_dim())
            };
            [
                Span::styled(format!("  {label}  "), style),
                Span::styled("  ", Style::default().fg(color_dim())),
            ]
        })
        .collect::<Vec<_>>();
    Line::from(spans)
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
            } else if let Some(now) = &app.playback.now_playing {
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
                    .fg(color_secondary())
                    .bg(color_background())
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
                    Style::default()
                        .fg(color_text())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "hub",
                    Style::default()
                        .fg(color_primary())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "-cli",
                    Style::default()
                        .fg(color_text())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" | v{} ", env!("CARGO_PKG_VERSION")),
                    Style::default().fg(color_dim()),
                ),
            ]))
            .style(Style::default().bg(color_background()))
            .alignment(Alignment::Left),
            columns[0],
        );
        // Centered framed keybinds: │ Enter Далі  ·  e Статус │
        f.render_widget(
            Paragraph::new(framed_shortcuts_line(&context_shortcuts(app)))
                .style(Style::default().bg(color_background()))
                .alignment(Alignment::Center),
            columns[1],
        );
        // Symmetric empty side keeps the bind strip visually centered.
        f.render_widget(
            Paragraph::new("").style(Style::default().bg(color_background())),
            columns[2],
        );
    }
}

/// Build `│  key Action  ·  key Action  │` with purple rails.
fn framed_shortcuts_line(shortcuts: &str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> =
        vec![Span::styled("│ ", Style::default().fg(color_primary()))];
    let parts: Vec<&str> = shortcuts
        .split("  ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(color_dim())));
        }
        // First token is the key chord, rest is the label.
        let mut tokens = part.splitn(2, ' ');
        let key = tokens.next().unwrap_or("");
        let label = tokens.next().unwrap_or("");
        spans.push(Span::styled(
            key.to_string(),
            Style::default()
                .fg(color_secondary())
                .add_modifier(Modifier::BOLD),
        ));
        if !label.is_empty() {
            spans.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(color_dim()),
            ));
        }
    }
    spans.push(Span::styled(" │", Style::default().fg(color_primary())));
    Line::from(spans)
}

fn context_shortcuts(app: &AppState) -> String {
    if app.mode == AppMode::Settings {
        if app.settings_ui.update_popup {
            return "Enter Дія  Esc Закрити  Ctrl+C Вихід".to_string();
        }
        if app.settings_ui.input.is_some() {
            return "Enter Зберегти  Esc Скасувати  Ctrl+C Вихід".to_string();
        }
        return "↑↓ Вибір  Space Змінити  Enter Дія  Tab Вкладка  Esc Назад".to_string();
    }
    if app.mode == AppMode::SearchInput {
        return "Enter Знайти  Esc Скасувати  Alt+2 Бібліотека  Ctrl+C Вихід".to_string();
    }
    if app.library.search_editing {
        return "Enter Застосувати  Esc Очистити  Tab Категорія  Ctrl+C Вихід".to_string();
    }
    if app.is_library_mode() {
        return match app.mode {
            AppMode::Library => {
                "Enter Відкрити  Space Усе переглянуто  s Сортування  e Статус  / Пошук".to_string()
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
            "Enter Далі  s Сортування  e Статус  c Продовжити  / Пошук".to_string()
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
            Style::default().fg(color_dim()),
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

fn dubbing_count_label(count: usize) -> String {
    let suffix = if count % 10 == 1 && count % 100 != 11 {
        "озвучка"
    } else if (2..=4).contains(&(count % 10)) && !(12..=14).contains(&(count % 100)) {
        "озвучки"
    } else {
        "озвучок"
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
        vec![(anime_type.to_uppercase(), Style::default().fg(color_text()))];
    if let Some(year) = year {
        values.push((year.to_string(), Style::default().fg(color_text())));
    }
    if let Some(rating) = rating {
        values.push((
            format!("★ {rating:.1}"),
            Style::default()
                .fg(color_warning())
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(episodes) = episodes {
        values.push((
            format!("{episodes} сер."),
            Style::default().fg(color_text()),
        ));
    }

    let mut spans = Vec::new();
    for (index, (value, style)) in values.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(color_dim())));
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

#[derive(Clone, Copy)]
struct TrackingPosition {
    season: u32,
    episode: u32,
    timestamp: f64,
}

impl From<&crate::storage::WatchProgress> for TrackingPosition {
    fn from(progress: &crate::storage::WatchProgress) -> Self {
        Self {
            season: progress.season,
            episode: progress.episode,
            timestamp: progress.timestamp,
        }
    }
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
    let watched = watched_episode_count(app, anime_ids) as u32;
    tracking_summary_lines(
        status,
        watched,
        total_episodes,
        latest_progress_for_ids(app, anime_ids).map(TrackingPosition::from),
    )
}

fn tracking_summary_lines(
    status: AnimeStatus,
    watched: u32,
    total_episodes: Option<u32>,
    latest_progress: Option<TrackingPosition>,
) -> Vec<Line<'static>> {
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
                    .fg(color_secondary())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(color_dim())),
            Span::styled(watched_label, Style::default().fg(color_text())),
        ])
        .alignment(Alignment::Center),
    ];
    if let Some(progress) = latest_progress {
        lines.push(
            Line::from(Span::styled(
                format!(
                    "⏱ S{}E{} · {}",
                    progress.season,
                    progress.episode,
                    format_timestamp(progress.timestamp)
                ),
                Style::default().fg(color_highlight()),
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
        .content
        .current_sources
        .as_ref()
        .and_then(|sources| {
            sources
                .ashdi
                .iter()
                .position(|studio| studio.season_number == season_num)
        })
        .and_then(|idx| app.content.studio_anime_ids.get(idx))
        .copied()?;
    season_is_complete(app, anime_id, season_num).then_some("✓")
}

fn selected_search_anime_id(app: &AppState) -> Option<u32> {
    let season_num = app.selected_season_num()?;
    let dub_idx = app.content.selected_dubbing_index?;
    app.content.current_sources.as_ref().and_then(|sources| {
        sources
            .ashdi
            .iter()
            .enumerate()
            .filter(|(_, studio)| studio.season_number == season_num)
            .nth(dub_idx)
            .and_then(|(idx, _)| app.content.studio_anime_ids.get(idx))
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
    let sources = app.content.current_sources.as_ref()?;
    let studio_idx = sources
        .ashdi
        .iter()
        .position(|s| s.season_number == season_num)?;
    let anime_id = app.content.studio_anime_ids.get(studio_idx).copied()?;
    app.details_cache
        .get(&anime_id)
        .and_then(|d| d.year)
        .or_else(|| {
            app.search
                .results
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
        Style::default().fg(color_highlight())
    } else {
        Style::default().fg(color_dim())
    };
    let title_style = if is_focused {
        Style::default()
            .fg(color_secondary())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color_dim())
    };

    List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(title, title_style))
                .border_style(border_style)
                .title_alignment(Alignment::Center),
        )
        .highlight_style(selection_style())
        .highlight_symbol(">> ")
}

fn render_list_message(
    f: &mut Frame,
    area: Rect,
    title: &str,
    is_focused: bool,
    message: &str,
    loading: bool,
) {
    let border_style = if is_focused {
        Style::default().fg(color_highlight())
    } else {
        Style::default().fg(color_dim())
    };
    let title_style = if is_focused {
        Style::default()
            .fg(color_secondary())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color_dim())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(title.to_string(), title_style))
        .title_alignment(Alignment::Center)
        .border_style(border_style);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .split(inner);
    let label = if loading {
        format!("⟳ {message}")
    } else {
        message.to_string()
    };
    let style = if loading {
        Style::default()
            .fg(color_secondary())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color_dim())
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            truncate_with_ellipsis(&label, inner.width.saturating_sub(2) as usize),
            style,
        ))
        .alignment(Alignment::Center),
        rows[1],
    );
}

fn render_moonanime_popup(f: &mut Frame, episode_title: &str) {
    let body = vec![
        Line::from(Span::styled(
            truncate_middle(episode_title, 44),
            Style::default()
                .fg(color_text())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Епізод відкриється в браузері",
            Style::default().fg(color_dim()),
        )),
        Line::from(Span::styled(
            "(MoonAnime embed)",
            Style::default().fg(color_dim()),
        )),
    ];
    render_confirm_dialog(
        f,
        " MoonAnime ",
        color_highlight(),
        &body,
        &[
            ("Enter", "Відкрити", color_highlight()),
            ("Esc", "", color_dim()),
        ],
        48,
        9,
    );
}

fn render_delete_popup(f: &mut Frame, anime_title: &str) {
    let body = vec![
        Line::from(Span::styled(
            "Видалити весь прогрес для",
            Style::default().fg(color_dim()),
        )),
        Line::from(Span::styled(
            truncate_middle(anime_title, 42),
            Style::default()
                .fg(color_text())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Цю дію не можна скасувати.",
            Style::default().fg(color_error()),
        )),
    ];
    render_confirm_dialog(
        f,
        " Підтвердження ",
        color_error(),
        &body,
        &[
            ("Enter", "Видалити", color_error()),
            ("Esc", "", color_dim()),
        ],
        46,
        9,
    );
}

fn render_clear_library_popup(f: &mut Frame) {
    let body = vec![
        Line::from(Span::styled(
            "Очистити всю бібліотеку?",
            Style::default()
                .fg(color_text())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Статуси, переглянуті серії та таймкоди буде видалено.",
            Style::default().fg(color_error()),
        )),
    ];
    render_confirm_dialog(
        f,
        " Очистити бібліотеку ",
        color_error(),
        &body,
        &[
            ("Enter", "Очистити", color_error()),
            ("Esc", "", color_dim()),
        ],
        58,
        8,
    );
}

fn render_error_popup(f: &mut Frame, message: &str, retry_available: bool) {
    let chunks = wrap_text(message, 46);
    let mut body = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        body.push(Line::from(Span::styled(
            chunk,
            Style::default().fg(color_text()),
        )));
    }
    let height = (body.len() as u16).saturating_add(4).clamp(6, 12);
    let actions = if retry_available {
        vec![
            ("r", "Повторити", color_highlight()),
            ("Esc", "Закрити", color_dim()),
        ]
    } else {
        vec![("Esc", "Закрити", color_dim())]
    };
    render_confirm_dialog(f, " Помилка ", color_error(), &body, &actions, 50, height);
}

fn render_status_editor_popup(f: &mut Frame, app: &AppState) {
    let Some(editor) = app.status_editor.as_ref() else {
        return;
    };
    let actions = [
        ("↑/↓", "Вибір", color_secondary()),
        ("Enter", "OK", color_highlight()),
        ("Esc", "", color_dim()),
    ];
    let area = centered_fixed(f.area(), dialog_width_for(40, &actions), 13);
    let block = dialog_block(" Статус аніме ", color_primary(), color_secondary());
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
            .style(
                Style::default()
                    .fg(color_text())
                    .add_modifier(Modifier::BOLD),
            ),
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
                selection_style()
            } else {
                Style::default().fg(color_dim())
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
    let block = dialog_block(" Довідка ", color_primary(), color_primary());
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
                .bg(color_secondary())
                .fg(active_palette().dark)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let row = |key: &'static str, desc: &'static str| {
        Line::from(vec![
            Span::styled(
                format!(" {key:<10}"),
                Style::default().fg(color_secondary()),
            ),
            Span::styled(desc, Style::default().fg(color_text())),
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
        row("s", "Сортування списку"),
        row("o", "У браузері"),
        row("Tab", "Категорія"),
    ];

    f.render_widget(Paragraph::new(left_col), columns[0]);
    f.render_widget(Paragraph::new(right_col), columns[1]);
    f.render_widget(
        Paragraph::new(action_footer_line(&[("Esc", "", color_dim())]))
            .alignment(Alignment::Center),
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
        // Respect the global transparent/opaque background preference.
        .style(Style::default().bg(color_background()))
}

fn action_footer_line(actions: &[(&str, &str, Color)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (key, label, color)) in actions.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(color_dim())));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default().fg(*color).add_modifier(Modifier::BOLD),
        ));
        if !label.is_empty() {
            spans.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(color_dim()),
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
                .fg(color_error())
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
                        .fg(color_primary())
                        .add_modifier(Modifier::BOLD),
                )))
                .style(Style::default().bg(color_background())),
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
        .search
        .selected_result_index
        .and_then(|i| app.search.results.get(i))
        .map(|a| a.id);
    if rep_id == Some(subject_id) {
        return None;
    }
    app.details_cache.get(&subject_id).or_else(|| {
        app.content
            .current_details
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
        .search
        .selected_result_index
        .and_then(|index| app.search.results.get(index))
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
    use ratatui::{Terminal, backend::TestBackend};

    fn rendered_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn loading_panel_renders_at_supported_terminal_sizes() {
        set_active_theme(
            ColorMode::AniHubRgb,
            ThemePreset::CatppuccinMocha,
            SurfaceMode::Auto,
            true,
        );
        for (width, height) in [(80, 24), (120, 35), (192, 55)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    render_list_message(
                        frame,
                        frame.area(),
                        " Озвучки ",
                        true,
                        "Завантаження озвучок…",
                        true,
                    );
                })
                .unwrap();
            let output = rendered_text(&terminal);
            assert!(output.contains("Озвучки"));
            assert!(output.contains("Завантаження озвучок"));
        }
    }

    #[test]
    fn retryable_error_dialog_exposes_retry_action() {
        set_active_theme(
            ColorMode::AniHubRgb,
            ThemePreset::CatppuccinMocha,
            SurfaceMode::Auto,
            true,
        );
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_error_popup(
                    frame,
                    "Не вдалося виконати пошук\nНемає з’єднання з AniHub",
                    true,
                );
            })
            .unwrap();
        let output = rendered_text(&terminal);
        assert!(output.contains("Повторити"));
        assert!(output.contains("Закрити"));
    }

    #[test]
    fn original_anihub_rgb_theme_remains_the_default_render_palette() {
        set_active_theme(
            ColorMode::AniHubRgb,
            ThemePreset::CatppuccinMocha,
            SurfaceMode::Auto,
            true,
        );
        assert_eq!(color_primary(), Color::Rgb(147, 51, 234));
        assert_eq!(color_secondary(), Color::Rgb(168, 85, 247));
        assert_eq!(color_highlight(), Color::Rgb(59, 130, 246));
        assert_eq!(color_background(), Color::Reset);

        set_active_theme(
            ColorMode::Ansi16,
            ThemePreset::GruvboxDark,
            SurfaceMode::Dark,
            true,
        );
        assert_eq!(color_primary(), Color::Yellow);

        set_active_theme(
            ColorMode::Ansi256,
            ThemePreset::GruvboxDark,
            SurfaceMode::Dark,
            false,
        );
        assert_eq!(color_primary(), Color::Indexed(214));
        assert_eq!(color_background(), Color::Indexed(235));
    }

    #[test]
    fn ansi_themes_use_terminal_or_indexed_colors_instead_of_rgb() {
        for theme in ThemePreset::ALL {
            for mode in [ColorMode::Ansi16, ColorMode::Ansi256] {
                let palette = palette_for_mode(mode, theme);
                for color in [
                    palette.primary,
                    palette.secondary,
                    palette.highlight,
                    palette.success,
                    palette.warning,
                    palette.error,
                    palette.dim,
                    palette.text,
                    palette.on_primary,
                    palette.dark,
                    palette.light_text,
                    palette.light_dim,
                    palette.light,
                ] {
                    assert!(!matches!(color, Color::Rgb(_, _, _)));
                }
            }
        }
    }

    #[test]
    fn theme_preview_follows_the_highlighted_row_before_apply() {
        assert_eq!(selected_theme_preview(0), None);
        assert_eq!(selected_theme_preview(1), None);
        assert_eq!(selected_theme_preview(2), None);
        assert_eq!(
            selected_theme_preview(3),
            Some(ThemePreset::CatppuccinMocha)
        );
        assert_eq!(selected_theme_preview(6), Some(ThemePreset::RosePine));
        assert_eq!(selected_theme_preview(9), Some(ThemePreset::AyuDark));
        assert_eq!(selected_theme_preview(10), None);
        assert_eq!(theme_settings_display_row(2), 2);
        assert_eq!(theme_settings_display_row(3), 4);
    }

    #[test]
    fn theme_hover_preview_does_not_mutate_the_active_palette() {
        set_active_theme(
            ColorMode::Ansi16,
            ThemePreset::TokyoNight,
            SurfaceMode::Dark,
            true,
        );
        let active_primary = color_primary();
        let active_background = color_background();

        assert_eq!(selected_theme_preview(6), Some(ThemePreset::RosePine));
        assert_eq!(color_primary(), active_primary);
        assert_eq!(color_background(), active_background);
    }

    #[test]
    fn ansi16_transparency_keeps_the_terminal_background_and_colored_selection() {
        set_active_theme(
            ColorMode::Ansi16,
            ThemePreset::TokyoNight,
            SurfaceMode::Auto,
            true,
        );
        assert_eq!(color_background(), Color::Reset);
        let selected = selection_style();
        assert_eq!(selected.bg, Some(Color::Blue));
        assert!(!selected.add_modifier.contains(Modifier::REVERSED));

        set_active_theme(
            ColorMode::Ansi16,
            ThemePreset::TokyoNight,
            SurfaceMode::Dark,
            false,
        );
        assert_eq!(color_background(), Color::Indexed(234));
    }

    #[test]
    fn colorfgbg_detection_recognizes_dark_and_light_terminal_backgrounds() {
        assert_eq!(colorfgbg_prefers_light("15;0"), Some(false));
        assert_eq!(colorfgbg_prefers_light("0;15"), Some(true));
        assert_eq!(colorfgbg_prefers_light("0;7"), Some(true));
        assert_eq!(colorfgbg_prefers_light("unknown"), None);
    }

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
            airing_status: None,
            next_airing_episode: None,
            next_airing_at: None,
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
    fn dubbing_count_uses_ukrainian_pluralization() {
        assert_eq!(dubbing_count_label(1), "1 озвучка");
        assert_eq!(dubbing_count_label(2), "2 озвучки");
        assert_eq!(dubbing_count_label(5), "5 озвучок");
        assert_eq!(dubbing_count_label(11), "11 озвучок");
        assert_eq!(dubbing_count_label(22), "22 озвучки");
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
