//! Primary-mode key dispatch.
//!
//! Modal/popup precedence stays in `AppState::handle_events`; once those
//! overlays decline a key, this module owns the tab-specific behavior.

use super::{AppMode, AppState};
use crossterm::event::KeyCode;

pub(super) fn handle_mode_key(app: &mut AppState, shortcut: KeyCode, raw: KeyCode) {
    match app.mode {
        AppMode::Normal => handle_search_results_key(app, shortcut),
        AppMode::SearchInput => handle_search_input_key(app, raw),
        AppMode::Library
        | AppMode::LibrarySeason
        | AppMode::LibraryDubbing
        | AppMode::LibraryEpisode => handle_library_key(app, shortcut),
        AppMode::Settings => app.handle_settings_key(shortcut),
    }
}

fn handle_search_results_key(app: &mut AppState, key: KeyCode) {
    match key {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('c') => app.request_continue(),
        KeyCode::Char(' ') => app.toggle_search_selection_watched(),
        KeyCode::Backspace => app.clear_selected_episode_timestamp(),
        KeyCode::Char('e') => app.open_status_editor(),
        KeyCode::Char('s') => app.open_search_sort_popup(),
        KeyCode::Char('o') => app.open_in_browser(),
        KeyCode::Char('l') => app.open_library(),
        KeyCode::Esc => app.handle_esc(),
        KeyCode::Char('/') => {
            app.mode = AppMode::SearchInput;
            app.search_query.clone_from(&app.last_search_query);
            app.search_cursor = app.search_query.chars().count();
            app.clear_activity();
        }
        KeyCode::Right => app.move_focus_right(),
        KeyCode::Left => app.move_focus_left(),
        KeyCode::Down => app.move_selection_down(),
        KeyCode::Up => app.move_selection_up(),
        KeyCode::Enter => app.handle_enter(),
        _ => {}
    }
}

fn handle_search_input_key(app: &mut AppState, key: KeyCode) {
    match key {
        KeyCode::Enter => {
            app.mode = AppMode::Normal;
            let query = app.search_query.trim().to_string();
            if !query.is_empty() {
                app.last_search_query.clone_from(&query);
                app.search_query = query;
                app.search_cursor = app.search_query.chars().count();
                app.loading = true;
                app.activity_message = Some("Пошук аніме…".to_string());
            }
        }
        KeyCode::Char(character) => app.insert_search_char(character),
        KeyCode::Backspace => app.backspace_search_char(),
        KeyCode::Delete => app.delete_search_char(),
        KeyCode::Left => app.search_cursor = app.search_cursor.saturating_sub(1),
        KeyCode::Right => {
            app.search_cursor = (app.search_cursor + 1).min(app.search_query.chars().count());
        }
        KeyCode::Home => app.search_cursor = 0,
        KeyCode::End => app.search_cursor = app.search_query.chars().count(),
        KeyCode::Esc => {
            // Cancel edit only — keep last results and last query display.
            app.mode = AppMode::Normal;
            app.search_query.clear();
            app.search_cursor = 0;
            app.clear_activity();
            if let Some(index) = app.selected_group_index {
                app.result_list_state.select(Some(index));
            }
        }
        _ => {}
    }
}

fn handle_library_key(app: &mut AppState, key: KeyCode) {
    match (app.mode, key) {
        (_, KeyCode::Char('q')) => app.should_quit = true,
        (AppMode::Library, KeyCode::Char('c')) => app.request_continue(),
        (AppMode::Library, KeyCode::Char('d')) => app.delete_library_selection(),
        (AppMode::Library, KeyCode::Char('s')) => app.open_library_sort_popup(),
        (AppMode::Library, KeyCode::Char(' ')) => app.open_library_watched_confirmation(),
        (
            AppMode::LibrarySeason | AppMode::LibraryDubbing | AppMode::LibraryEpisode,
            KeyCode::Char(' '),
        ) => app.toggle_library_selection_watched(),
        (_, KeyCode::Backspace) if app.mode == AppMode::LibraryEpisode => {
            app.clear_selected_episode_timestamp();
        }
        (_, KeyCode::Char('e')) => app.open_status_editor(),
        (_, KeyCode::Char('o')) => app.open_in_browser(),
        (_, KeyCode::Char('/')) => app.open_library_search(),
        (_, KeyCode::Tab) => app.cycle_library_filter(false),
        (_, KeyCode::BackTab) => app.cycle_library_filter(true),
        (AppMode::Library, KeyCode::Left) => {}
        (AppMode::Library, KeyCode::Esc) => {
            if app.library.search_query.is_empty() {
                app.reset_to_home();
            } else {
                app.library.search_query.clear();
                app.library.search_cursor = 0;
                app.apply_library_filter();
            }
        }
        (_, KeyCode::Left | KeyCode::Esc) => app.leave_library_level(),
        (_, KeyCode::Up) => app.move_library_up(),
        (_, KeyCode::Down) => app.move_library_down(),
        (AppMode::Library, KeyCode::Right | KeyCode::Enter) => app.enter_library_season(),
        (AppMode::LibrarySeason, KeyCode::Right | KeyCode::Enter) => {
            app.enter_library_dubbing();
        }
        (AppMode::LibraryDubbing, KeyCode::Right | KeyCode::Enter) => {
            app.enter_library_episode();
        }
        (AppMode::LibraryEpisode, KeyCode::Enter) => app.activate_selected_episode(),
        _ => {}
    }
}
