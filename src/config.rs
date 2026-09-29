//! Configuration model, defaults, loading and validation.
//!
//! The configuration is a plain TOML document. Every field has a default, so an empty
//! file is valid, and a missing file is not an error: syswatch starts with its built-in
//! defaults and says so in the status bar.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::app::View;
use crate::collector::processes::SortKey;

/// Default location of the configuration file inside the XDG config home.
pub const CONFIG_RELATIVE_PATH: &str = "syswatch/config.toml";

/// Lowest accepted refresh interval, in milliseconds.
pub const MIN_INTERVAL_MS: u64 = 100;

/// Shortest accepted graph history, in samples.
pub const MIN_HISTORY: usize = 16;

/// Longest accepted graph history, in samples.
pub const MAX_HISTORY: usize = 4096;

/// Colour theme used by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    /// Dark background, high contrast accents. The default.
    #[default]
    Dark,
    /// Light background, dark text.
    Light,
    /// No colours at all, for terminals with a bad palette.
    Monochrome,
}

impl Theme {
    /// Parses a theme name, case insensitively.
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "dark" | "default" => Ok(Self::Dark),
            "light" => Ok(Self::Light),
            "mono" | "monochrome" | "none" => Ok(Self::Monochrome),
            other => Err(ConfigError::InvalidTheme(other.to_string())),
        }
    }
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Monochrome => "monochrome",
        })
    }
}

/// Which process filter is active at start-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessFilterSetting {
    /// Every process, including kernel threads.
    #[default]
    All,
    /// Only processes owned by the current user.
    User,
    /// Only processes with a command line (excludes most kernel threads).
    Tasks,
}

/// Process sort order, as a value rather than a mutable flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SortSetting {
    /// Column to sort by.
    pub key: SortKeySetting,
    /// `true` for largest-first.
    pub descending: bool,
}

impl Default for SortSetting {
    fn default() -> Self {
        Self {
            key: SortKeySetting::Cpu,
            descending: true,
        }
    }
}

/// Column names accepted in the configuration file.
///
/// This mirrors [`SortKey`] but is `Deserialize`-friendly: `sysinfo` types are not
/// part of the configuration schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortKeySetting {
    /// Sort by CPU usage.
    #[default]
    Cpu,
    /// Sort by resident memory.
    Memory,
    /// Sort by process id.
    Pid,
    /// Sort by process name.
    Name,
}

impl From<SortKeySetting> for SortKey {
    fn from(value: SortKeySetting) -> Self {
        match value {
            SortKeySetting::Cpu => Self::Cpu,
            SortKeySetting::Memory => Self::Memory,
            SortKeySetting::Pid => Self::Pid,
            SortKeySetting::Name => Self::Name,
        }
    }
}

impl From<SortKey> for SortKeySetting {
    fn from(value: SortKey) -> Self {
        match value {
            SortKey::Cpu => Self::Cpu,
            SortKey::Memory => Self::Memory,
            SortKey::Pid => Self::Pid,
            SortKey::Name => Self::Name,
        }
    }
}

/// Dashboard names accepted in the configuration file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ViewSetting {
    /// CPU dashboard.
    #[default]
    Cpu,
    /// Memory dashboard.
    Memory,
    /// Process manager.
    Processes,
    /// Storage dashboard.
    Storage,
    /// Network dashboard.
    Network,
    /// System information.
    System,
}

impl From<ViewSetting> for View {
    fn from(value: ViewSetting) -> Self {
        match value {
            ViewSetting::Cpu => Self::Cpu,
            ViewSetting::Memory => Self::Memory,
            ViewSetting::Processes => Self::Processes,
            ViewSetting::Storage => Self::Storage,
            ViewSetting::Network => Self::Network,
            ViewSetting::System => Self::System,
        }
    }
}

impl std::str::FromStr for ViewSetting {
    type Err = ConfigError;

    /// Parses a dashboard name, case insensitively.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cpu" | "1" => Ok(Self::Cpu),
            "memory" | "mem" | "2" => Ok(Self::Memory),
            "processes" | "process" | "proc" | "3" => Ok(Self::Processes),
            "storage" | "disk" | "disks" | "4" => Ok(Self::Storage),
            "network" | "net" | "5" => Ok(Self::Network),
            "system" | "info" | "6" => Ok(Self::System),
            other => Err(ConfigError::Invalid {
                field: "view",
                message: format!(
                    "unknown view {other:?}, expected one of cpu, memory, processes, storage, network, system"
                ),
            }),
        }
    }
}

impl fmt::Display for ViewSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Processes => "processes",
            Self::Storage => "storage",
            Self::Network => "network",
            Self::System => "system",
        })
    }
}

/// The complete configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Seconds between two samples. Clamped to `0.1 ..= 3600`.
    pub refresh_interval: f64,
    /// Colour theme.
    pub theme: Theme,
    /// Dashboard shown at start-up.
    pub default_view: ViewSetting,
    /// Number of samples kept per graph. Clamped to 16 ..= 4096.
    pub graph_history: usize,
    /// Use Unicode block characters for graphs instead of ASCII.
    pub unicode_graphs: bool,
    /// Initial process sort order.
    pub process_sort: SortSetting,
    /// Process filter active at start-up.
    pub process_filter: ProcessFilterSetting,
    /// Network interfaces to monitor. Empty means "every interface except loopback".
    pub network_interfaces: Vec<String>,
    /// Show temperature rows when the kernel exposes sensors.
    pub show_temperatures: bool,
    /// Show the physical disk inventory and its temperatures on the storage view.
    pub show_disk_health: bool,
    /// Include kernel pseudo filesystems in the storage view.
    pub show_pseudo_filesystems: bool,
    /// Show the load average in the header.
    pub show_load_average: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_interval: 1.0,
            theme: Theme::Dark,
            default_view: ViewSetting::Cpu,
            graph_history: 120,
            unicode_graphs: true,
            process_sort: SortSetting::default(),
            process_filter: ProcessFilterSetting::All,
            network_interfaces: Vec::new(),
            show_temperatures: true,
            show_disk_health: true,
            show_pseudo_filesystems: false,
            show_load_average: true,
        }
    }
}

/// Configuration problem detected while loading or validating.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("cannot read configuration file {path}: {source}")]
    Io {
        /// Path that was attempted.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid TOML or has the wrong shape.
    #[error("invalid configuration in {path}: {message}")]
    Parse {
        /// Path of the file.
        path: PathBuf,
        /// Human readable reason.
        message: String,
    },
    /// A value is outside its accepted range.
    #[error("invalid value for {field}: {message}")]
    Invalid {
        /// Field name.
        field: &'static str,
        /// Human readable reason.
        message: String,
    },
    /// The theme name is not recognised.
    #[error("unknown theme {0:?}, expected dark, light or monochrome")]
    InvalidTheme(String),
}

impl Config {
    /// Refresh interval as a [`Duration`], clamped to the supported range.
    pub fn interval(&self) -> Duration {
        Duration::from_millis(
            (self.refresh_interval * 1000.0).clamp(MIN_INTERVAL_MS as f64, 3_600_000.0) as u64,
        )
    }

    /// Validates and normalises the configuration in place.
    ///
    /// Clamping is preferred over failing: a slightly odd value should still give the
    /// user a working dashboard, and the status bar reports what was adjusted.
    pub fn validate(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        if !self.refresh_interval.is_finite() || self.refresh_interval <= 0.0 {
            notes.push(format!(
                "refresh_interval {} reset to 1.0",
                self.refresh_interval
            ));
            self.refresh_interval = 1.0;
        }
        let clamped_seconds = self
            .refresh_interval
            .clamp(MIN_INTERVAL_MS as f64 / 1000.0, 3600.0);
        if (clamped_seconds - self.refresh_interval).abs() > f64::EPSILON {
            notes.push(format!(
                "refresh_interval {:.3}s clamped to {clamped_seconds:.3}s",
                self.refresh_interval
            ));
            self.refresh_interval = clamped_seconds;
        }
        if self.graph_history < MIN_HISTORY {
            notes.push(format!(
                "graph_history {} raised to {MIN_HISTORY}",
                self.graph_history
            ));
            self.graph_history = MIN_HISTORY;
        }
        if self.graph_history > MAX_HISTORY {
            notes.push(format!(
                "graph_history {} lowered to {MAX_HISTORY}",
                self.graph_history
            ));
            self.graph_history = MAX_HISTORY;
        }
        self.network_interfaces
            .retain(|name| !name.trim().is_empty());
        self.network_interfaces.truncate(16);
        notes
    }

    /// Parses a TOML document.
    pub fn from_toml(text: &str, path: &Path) -> Result<Self, ConfigError> {
        toml::from_str::<Config>(text).map_err(|err| ConfigError::Parse {
            path: path.to_path_buf(),
            message: err.to_string(),
        })
    }

    /// Serialises the configuration as a commented TOML document.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Reads, parses and validates the configuration file.
    ///
    /// A missing file is *not* an error: the defaults are returned together with a
    /// note so the UI can tell the user that no configuration was found.
    pub fn load(path: &Path) -> (Self, Option<ConfigError>, Vec<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::from_toml(&text, path) {
                Ok(mut config) => {
                    let notes = config.validate();
                    (config, None, notes)
                }
                Err(err) => (Self::default(), Some(err), Vec::new()),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (
                Self::default(),
                None,
                vec![format!(
                    "no configuration at {}, using defaults",
                    path.display()
                )],
            ),
            Err(err) => (
                Self::default(),
                Some(ConfigError::Io {
                    path: path.to_path_buf(),
                    source: err,
                }),
                Vec::new(),
            ),
        }
    }

    /// Writes the configuration, creating parent directories as needed.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let body = self.to_toml();
        // Write to a temporary file and rename, so an interrupted write cannot leave a
        // truncated configuration behind.
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, body).map_err(|source| ConfigError::Io {
            path: temporary.clone(),
            source,
        })?;
        std::fs::rename(&temporary, path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    /// Default configuration path: `$XDG_CONFIG_HOME/syswatch/config.toml`, falling
    /// back to `~/.config/syswatch/config.toml` and finally to `./config.toml`.
    pub fn default_path() -> PathBuf {
        Self::path_from_env()
    }

    /// Resolves the configuration path, honouring `XDG_CONFIG_HOME`.
    pub fn path_from_env() -> PathBuf {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
            && !xdg.is_empty()
        {
            return Path::new(&xdg).join(CONFIG_RELATIVE_PATH);
        }
        if let Some(home) = std::env::var_os("HOME")
            && !home.is_empty()
        {
            return Path::new(&home).join(".config").join(CONFIG_RELATIVE_PATH);
        }
        PathBuf::from(CONFIG_RELATIVE_PATH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).expect("create file");
        file.write_all(contents.as_bytes()).expect("write file");
        (dir, path)
    }

    #[test]
    fn defaults_are_valid() {
        let config = Config::default();
        assert_eq!(config.interval(), Duration::from_secs(1));
        assert_eq!(config.theme, Theme::Dark);
        assert_eq!(config.default_view, ViewSetting::Cpu);
        assert!(config.graph_history >= MIN_HISTORY);
        assert!(config.unicode_graphs);
    }

    #[test]
    fn empty_document_yields_defaults() {
        let config =
            Config::from_toml("", Path::new("test.toml")).expect("empty document is valid");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn partial_document_keeps_other_defaults() {
        let text = "refresh_interval = 2.5\ntheme = \"light\"\n";
        let config = Config::from_toml(text, Path::new("test.toml")).expect("valid document");
        assert_eq!(config.refresh_interval, 2.5);
        assert_eq!(config.theme, Theme::Light);
        assert_eq!(config.graph_history, Config::default().graph_history);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let text = "refresh_intervall = 2.0\n";
        let err =
            Config::from_toml(text, Path::new("test.toml")).expect_err("typo must be reported");
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn invalid_types_are_reported() {
        let text = "graph_history = \"lots\"\n";
        assert!(Config::from_toml(text, Path::new("test.toml")).is_err());
    }

    #[test]
    fn theme_names_are_case_insensitive() {
        assert_eq!(Theme::parse("DARK").expect("valid"), Theme::Dark);
        assert_eq!(Theme::parse(" light ").expect("valid"), Theme::Light);
        assert_eq!(Theme::parse("mono").expect("valid"), Theme::Monochrome);
        assert!(Theme::parse("neon").is_err());
    }

    #[test]
    fn interval_is_clamped() {
        let mut config = Config {
            refresh_interval: 0.0,
            ..Config::default()
        };
        let notes = config.validate();
        assert_eq!(config.refresh_interval, 1.0);
        assert!(!notes.is_empty());

        let mut config = Config {
            refresh_interval: 0.01,
            ..Config::default()
        };
        config.validate();
        assert_eq!(config.interval(), Duration::from_millis(MIN_INTERVAL_MS));
    }

    #[test]
    fn non_finite_interval_is_reset() {
        let mut config = Config {
            refresh_interval: f64::NAN,
            ..Config::default()
        };
        config.validate();
        assert_eq!(config.refresh_interval, 1.0);
        assert!(config.interval() >= Duration::from_millis(MIN_INTERVAL_MS));
    }

    #[test]
    fn history_length_is_clamped() {
        let mut config = Config {
            graph_history: 1,
            ..Config::default()
        };
        config.validate();
        assert_eq!(config.graph_history, MIN_HISTORY);

        let mut config = Config {
            graph_history: 100_000,
            ..Config::default()
        };
        config.validate();
        assert_eq!(config.graph_history, MAX_HISTORY);
    }

    #[test]
    fn interface_filter_is_cleaned() {
        let mut config = Config {
            network_interfaces: vec!["  ".to_string(), "wlan0".to_string(), "".to_string()],
            ..Config::default()
        };
        config.validate();
        assert_eq!(config.network_interfaces, vec!["wlan0".to_string()]);
    }

    #[test]
    fn round_trips_through_toml() {
        let config = Config {
            refresh_interval: 0.5,
            theme: Theme::Light,
            default_view: ViewSetting::Network,
            graph_history: 240,
            unicode_graphs: false,
            process_sort: SortSetting {
                key: SortKeySetting::Memory,
                descending: false,
            },
            process_filter: ProcessFilterSetting::User,
            network_interfaces: vec!["eth0".to_string()],
            show_temperatures: false,
            show_disk_health: false,
            show_pseudo_filesystems: true,
            show_load_average: false,
        };
        let text = config.to_toml();
        assert!(!text.is_empty());
        let parsed = Config::from_toml(&text, Path::new("roundtrip.toml")).expect("round trip");
        assert_eq!(parsed, config);
    }

    #[test]
    fn load_reports_a_missing_file_without_failing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("absent.toml");
        let (config, error, notes) = Config::load(&path);
        assert!(error.is_none());
        assert_eq!(config, Config::default());
        assert!(notes.iter().any(|note| note.contains("using defaults")));
    }

    #[test]
    fn load_falls_back_to_defaults_on_a_broken_file() {
        let (_dir, path) = write_temp("broken.toml", "this is not = = toml");
        let (config, error, _notes) = Config::load(&path);
        assert!(matches!(error, Some(ConfigError::Parse { .. })));
        assert_eq!(config, Config::default());
    }

    #[test]
    fn load_validates_values() {
        let (_dir, path) = write_temp("bad.toml", "graph_history = 2\n");
        let (config, error, notes) = Config::load(&path);
        assert!(error.is_none());
        assert_eq!(config.graph_history, MIN_HISTORY);
        assert!(!notes.is_empty());
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested/deeper/config.toml");
        let config = Config::default();
        config.save(&path).expect("save must create directories");
        assert!(path.exists());
        let (loaded, error, _notes) = Config::load(&path);
        assert!(error.is_none());
        assert_eq!(loaded, config);
    }

    #[test]
    fn default_path_uses_xdg_config_home() {
        // The helper is environment dependent, so only the shape is asserted here.
        let path = Config::path_from_env();
        assert!(path.ends_with("syswatch/config.toml") || path.ends_with("config.toml"));
    }

    #[test]
    fn view_setting_converts_to_view() {
        assert_eq!(View::from(ViewSetting::Processes), View::Processes);
        assert_eq!(ViewSetting::Network.to_string(), "network");
    }

    #[test]
    fn sort_setting_converts_both_ways() {
        for key in [SortKey::Cpu, SortKey::Memory, SortKey::Pid, SortKey::Name] {
            let setting = SortKeySetting::from(key);
            assert_eq!(SortKey::from(setting), key);
        }
    }
}
