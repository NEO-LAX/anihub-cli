use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const GITHUB_URL: &str = "https://github.com/NEO-LAX/anihub-cli";
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/NEO-LAX/anihub-cli/releases/latest";
const SETTINGS_SCHEMA_VERSION: u32 = 1;
const SETTINGS_FILE_NAME: &str = "settings.json";
const LEGACY_SETTINGS_FILE_NAME: &str = "settings-v1.json";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartScreen {
    #[default]
    Search,
    Library,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Strict,
    Extended,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreset {
    #[default]
    #[serde(alias = "violet")]
    CatppuccinMocha,
    #[serde(alias = "ocean")]
    TokyoNight,
    #[serde(alias = "amber")]
    KanagawaWave,
    #[serde(alias = "sakura")]
    RosePine,
    #[serde(alias = "monochrome")]
    GruvboxDark,
    #[serde(alias = "matrix")]
    EverforestDark,
    AyuDark,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColorMode {
    #[default]
    AniHubRgb,
    Ansi16,
    Ansi256,
}

impl ColorMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::AniHubRgb => "AniHub RGB",
            Self::Ansi16 => "ANSI 16",
            Self::Ansi256 => "ANSI 256",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceMode {
    #[default]
    Auto,
    Dark,
    Light,
}

impl SurfaceMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Авто",
            Self::Dark => "Темна",
            Self::Light => "Світла",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Auto => Self::Dark,
            Self::Dark => Self::Light,
            Self::Light => Self::Auto,
        }
    }
}

impl ThemePreset {
    pub const ALL: [Self; 7] = [
        Self::CatppuccinMocha,
        Self::TokyoNight,
        Self::KanagawaWave,
        Self::RosePine,
        Self::GruvboxDark,
        Self::EverforestDark,
        Self::AyuDark,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::TokyoNight => "Tokyo Night",
            Self::KanagawaWave => "Kanagawa Wave",
            Self::RosePine => "Rosé Pine",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::EverforestDark => "Everforest Dark",
            Self::AyuDark => "Ayu Dark",
        }
    }
}

impl SearchMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Strict => "Звичайний · 20",
            Self::Extended => "Розширений · до 100",
        }
    }

    pub const fn toggled(self) -> Self {
        match self {
            Self::Strict => Self::Extended,
            Self::Extended => Self::Strict,
        }
    }

    pub const fn is_extended(self) -> bool {
        matches!(self, Self::Extended)
    }
}

impl StartScreen {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Search => "Пошук",
            Self::Library => "Бібліотека",
        }
    }

    pub const fn toggled(self) -> Self {
        match self {
            Self::Search => Self::Library,
            Self::Library => Self::Search,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultLibraryFilter {
    #[default]
    All,
    Watching,
    Planned,
    Completed,
    OnHold,
    Dropped,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibrarySortPreference {
    #[default]
    Recent,
    Title,
    Year,
    Rating,
    Progress,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSortPreference {
    #[default]
    Relevance,
    Title,
    Year,
    Rating,
}

impl DefaultLibraryFilter {
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::Watching,
        Self::Planned,
        Self::Completed,
        Self::OnHold,
        Self::Dropped,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "Усі",
            Self::Watching => "Дивлюся",
            Self::Planned => "У планах",
            Self::Completed => "Переглянуто",
            Self::OnHold => "Відкладено",
            Self::Dropped => "Кинуто",
        }
    }

    pub fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub autoplay_next: bool,
    pub resume_from_timestamp: bool,
    /// Percentage at which a partial playback snapshot becomes watched.
    /// `None` means only natural EOF or an explicit Space/status action marks it.
    pub watched_threshold_percent: Option<u8>,
    pub search_mode: SearchMode,
    pub start_screen: StartScreen,
    pub default_library_filter: DefaultLibraryFilter,
    /// Last interactive library state. Unlike `default_library_filter`, these
    /// values follow the user's latest session rather than a settings choice.
    pub last_library_filter: Option<DefaultLibraryFilter>,
    pub library_sort: LibrarySortPreference,
    pub library_sort_reversed: bool,
    pub search_sort: SearchSortPreference,
    pub search_sort_reversed: bool,
    pub last_library_anime_id: Option<u32>,
    /// Highest episode count acknowledged by opening each release.
    pub seen_episode_counts: BTreeMap<u32, u32>,
    pub show_posters: bool,
    pub ansi_themes: bool,
    pub ansi_256_colors: bool,
    pub theme: ThemePreset,
    pub surface_mode: SurfaceMode,
    pub transparent_background: bool,
    pub discord_presence: bool,
    /// Read only for migrating settings written before AniHub shipped a shared
    /// Discord application. New settings files omit this obsolete override.
    #[serde(rename = "discord_application_id", skip_serializing)]
    pub legacy_discord_application_id: String,
    pub mpv_path: String,
    pub mpv_extra_args: String,
    /// Preserve settings written by a newer AniHub version. Older binaries
    /// may not understand these keys yet, but must not call the file corrupt
    /// or silently erase them on the next save.
    #[serde(flatten)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            autoplay_next: true,
            resume_from_timestamp: true,
            watched_threshold_percent: Some(90),
            search_mode: SearchMode::Strict,
            start_screen: StartScreen::Search,
            default_library_filter: DefaultLibraryFilter::All,
            last_library_filter: None,
            library_sort: LibrarySortPreference::Recent,
            library_sort_reversed: false,
            search_sort: SearchSortPreference::Relevance,
            search_sort_reversed: false,
            last_library_anime_id: None,
            seen_episode_counts: BTreeMap::new(),
            show_posters: true,
            ansi_themes: false,
            ansi_256_colors: false,
            theme: ThemePreset::CatppuccinMocha,
            surface_mode: SurfaceMode::Auto,
            transparent_background: true,
            discord_presence: false,
            legacy_discord_application_id: String::new(),
            mpv_path: "mpv".to_string(),
            mpv_extra_args: String::new(),
            unknown_fields: BTreeMap::new(),
        }
    }
}

impl Settings {
    pub const fn color_mode(&self) -> ColorMode {
        if !self.ansi_themes {
            ColorMode::AniHubRgb
        } else if self.ansi_256_colors {
            ColorMode::Ansi256
        } else {
            ColorMode::Ansi16
        }
    }

    pub fn cycle_color_mode(&mut self) {
        match self.color_mode() {
            ColorMode::AniHubRgb => {
                self.ansi_themes = true;
                self.ansi_256_colors = false;
            }
            ColorMode::Ansi16 => self.ansi_256_colors = true,
            ColorMode::Ansi256 => {
                self.ansi_themes = false;
                self.ansi_256_colors = false;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct SettingsStore {
    data_dir: PathBuf,
    settings_file: PathBuf,
}

impl SettingsStore {
    pub fn new() -> Result<Self> {
        let project_dirs = ProjectDirs::from("com", "shadowgarden", "anihub-cli")
            .ok_or_else(|| anyhow!("не вдалося визначити теку даних"))?;
        let data_dir = project_dirs.data_dir().to_path_buf();
        fs::create_dir_all(&data_dir)
            .with_context(|| format!("не вдалося створити {}", data_dir.display()))?;
        Ok(Self {
            settings_file: data_dir.join(SETTINGS_FILE_NAME),
            data_dir,
        })
    }

    pub fn load(&self) -> Result<Settings> {
        if !self.settings_file.exists() {
            let legacy_file = self.data_dir.join(LEGACY_SETTINGS_FILE_NAME);
            if !legacy_file.exists() {
                return Ok(Settings::default());
            }

            let settings = Self::read_settings(&legacy_file)?;
            self.save(&settings).with_context(|| {
                format!(
                    "не вдалося перенести {} у {}",
                    legacy_file.display(),
                    self.settings_file.display()
                )
            })?;
            return Ok(settings);
        }

        Self::read_settings(&self.settings_file)
    }

    fn read_settings(path: &Path) -> Result<Settings> {
        let bytes =
            fs::read(path).with_context(|| format!("не вдалося прочитати {}", path.display()))?;
        let settings: Settings = serde_json::from_slice(&bytes)
            .with_context(|| format!("пошкоджено {}", path.display()))?;
        if settings.schema_version != SETTINGS_SCHEMA_VERSION {
            return Err(anyhow!(
                "непідтримувана версія settings: {}",
                settings.schema_version
            ));
        }
        Ok(settings)
    }

    pub fn save(&self, settings: &Settings) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(settings)?;
        let temporary = self.settings_file.with_extension("json.tmp");
        fs::write(&temporary, bytes)
            .with_context(|| format!("не вдалося записати {}", temporary.display()))?;
        if let Err(first_error) = fs::rename(&temporary, &self.settings_file) {
            if self.settings_file.exists() {
                fs::remove_file(&self.settings_file).with_context(|| {
                    format!("не вдалося замінити {}", self.settings_file.display())
                })?;
                fs::rename(&temporary, &self.settings_file).with_context(|| {
                    format!("не вдалося оновити {}", self.settings_file.display())
                })?;
            } else {
                return Err(first_error).with_context(|| {
                    format!("не вдалося оновити {}", self.settings_file.display())
                });
            }
        }
        Ok(())
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn settings_path(&self) -> &Path {
        &self.settings_file
    }

    pub fn history_path(&self) -> PathBuf {
        self.data_dir.join("history.json")
    }

    pub fn poster_cache_dir(&self) -> PathBuf {
        self.data_dir.join("posters")
    }
}

pub fn mpv_is_available(path: &str) -> bool {
    if path.trim().is_empty() {
        return false;
    }
    Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateCheck {
    pub latest_version: String,
    pub release_url: String,
    pub update_available: bool,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

pub async fn check_for_update(current: &str) -> Result<UpdateCheck> {
    let client = reqwest::Client::builder()
        .user_agent(format!("anihub-cli/{current}"))
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let release = client
        .get(LATEST_RELEASE_API)
        .send()
        .await?
        .error_for_status()?
        .json::<GithubRelease>()
        .await?;
    let update_available = is_newer_version(current, &release.tag_name)?;
    Ok(UpdateCheck {
        latest_version: release.tag_name,
        release_url: release.html_url,
        update_available,
    })
}

fn is_newer_version(current: &str, latest: &str) -> Result<bool> {
    let current_version = Version::parse(current.trim_start_matches('v'))?;
    let latest_version = Version::parse(latest.trim_start_matches('v'))?;
    Ok(latest_version > current_version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn defaults_are_conservative_and_use_mpv_from_path() {
        let settings = Settings::default();
        assert!(settings.autoplay_next);
        assert!(settings.resume_from_timestamp);
        assert_eq!(settings.watched_threshold_percent, Some(90));
        assert_eq!(settings.search_mode, SearchMode::Strict);
        assert!(!settings.ansi_themes);
        assert!(!settings.ansi_256_colors);
        assert_eq!(settings.color_mode(), ColorMode::AniHubRgb);
        assert_eq!(settings.theme, ThemePreset::CatppuccinMocha);
        assert_eq!(settings.surface_mode, SurfaceMode::Auto);
        assert!(settings.transparent_background);
        assert!(!settings.discord_presence);
        assert!(settings.legacy_discord_application_id.is_empty());
        assert_eq!(settings.mpv_path, "mpv");
    }

    #[test]
    fn search_mode_toggles_between_strict_and_extended() {
        assert_eq!(SearchMode::Strict.toggled(), SearchMode::Extended);
        assert_eq!(SearchMode::Extended.toggled(), SearchMode::Strict);
    }

    #[test]
    fn color_mode_cycles_rgb_ansi16_and_ansi256() {
        let mut settings = Settings::default();
        settings.cycle_color_mode();
        assert_eq!(settings.color_mode(), ColorMode::Ansi16);
        settings.cycle_color_mode();
        assert_eq!(settings.color_mode(), ColorMode::Ansi256);
        settings.cycle_color_mode();
        assert_eq!(settings.color_mode(), ColorMode::AniHubRgb);
    }

    #[test]
    fn surface_mode_cycles_auto_dark_and_light() {
        assert_eq!(SurfaceMode::Auto.next(), SurfaceMode::Dark);
        assert_eq!(SurfaceMode::Dark.next(), SurfaceMode::Light);
        assert_eq!(SurfaceMode::Light.next(), SurfaceMode::Auto);
    }

    #[test]
    fn settings_without_search_mode_keep_strict_default() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("search_mode");
        let settings: Settings = serde_json::from_value(value).unwrap();
        assert_eq!(settings.search_mode, SearchMode::Strict);
    }

    #[test]
    fn older_settings_keep_theme_and_discord_defaults() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("theme");
        object.remove("ansi_themes");
        object.remove("ansi_256_colors");
        object.remove("surface_mode");
        object.remove("transparent_background");
        object.remove("discord_presence");
        object.remove("discord_application_id");
        object.remove("last_library_filter");
        object.remove("library_sort");
        object.remove("library_sort_reversed");
        object.remove("search_sort");
        object.remove("search_sort_reversed");
        object.remove("last_library_anime_id");
        object.remove("seen_episode_counts");

        let settings: Settings = serde_json::from_value(value).unwrap();
        assert!(!settings.ansi_themes);
        assert!(!settings.ansi_256_colors);
        assert_eq!(settings.theme, ThemePreset::CatppuccinMocha);
        assert_eq!(settings.surface_mode, SurfaceMode::Auto);
        assert!(settings.transparent_background);
        assert!(!settings.discord_presence);
        assert!(settings.legacy_discord_application_id.is_empty());
        assert_eq!(settings.last_library_filter, None);
        assert_eq!(settings.library_sort, LibrarySortPreference::Recent);
        assert!(!settings.library_sort_reversed);
        assert_eq!(settings.search_sort, SearchSortPreference::Relevance);
        assert!(!settings.search_sort_reversed);
        assert_eq!(settings.last_library_anime_id, None);
        assert!(settings.seen_episode_counts.is_empty());
    }

    #[test]
    fn old_theme_names_map_to_the_replacement_palettes() {
        let cases = [
            ("violet", ThemePreset::CatppuccinMocha),
            ("ocean", ThemePreset::TokyoNight),
            ("amber", ThemePreset::KanagawaWave),
            ("sakura", ThemePreset::RosePine),
            ("monochrome", ThemePreset::GruvboxDark),
            ("matrix", ThemePreset::EverforestDark),
        ];
        for (stored, expected) in cases {
            let parsed: ThemePreset = serde_json::from_str(&format!("\"{stored}\"")).unwrap();
            assert_eq!(parsed, expected);
        }
    }

    #[test]
    fn obsolete_discord_application_id_is_accepted_but_not_saved_again() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().insert(
            "discord_application_id".to_string(),
            serde_json::Value::String("123456".to_string()),
        );

        let settings: Settings = serde_json::from_value(value).unwrap();
        assert_eq!(settings.legacy_discord_application_id, "123456");

        let saved = serde_json::to_value(settings).unwrap();
        assert!(saved.get("discord_application_id").is_none());
    }

    #[test]
    fn library_filter_cycles() {
        assert_eq!(
            DefaultLibraryFilter::All.next(),
            DefaultLibraryFilter::Watching
        );
        assert_eq!(
            DefaultLibraryFilter::Dropped.next(),
            DefaultLibraryFilter::All
        );
    }

    #[test]
    fn semantic_update_comparison_ignores_v_prefix() {
        assert!(is_newer_version("0.6.0", "v0.6.1").unwrap());
        assert!(!is_newer_version("0.6.0", "v0.6.0").unwrap());
        assert!(!is_newer_version("0.6.0", "v0.5.9").unwrap());
    }

    #[test]
    fn settings_round_trip_and_replace_existing_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!(
            "anihub-settings-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&data_dir).unwrap();
        let store = SettingsStore {
            settings_file: data_dir.join(SETTINGS_FILE_NAME),
            data_dir: data_dir.clone(),
        };
        let mut settings = Settings::default();
        store.save(&settings).unwrap();
        settings.autoplay_next = false;
        settings.mpv_extra_args = "--fs --hwdec=auto".to_string();
        settings.last_library_filter = Some(DefaultLibraryFilter::Watching);
        settings.library_sort = LibrarySortPreference::Progress;
        settings.library_sort_reversed = true;
        settings.search_sort = SearchSortPreference::Rating;
        settings.search_sort_reversed = true;
        settings.last_library_anime_id = Some(42);
        settings.seen_episode_counts.insert(42, 8);
        settings.unknown_fields.insert(
            "future_option".to_string(),
            serde_json::json!({ "enabled": true }),
        );
        store.save(&settings).unwrap();

        assert_eq!(store.load().unwrap(), settings);
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn unknown_settings_survive_load_and_save() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("future_option".to_string(), serde_json::json!([1, 2, 3]));

        let settings: Settings = serde_json::from_value(value).unwrap();
        assert_eq!(
            settings.unknown_fields.get("future_option"),
            Some(&serde_json::json!([1, 2, 3]))
        );
        assert_eq!(
            serde_json::to_value(settings).unwrap().get("future_option"),
            Some(&serde_json::json!([1, 2, 3]))
        );
    }

    #[test]
    fn legacy_versioned_filename_is_copied_to_stable_filename() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!(
            "anihub-settings-migration-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&data_dir).unwrap();
        let store = SettingsStore {
            settings_file: data_dir.join(SETTINGS_FILE_NAME),
            data_dir: data_dir.clone(),
        };
        let expected = Settings {
            show_posters: false,
            ..Settings::default()
        };
        let legacy_path = data_dir.join(LEGACY_SETTINGS_FILE_NAME);
        fs::write(&legacy_path, serde_json::to_vec_pretty(&expected).unwrap()).unwrap();

        assert_eq!(store.load().unwrap(), expected);
        assert!(store.settings_path().exists());
        assert!(legacy_path.exists(), "legacy backup must be retained");

        fs::remove_dir_all(data_dir).unwrap();
    }
}
