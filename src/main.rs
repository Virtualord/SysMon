//! syswatch — a fast, lightweight native terminal system monitor for Linux.
//!
//! This binary is a thin shell around the [`syswatch`] library: it parses the command
//! line, loads the configuration, starts the monitoring thread and runs the event
//! loop. All behaviour lives in the library so it can be tested without a terminal.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Parser;
use syswatch::app::App;
use syswatch::config::{Config, Theme as ThemeSetting};
use syswatch::events::{Monitor, poll_events};
use syswatch::terminal::TerminalGuard;
use syswatch::ui::draw;

/// How long the event loop may block waiting for input before looping again.
///
/// This is not a frame rate: it is the longest the loop will sit still when nothing
/// is happening. The header clock ticks once a second, so a one second ceiling is
/// enough to keep the clock honest while an idle syswatch uses no measurable CPU.
const MAX_BLOCK_INTERVAL: Duration = Duration::from_secs(1);

/// One second: the resolution at which the header clock is displayed.
const CLOCK_TICK: Duration = Duration::from_secs(1);

/// Command line interface.
#[derive(Debug, Parser, Default)]
#[command(
    name = "syswatch",
    version,
    about = "A fast, lightweight native terminal system monitor for Linux",
    long_about = None,
    disable_version_flag = false
)]
struct Cli {
    /// Path to the configuration file.
    #[arg(short = 'c', long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Seconds between samples, overriding the configuration file.
    #[arg(short = 'i', long, value_name = "SECONDS", value_parser = parse_interval)]
    interval: Option<f64>,

    /// Disable all colours.
    #[arg(short = 'n', long)]
    no_color: bool,

    /// Override the colour theme.
    #[arg(short = 't', long, value_name = "THEME")]
    theme: Option<String>,

    /// Override the dashboard shown at start-up.
    #[arg(long, value_name = "VIEW")]
    view: Option<String>,

    /// Print the resolved configuration and exit.
    #[arg(long)]
    dump_config: bool,

    /// Write the resolved configuration to PATH and exit.
    #[arg(long, value_name = "PATH")]
    write_config: Option<PathBuf>,

    /// Increase log verbosity (repeatable): -l INFO, -ll DEBUG, -lll TRACE.
    ///
    /// Logging is only emitted when `SYSWATCH_LOG` is set, so that the terminal is
    /// never polluted by diagnostics during normal use.
    #[arg(short = 'l', long, action = clap::ArgAction::Count)]
    log_level: u8,
}

/// Parses and validates the `--interval` argument.
fn parse_interval(value: &str) -> Result<f64, String> {
    let seconds: f64 = value
        .parse()
        .map_err(|_| format!("`{value}` is not a number"))?;
    if !seconds.is_finite() || seconds < 0.1 || seconds > 3600.0 {
        return Err(format!(
            "interval must be between 0.1 and 3600 seconds, got {seconds}"
        ));
    }
    Ok(seconds)
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.log_level);

    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            // The terminal guard restores the terminal on the way out, so this is
            // printed on a usable terminal.
            eprintln!("syswatch: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Installs the `tracing` subscriber.
///
/// Diagnostics go to stderr and only when `SYSWATCH_LOG` is set: a TUI must not write
/// to stdout, and the user should not have to silence warnings in normal use.
/// `SYSWATCH_LOG` may be a bare flag or a `tracing` filter expression, e.g.
/// `SYSWATCH_LOG=syswatch=debug`.
fn init_tracing(verbosity: u8) {
    use tracing_subscriber::EnvFilter;

    let Ok(spec) = std::env::var("SYSWATCH_LOG") else {
        return;
    };
    let level = match verbosity {
        0 => "info",
        1 => "debug",
        2 => "trace",
        _ => "trace",
    };
    let filter =
        EnvFilter::try_new(spec).unwrap_or_else(|_| EnvFilter::new(format!("syswatch={level}")));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Loads the configuration, applies the command line overrides and starts the app.
fn run(cli: Cli) -> anyhow::Result<()> {
    let path = cli.config.clone().unwrap_or_else(Config::path_from_env);
    let (mut config, error, _notes) = Config::load(&path);

    if let Some(err) = error {
        // A broken configuration is not fatal: the defaults are usable and the user is
        // told exactly what was ignored.
        eprintln!("syswatch: {err}");
    }

    if let Some(interval) = cli.interval {
        config.refresh_interval = interval;
    }
    if let Some(theme) = &cli.theme {
        config.theme = ThemeSetting::parse(theme)?;
    }
    if let Some(view) = &cli.view {
        config.default_view = view
            .parse()
            .map_err(|_| anyhow::anyhow!("unknown view {view:?}"))?;
    }
    if cli.no_color {
        config.theme = ThemeSetting::Monochrome;
    }
    let notes = config.validate();

    if cli.dump_config {
        print!("{}", config.to_toml());
        return Ok(());
    }
    if let Some(target) = &cli.write_config {
        config.save(target)?;
        println!("wrote {}", target.display());
        return Ok(());
    }

    let mut app = App::new(config);
    app.color_enabled = !matches!(app.config.theme, ThemeSetting::Monochrome);
    for note in notes {
        tracing::info!("config: {note}");
    }
    // A configuration that was not found is only worth mentioning in the status bar.
    surface_config_notes(&mut app, &path);

    let mut monitor = Monitor::spawn(&app.config);
    let mut guard = TerminalGuard::new()?;

    let result = event_loop(&mut guard, &mut app, &mut monitor);

    // Order matters: the terminal is restored before the collector is stopped, so a
    // slow shutdown never leaves the screen in raw mode.
    guard.restore();
    monitor.shutdown();
    result
}

/// The main loop: redraw only when something changed, otherwise block on input.
///
/// The important property is that a frame is *not* a timer. Rendering is the
/// expensive half of a TUI — building every widget and diffing the buffer — and
/// drawing the same dashboard nine times a second to change nothing is pure waste.
/// A frame is produced when, and only when, one of these is true:
///
/// * a new snapshot arrived,
/// * the user pressed a key,
/// * the terminal was resized,
/// * a status message expired and has to disappear,
/// * the header clock ticked over to the next second.
///
/// Otherwise the loop blocks in `poll` until one of those happens, which costs
/// nothing measurable.
fn event_loop(
    guard: &mut TerminalGuard,
    app: &mut App,
    monitor: &mut Monitor,
) -> anyhow::Result<()> {
    let mut dirty = true;
    let mut frames: u64 = 0;
    let mut next_clock_tick = Instant::now() + CLOCK_TICK;

    loop {
        // 1. Drain whatever the collector produced since the last iteration.
        for message in monitor.drain() {
            match message {
                syswatch::events::MonitorMessage::Snapshot(snapshot) => {
                    app.update(*snapshot);
                    dirty = true;
                }
                syswatch::events::MonitorMessage::Failed(reason) => {
                    app.notify(syswatch::app::StatusMessage::error(reason));
                    dirty = true;
                }
            }
        }
        if app.pending_refresh {
            monitor.request_refresh();
            app.pending_refresh = false;
        }

        // 2. An expiring status message has to be cleared by a redraw.
        if app.expire_message() {
            dirty = true;
        }

        // 3. The header clock only needs a frame when the displayed second changes.
        if Instant::now() >= next_clock_tick {
            next_clock_tick = Instant::now() + CLOCK_TICK;
            dirty = true;
        }

        // 4. Render, but only if something actually changed.
        if dirty {
            let terminal = guard.terminal()?;
            terminal.draw(|frame| draw(frame, app))?;
            dirty = false;
            frames += 1;
            if frames.is_multiple_of(600) {
                tracing::debug!("rendered {frames} frames");
            }
        }

        // 5. Block until there is a reason to draw again.
        if syswatch::terminal::termination_requested() {
            app.should_quit = true;
        }
        match poll_events(app, MAX_BLOCK_INTERVAL) {
            Ok(batch) => {
                if batch.should_quit || app.should_quit {
                    break;
                }
                dirty |= batch.key_pressed || batch.resized;
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {
                // A signal interrupted the poll; loop around and re-check the flag.
                continue;
            }
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

/// Records configuration problems so the user sees them in the status bar.
fn surface_config_notes(app: &mut App, path: &Path) {
    if !path.exists() {
        app.notify(syswatch::app::StatusMessage::info(format!(
            "no config at {} (defaults)",
            path.display()
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_the_documented_flags() {
        let cli = Cli::parse_from([
            "syswatch",
            "--config",
            "/tmp/c.toml",
            "--interval",
            "2.5",
            "--no-color",
        ]);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/c.toml")));
        assert_eq!(cli.interval, Some(2.5));
        assert!(cli.no_color);
    }

    #[test]
    fn defaults_are_empty() {
        let cli = Cli::parse_from(["syswatch"]);
        assert!(cli.config.is_none());
        assert!(cli.interval.is_none());
        assert!(!cli.no_color);
        assert_eq!(cli.log_level, 0);
    }

    #[test]
    fn rejects_out_of_range_intervals() {
        assert!(Cli::try_parse_from(["syswatch", "--interval", "0"]).is_err());
        assert!(Cli::try_parse_from(["syswatch", "--interval", "10000"]).is_err());
        assert!(Cli::try_parse_from(["syswatch", "--interval", "abc"]).is_err());
    }

    #[test]
    fn accepts_the_documented_interval_range() {
        assert!(Cli::try_parse_from(["syswatch", "--interval", "0.1"]).is_ok());
        assert!(Cli::try_parse_from(["syswatch", "--interval", "3600"]).is_ok());
    }

    #[test]
    fn log_level_is_countable() {
        assert_eq!(Cli::parse_from(["syswatch"]).log_level, 0);
        assert_eq!(Cli::parse_from(["syswatch", "-lll"]).log_level, 3);
    }

    #[test]
    fn view_and_theme_overrides_are_accepted() {
        let cli = Cli::parse_from(["syswatch", "--view", "network", "--theme", "light"]);
        assert_eq!(cli.view.as_deref(), Some("network"));
        assert_eq!(cli.theme.as_deref(), Some("light"));
    }

    #[test]
    fn dump_and_write_config_short_circuit() {
        let cli = Cli::parse_from(["syswatch", "--dump-config"]);
        assert!(cli.dump_config);
        let cli = Cli::parse_from(["syswatch", "--write-config", "/tmp/x.toml"]);
        assert_eq!(cli.write_config, Some(PathBuf::from("/tmp/x.toml")));
    }

    #[test]
    fn help_and_version_work() {
        let mut command = Cli::command();
        assert!(command.render_help().to_string().contains("syswatch"));
    }
}
