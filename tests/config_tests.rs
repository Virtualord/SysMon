//! Integration tests for the configuration layer.
//!
//! These exercise `Config` through its public API only, the way the binary does:
//! load, validate, serialize, persist.

use std::path::Path;

use syswatch::config::{
    Config, ConfigError, MAX_HISTORY, MIN_HISTORY, MIN_INTERVAL_MS, ProcessFilterSetting,
    SortKeySetting, SortSetting, Theme, ViewSetting,
};

/// Writes `contents` to a fresh temporary file and returns its path.
fn config_file(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, contents).expect("write config");
    (dir, path)
}

#[test]
fn a_missing_configuration_is_not_an_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("nested/config.toml");
    let (config, error, notes) = Config::load(&path);

    assert!(
        error.is_none(),
        "a missing file must not be an error: {error:?}"
    );
    assert_eq!(config, Config::default());
    assert!(
        notes.iter().any(|note| note.contains("using defaults")),
        "the user should be told that defaults are in use, got {notes:?}"
    );
}

#[test]
fn the_shipped_default_configuration_is_valid() {
    // `config/default.toml` is what install.sh copies; it must always parse.
    let path = Path::new("config/default.toml");
    assert!(
        path.exists(),
        "config/default.toml is missing from the repository"
    );
    let (_config, error, notes) = Config::load(path);
    assert!(
        error.is_none(),
        "the shipped configuration must be valid: {error:?}"
    );
    assert!(
        notes.is_empty(),
        "the shipped configuration must not need clamping: {notes:?}"
    );
}

#[test]
fn a_documented_configuration_round_trips() {
    let document = r#"
refresh_interval = 2.0
theme = "light"
default_view = "network"
graph_history = 240
unicode_graphs = false
process_filter = "user"
network_interfaces = ["eth0", "wlan0"]
show_temperatures = false
show_pseudo_filesystems = true
show_load_average = false

[process_sort]
key = "memory"
descending = false
"#;
    let (_dir, path) = config_file(document);
    let (config, error, _notes) = Config::load(&path);
    assert!(error.is_none(), "{error:?}");

    assert_eq!(config.refresh_interval, 2.0);
    assert_eq!(config.theme, Theme::Light);
    assert_eq!(config.default_view, ViewSetting::Network);
    assert_eq!(config.graph_history, 240);
    assert!(!config.unicode_graphs);
    assert_eq!(config.process_filter, ProcessFilterSetting::User);
    assert_eq!(config.network_interfaces, vec!["eth0", "wlan0"]);
    assert!(!config.show_temperatures);
    assert!(config.show_pseudo_filesystems);
    assert!(!config.show_load_average);
    assert_eq!(
        config.process_sort,
        SortSetting {
            key: SortKeySetting::Memory,
            descending: false
        }
    );
}

#[test]
fn an_unknown_key_is_rejected_rather_than_ignored() {
    // Silently ignoring a typo would leave the user with settings that never apply.
    let (_dir, path) = config_file("refresh_intervall = 5.0\n");
    let (config, error, _notes) = Config::load(&path);
    assert!(
        matches!(error, Some(ConfigError::Parse { .. })),
        "expected a parse error, got {error:?}"
    );
    assert_eq!(
        config,
        Config::default(),
        "the defaults must be used instead of a broken file"
    );
}

#[test]
fn a_wrongly_typed_value_is_rejected() {
    for document in [
        "refresh_interval = \"fast\"\n",
        "graph_history = true\n",
        "show_temperatures = 1\n",
        "network_interfaces = \"eth0\"\n",
    ] {
        let (_dir, path) = config_file(document);
        let (_config, error, _notes) = Config::load(&path);
        assert!(error.is_some(), "expected {document:?} to be rejected");
    }
}

#[test]
fn out_of_range_values_are_clamped_and_reported() {
    let (_dir, path) = config_file("refresh_interval = 0.001\ngraph_history = 1\n");
    let (config, error, notes) = Config::load(&path);

    assert!(error.is_none());
    assert_eq!(config.graph_history, MIN_HISTORY);
    assert_eq!(config.interval().as_millis(), MIN_INTERVAL_MS as u128);
    assert!(
        notes.len() >= 2,
        "both corrections should be reported, got {notes:?}"
    );
}

#[test]
fn history_length_is_bounded_on_both_sides() {
    let mut config = Config {
        graph_history: 1_000_000,
        ..Config::default()
    };
    config.validate();
    assert_eq!(config.graph_history, MAX_HISTORY);
}

#[test]
fn themes_parse_case_insensitively() {
    for (input, expected) in [
        ("dark", Theme::Dark),
        ("DARK", Theme::Dark),
        (" light ", Theme::Light),
        ("monochrome", Theme::Monochrome),
        ("mono", Theme::Monochrome),
        ("none", Theme::Monochrome),
    ] {
        assert_eq!(
            Theme::parse(input).expect("valid theme"),
            expected,
            "input {input:?}"
        );
    }
    assert!(Theme::parse("chartreuse").is_err());
}

#[test]
fn views_accept_names_and_tab_indexes() {
    use std::str::FromStr;
    for (input, expected) in [
        ("cpu", ViewSetting::Cpu),
        ("MEM", ViewSetting::Memory),
        ("processes", ViewSetting::Processes),
        ("4", ViewSetting::Storage),
        ("net", ViewSetting::Network),
        ("6", ViewSetting::System),
    ] {
        assert_eq!(
            ViewSetting::from_str(input).expect("valid view"),
            expected,
            "input {input:?}"
        );
    }
    assert!(ViewSetting::from_str("nope").is_err());
}

#[test]
fn saving_and_loading_preserves_every_field() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("a/b/c/config.toml");
    let original = Config {
        refresh_interval: 0.5,
        theme: Theme::Monochrome,
        default_view: ViewSetting::Storage,
        graph_history: 64,
        unicode_graphs: false,
        process_sort: SortSetting {
            key: SortKeySetting::Name,
            descending: false,
        },
        process_filter: ProcessFilterSetting::Tasks,
        network_interfaces: vec!["eth0".to_string()],
        show_temperatures: false,
        show_pseudo_filesystems: true,
        show_load_average: false,
    };

    original
        .save(&path)
        .expect("save must create parent directories");
    let (loaded, error, _notes) = Config::load(&path);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(loaded, original);
}

#[test]
fn saving_is_atomic() {
    // The temporary file used during the write must not survive.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    Config::default().save(&path).expect("save");

    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .expect("read dir")
        .filter_map(|entry| {
            entry
                .ok()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
        })
        .collect();
    assert_eq!(
        leftovers,
        vec!["config.toml".to_string()],
        "unexpected files left behind: {leftovers:?}"
    );
}

#[test]
fn validation_removes_blank_interface_names() {
    let mut config = Config {
        network_interfaces: vec!["".to_string(), "   ".to_string(), "eth0".to_string()],
        ..Config::default()
    };
    config.validate();
    assert_eq!(config.network_interfaces, vec!["eth0".to_string()]);
}

#[test]
fn the_interface_list_is_capped() {
    let many: Vec<String> = (0..64).map(|index| format!("eth{index}")).collect();
    let mut config = Config {
        network_interfaces: many,
        ..Config::default()
    };
    config.validate();
    assert!(
        config.network_interfaces.len() <= 16,
        "got {}",
        config.network_interfaces.len()
    );
}

#[test]
fn a_non_finite_interval_is_reset() {
    let mut config = Config {
        refresh_interval: f64::INFINITY,
        ..Config::default()
    };
    config.validate();
    assert!(config.interval() >= std::time::Duration::from_millis(MIN_INTERVAL_MS));
    assert!(config.refresh_interval.is_finite());
}

#[test]
fn the_default_path_ends_in_the_documented_location() {
    let path = Config::path_from_env();
    let text = path.to_string_lossy();
    assert!(
        text.contains("syswatch") || text.contains("config.toml"),
        "unexpected path {text}"
    );
}
