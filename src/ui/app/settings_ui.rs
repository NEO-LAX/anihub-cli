//! Settings-tab actions and modal editors.

use super::*;

/// Transient settings-screen state. Persisted preferences remain in
/// `Settings`; this struct owns selection, modal drafts and async UI signals.
pub(crate) struct SettingsUiState {
    pub tab: SettingsTab,
    pub selected: usize,
    pub input: Option<SettingsInput>,
    pub input_value: String,
    pub input_cursor: usize,
    pub choice: Option<SettingsChoiceEditor>,
    pub threshold: Option<SettingsThresholdEditor>,
    pub update_popup: bool,
    pub mpv_available: bool,
    pub image_protocol: String,
    pub update_state: UpdateState,
    pub update_check_requested: bool,
    pub discord_config_changed: bool,
}

impl SettingsUiState {
    pub fn new(mpv_available: bool, image_protocol: String) -> Self {
        Self {
            tab: SettingsTab::General,
            selected: 0,
            input: None,
            input_value: String::new(),
            input_cursor: 0,
            choice: None,
            threshold: None,
            update_popup: false,
            mpv_available,
            image_protocol,
            update_state: UpdateState::Idle,
            update_check_requested: false,
            discord_config_changed: false,
        }
    }

    pub fn close_editors(&mut self) {
        self.input = None;
        self.input_value.clear();
        self.input_cursor = 0;
        self.choice = None;
        self.threshold = None;
        self.update_popup = false;
    }
}

impl AppState {
    fn persist_settings(&mut self) {
        match self.settings_store.save(&self.settings) {
            Ok(()) => self.set_info_status("Налаштування збережено"),
            Err(error) => self.set_error_status(format!("Не вдалося зберегти: {error}")),
        }
    }

    fn toggle_poster_setting(&mut self) {
        self.settings.show_posters = !self.settings.show_posters;
        if self.settings.show_posters {
            self.select_sidebar_subject(self.sidebar_subject());
        } else {
            self.current_poster = None;
            self.poster_fetch_pending = None;
        }
    }

    fn open_settings_choice(&mut self, kind: SettingsChoiceKind) {
        self.settings_ui.choice = Some(SettingsChoiceEditor {
            selected: kind.selected_index(&self.settings),
            kind,
        });
    }

    fn open_settings_threshold(&mut self) {
        self.settings_ui.threshold = Some(SettingsThresholdEditor {
            percent: self.settings.watched_threshold_percent,
        });
    }

    fn activate_general_setting(&mut self) {
        match self.settings_ui.selected {
            0 => {
                self.settings.autoplay_next = !self.settings.autoplay_next;
                self.persist_settings();
            }
            1 => {
                self.settings.resume_from_timestamp = !self.settings.resume_from_timestamp;
                self.persist_settings();
            }
            2 => self.open_settings_threshold(),
            3 => {
                self.settings.search_mode = self.settings.search_mode.toggled();
                self.persist_settings();
            }
            4 => self.open_settings_choice(SettingsChoiceKind::StartScreen),
            5 => self.open_settings_choice(SettingsChoiceKind::LibraryFilter),
            6 => {
                self.toggle_poster_setting();
                self.persist_settings();
            }
            7 => {
                self.settings.discord_presence = !self.settings.discord_presence;
                self.settings_ui.discord_config_changed = true;
                self.persist_settings();
            }
            8 => self.open_settings_text(SettingsInput::MpvPath),
            9 => self.open_settings_text(SettingsInput::MpvArgs),
            _ => {}
        }
    }

    fn activate_theme_setting(&mut self) {
        if self.settings_ui.selected == 0 {
            self.settings.cycle_color_mode();
            self.persist_settings();
        } else if self.settings_ui.selected == 1 {
            self.settings.surface_mode = self.settings.surface_mode.next();
            self.persist_settings();
        } else if self.settings_ui.selected == 2 {
            self.settings.transparent_background = !self.settings.transparent_background;
            self.persist_settings();
        } else if let Some(theme) = ThemePreset::ALL
            .get(self.settings_ui.selected.saturating_sub(3))
            .copied()
        {
            if !self.settings.ansi_themes {
                self.set_info_status("Палітри доступні в режимах ANSI 16 та ANSI 256");
            } else {
                self.settings.theme = theme;
                self.persist_settings();
            }
        }
    }

    fn open_settings_text(&mut self, kind: SettingsInput) {
        let value = match kind {
            SettingsInput::MpvPath => self.settings.mpv_path.clone(),
            SettingsInput::MpvArgs => self.settings.mpv_extra_args.clone(),
        };
        self.settings_ui.input_cursor = value.chars().count();
        self.settings_ui.input_value = value;
        self.settings_ui.input = Some(kind);
    }

    fn spawn_external(&mut self, command: crate::platform::CommandSpec, label: &str) {
        if std::process::Command::new(command.program)
            .args(command.args)
            .spawn()
            .is_ok()
        {
            self.set_info_status(format!("Відкрито: {label}"));
        } else {
            self.set_error_status(format!("Не вдалося відкрити: {label}"));
        }
    }

    fn activate_about_setting(&mut self) {
        match self.settings_ui.selected {
            0 => {
                let path = self.settings_store.data_dir().display().to_string();
                let command =
                    crate::platform::path_open_command(crate::platform::Platform::current(), &path);
                self.spawn_external(command, "теку даних");
            }
            1 => self.open_url_in_browser(GITHUB_URL, "GitHub"),
            2 => self.open_update_popup(),
            3 => match self.poster_disk_cache.clear() {
                Ok(()) => {
                    self.poster_cache.invalidate_all();
                    self.set_info_status("Кеш постерів очищено");
                }
                Err(error) => {
                    self.set_error_status(format!("Не вдалося очистити постери: {error}"));
                }
            },
            4 => self.library.clear_confirmation = true,
            _ => {}
        }
    }

    fn open_update_popup(&mut self) {
        self.settings_ui.update_popup = true;
        if !matches!(self.settings_ui.update_state, UpdateState::Checking) {
            self.settings_ui.update_state = UpdateState::Checking;
            self.settings_ui.update_check_requested = true;
        }
    }

    pub(super) fn handle_settings_update_popup(&mut self, key_code: KeyCode) -> bool {
        if !self.settings_ui.update_popup {
            return false;
        }
        match key_code {
            KeyCode::Enter => match &self.settings_ui.update_state {
                UpdateState::Available(update) => {
                    let url = update.release_url.clone();
                    self.open_url_in_browser(&url, "сторінку оновлення");
                }
                UpdateState::Failed(_) | UpdateState::Current(_) | UpdateState::Idle => {
                    self.settings_ui.update_state = UpdateState::Checking;
                    self.settings_ui.update_check_requested = true;
                }
                UpdateState::Checking => {}
            },
            KeyCode::Esc => self.settings_ui.update_popup = false,
            _ => {}
        }
        true
    }

    pub(super) fn handle_settings_input(&mut self, key_code: KeyCode) -> bool {
        let Some(input) = self.settings_ui.input else {
            return false;
        };
        match key_code {
            KeyCode::Enter => {
                match input {
                    SettingsInput::MpvPath => {
                        self.settings.mpv_path = self.settings_ui.input_value.trim().to_string();
                        if self.settings.mpv_path.is_empty() {
                            self.settings.mpv_path = "mpv".to_string();
                        }
                        self.settings_ui.mpv_available = mpv_is_available(&self.settings.mpv_path);
                    }
                    SettingsInput::MpvArgs => {
                        self.settings.mpv_extra_args =
                            self.settings_ui.input_value.trim().to_string();
                    }
                }
                self.settings_ui.input = None;
                self.settings_ui.input_value.clear();
                self.settings_ui.input_cursor = 0;
                self.persist_settings();
            }
            KeyCode::Esc => {
                self.settings_ui.input = None;
                self.settings_ui.input_value.clear();
                self.settings_ui.input_cursor = 0;
            }
            KeyCode::Home => self.settings_ui.input_cursor = 0,
            KeyCode::End => {
                self.settings_ui.input_cursor = self.settings_ui.input_value.chars().count()
            }
            KeyCode::Left => {
                self.settings_ui.input_cursor = self.settings_ui.input_cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                self.settings_ui.input_cursor = (self.settings_ui.input_cursor + 1)
                    .min(self.settings_ui.input_value.chars().count());
            }
            KeyCode::Backspace if self.settings_ui.input_cursor > 0 => {
                let start = byte_index_for_char(
                    &self.settings_ui.input_value,
                    self.settings_ui.input_cursor - 1,
                );
                let end = byte_index_for_char(
                    &self.settings_ui.input_value,
                    self.settings_ui.input_cursor,
                );
                self.settings_ui.input_value.replace_range(start..end, "");
                self.settings_ui.input_cursor -= 1;
            }
            KeyCode::Backspace => {}
            KeyCode::Delete => {
                let len = self.settings_ui.input_value.chars().count();
                if self.settings_ui.input_cursor < len {
                    let start = byte_index_for_char(
                        &self.settings_ui.input_value,
                        self.settings_ui.input_cursor,
                    );
                    let end = byte_index_for_char(
                        &self.settings_ui.input_value,
                        self.settings_ui.input_cursor + 1,
                    );
                    self.settings_ui.input_value.replace_range(start..end, "");
                }
            }
            KeyCode::Char(character) => {
                let index = byte_index_for_char(
                    &self.settings_ui.input_value,
                    self.settings_ui.input_cursor,
                );
                self.settings_ui.input_value.insert(index, character);
                self.settings_ui.input_cursor += 1;
            }
            _ => {}
        }
        true
    }

    pub(super) fn handle_settings_choice(&mut self, key_code: KeyCode) -> bool {
        let Some(editor) = self.settings_ui.choice.as_mut() else {
            return false;
        };
        let option_count = editor.kind.option_labels().len().max(1);
        match key_code {
            KeyCode::Up | KeyCode::Char('k') => {
                editor.selected = editor.selected.checked_sub(1).unwrap_or(option_count - 1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                editor.selected = (editor.selected + 1) % option_count;
            }
            KeyCode::Enter => {
                let editor = self.settings_ui.choice.take().expect("choice editor open");
                match editor.kind {
                    SettingsChoiceKind::StartScreen => {
                        self.settings.start_screen = if editor.selected == 0 {
                            StartScreen::Search
                        } else {
                            StartScreen::Library
                        };
                    }
                    SettingsChoiceKind::LibraryFilter => {
                        let filter = DefaultLibraryFilter::ALL
                            .get(editor.selected)
                            .copied()
                            .unwrap_or(DefaultLibraryFilter::All);
                        self.settings.default_library_filter = filter;
                        self.library.filter = library_filter_from_setting(filter);
                    }
                }
                self.persist_settings();
            }
            KeyCode::Esc => self.settings_ui.choice = None,
            _ => {}
        }
        true
    }

    pub(super) fn handle_settings_threshold(&mut self, key_code: KeyCode) -> bool {
        let Some(editor) = self.settings_ui.threshold.as_mut() else {
            return false;
        };
        match key_code {
            KeyCode::Char(' ') => {
                if editor.percent.is_some() {
                    editor.percent = None;
                } else {
                    editor.percent = Some(90);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                let current = editor.percent.unwrap_or(THRESHOLD_MIN);
                editor.percent = Some(current.saturating_sub(THRESHOLD_STEP).max(THRESHOLD_MIN));
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let current = editor.percent.unwrap_or(THRESHOLD_MIN);
                editor.percent = Some((current + THRESHOLD_STEP).min(THRESHOLD_MAX));
            }
            KeyCode::Home => editor.percent = Some(THRESHOLD_MIN),
            KeyCode::End => editor.percent = Some(THRESHOLD_MAX),
            KeyCode::Enter => {
                let editor = self
                    .settings_ui
                    .threshold
                    .take()
                    .expect("threshold editor open");
                self.settings.watched_threshold_percent = editor.percent;
                self.persist_settings();
            }
            KeyCode::Esc => self.settings_ui.threshold = None,
            _ => {}
        }
        true
    }

    pub(super) fn handle_settings_key(&mut self, key_code: KeyCode) {
        let rows = match self.settings_ui.tab {
            SettingsTab::General => 10,
            SettingsTab::Themes => ThemePreset::ALL.len() + 3,
            SettingsTab::About => 5,
        };
        match key_code {
            KeyCode::Tab => {
                self.settings_ui.tab = self.settings_ui.tab.next();
                self.settings_ui.selected = 0;
            }
            KeyCode::BackTab => {
                self.settings_ui.tab = self.settings_ui.tab.previous();
                self.settings_ui.selected = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings_ui.selected =
                    self.settings_ui.selected.checked_sub(1).unwrap_or(rows - 1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings_ui.selected = (self.settings_ui.selected + 1) % rows;
            }
            KeyCode::Char(' ') | KeyCode::Enter => match self.settings_ui.tab {
                SettingsTab::General => self.activate_general_setting(),
                SettingsTab::Themes => self.activate_theme_setting(),
                SettingsTab::About => self.activate_about_setting(),
            },
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => self.switch_primary_tab(PrimaryTab::Search),
            _ => {}
        }
    }
}
