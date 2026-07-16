use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use semver::Version;
use serde::{Deserialize, Serialize};
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
    Violet,
    Ocean,
    Amber,
    Sakura,
    Matrix,
    Monochrome,
}

impl ThemePreset {
    pub const ALL: [Self; 6] = [
        Self::Violet,
        Self::Ocean,
        Self::Amber,
        Self::Sakura,
        Self::Matrix,
        Self::Monochrome,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Violet => "Неон",
            Self::Ocean => "Океан",
            Self::Amber => "Бурштин",
            Self::Sakura => "Сакура",
            Self::Matrix => "Матриця",
            Self::Monochrome => "Моно",
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
#[serde(default, deny_unknown_fields)]
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
    pub show_posters: bool,
    pub ansi_themes: bool,
    pub theme: ThemePreset,
    pub discord_presence: bool,
    pub discord_application_id: String,
    pub mpv_path: String,
    pub mpv_extra_args: String,
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
            show_posters: true,
            ansi_themes: false,
            theme: ThemePreset::Violet,
            discord_presence: false,
            discord_application_id: String::new(),
            mpv_path: "mpv".to_string(),
            mpv_extra_args: String::new(),
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
        assert_eq!(settings.theme, ThemePreset::Violet);
        assert!(!settings.discord_presence);
        assert!(settings.discord_application_id.is_empty());
        assert_eq!(settings.mpv_path, "mpv");
    }

    #[test]
    fn search_mode_toggles_between_strict_and_extended() {
        assert_eq!(SearchMode::Strict.toggled(), SearchMode::Extended);
        assert_eq!(SearchMode::Extended.toggled(), SearchMode::Strict);
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
        object.remove("discord_presence");
        object.remove("discord_application_id");

        let settings: Settings = serde_json::from_value(value).unwrap();
        assert!(!settings.ansi_themes);
        assert_eq!(settings.theme, ThemePreset::Violet);
        assert!(!settings.discord_presence);
        assert!(settings.discord_application_id.is_empty());
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
        store.save(&settings).unwrap();

        assert_eq!(store.load().unwrap(), settings);
        fs::remove_dir_all(data_dir).unwrap();
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
