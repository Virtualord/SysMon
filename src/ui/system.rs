//! System information dashboard: distribution, kernel, hardware and sensors.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use super::theme::Theme;
use super::widgets;
use crate::app::App;
use crate::collector::system::{DynamicInfo, SystemInfo, Temperature};
use crate::format;

/// Draws the system panel into `area`.
pub fn draw(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(snapshot) = app.snapshot() else {
        frame.render_widget(
            widgets::key_values(
                &[("status", "waiting for the first sample".to_string())],
                theme,
            ),
            area,
        );
        return;
    };

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);
    draw_os(frame, left, theme, &snapshot.system, &snapshot.dynamic);
    draw_hardware(
        frame,
        right,
        theme,
        &snapshot.system,
        &snapshot.dynamic,
        app,
    );
}

/// Distribution, kernel, host and uptime.
fn draw_os(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    system: &SystemInfo,
    dynamic: &DynamicInfo,
) {
    let block = widgets::panel("Operating system", theme, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let pairs = vec![
        (
            "Distribution",
            system
                .distribution
                .clone()
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
        (
            "ID",
            system
                .distribution_id
                .clone()
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
        ("Kernel", format::truncate(&system.kernel, 48)),
        ("Architecture", system.architecture.clone()),
        ("Hostname", system.hostname.clone()),
        ("Uptime", format::duration(dynamic.uptime)),
        (
            "Load average",
            if dynamic.load_available() == 0 {
                format::NOT_AVAILABLE.to_string()
            } else {
                format!(
                    "{}  {}  {}",
                    format::load_average(dynamic.load_one),
                    format::load_average(dynamic.load_five),
                    format::load_average(dynamic.load_fifteen)
                )
            },
        ),
        (
            "Tasks",
            match (dynamic.running_tasks, dynamic.total_tasks) {
                (Some(running), Some(total)) => format!("{running} running / {total} total"),
                (Some(running), None) => format!("{running} running"),
                _ => format::NOT_AVAILABLE.to_string(),
            },
        ),
    ];
    frame.render_widget(widgets::key_values(&pairs, theme), inner);
}

/// Hardware inventory and the sensor table.
fn draw_hardware(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    system: &SystemInfo,
    dynamic: &DynamicInfo,
    app: &App,
) {
    let [top, sensors] = Layout::vertical([Constraint::Length(9), Constraint::Min(0)]).areas(area);

    let block = widgets::panel("Hardware", theme, false);
    let inner = block.inner(top);
    frame.render_widget(block, top);
    let pairs = vec![
        (
            "CPU",
            system
                .cpu_brand
                .clone()
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
        (
            "Cores",
            match system.physical_cores {
                Some(physical) => format!("{physical} physical / {} logical", system.logical_cores),
                None => format!("{} logical", system.logical_cores),
            },
        ),
        ("Memory", format::bytes(system.total_memory)),
        ("Boot time", unix_to_local(system.boot_time)),
    ];
    frame.render_widget(widgets::key_values(&pairs, theme), inner);

    if !app.config.show_temperatures {
        return;
    }

    let block = widgets::panel("Sensors", theme, false);
    let inner = block.inner(sensors);
    frame.render_widget(block, sensors);

    if dynamic.temperatures.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no temperature sensors exposed by the kernel",
                theme.dim(),
            ))),
            inner,
        );
        return;
    }

    let table = Table::new(
        dynamic
            .temperatures
            .iter()
            .map(|temperature| temperature_row(temperature, theme)),
        [
            Constraint::Min(16),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(10),
        ],
    )
    .header(Row::new(vec![
        Cell::from("SENSOR"),
        Cell::from("CURRENT"),
        Cell::from("MAX"),
        Cell::from("CRITICAL"),
    ]))
    .column_spacing(1)
    .row_highlight_style(theme.selection())
    .highlight_symbol("▶ ");
    let mut state = TableState::default().with_selected(Some(
        app.selected_process
            .min(dynamic.temperatures.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, inner, &mut state);
}

/// One sensor row, colour coded by proximity to the reported maximum.
fn temperature_row<'a>(temperature: &Temperature, theme: &Theme) -> Row<'a> {
    let ratio = temperature
        .max_celsius
        .filter(|max| *max > 0.0)
        .map(|max| f64::from(temperature.celsius) / f64::from(max))
        .unwrap_or(0.0);
    let style = Style::default().fg(theme.usage_color(ratio * 100.0));
    Row::new(vec![
        Cell::from(format::truncate(&temperature.label, 24)),
        Cell::from(Span::styled(
            format!("{:.1} °C", temperature.celsius),
            style,
        )),
        Cell::from(
            temperature
                .max_celsius
                .map(|max| format!("{max:.0} °C"))
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
        Cell::from(
            temperature
                .critical_celsius
                .map(|max| format!("{max:.0} °C"))
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
    ])
}

/// Formats a UNIX timestamp as an ISO-like date without a date-time dependency.
///
/// Only the date and time of day are shown; the year offset from the epoch is computed
/// with the civil-from-days algorithm, which is exact for all dates syswatch will ever
/// see.
fn unix_to_local(timestamp: u64) -> String {
    if timestamp == 0 {
        return format::NOT_AVAILABLE.to_string();
    }
    let seconds = timestamp as i64;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

/// Converts a count of days since 1970-01-01 into a civil `(year, month, day)`.
///
/// This is Howard Hinnant's `civil_from_days` algorithm: a shift of the epoch to
/// 0000-03-01 turns the leap-year rules into a simple 146 097 day cycle.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::Snapshot;
    use crate::config::Config;
    use crate::ui::tests_support::render;

    fn system_snapshot() -> Snapshot {
        Snapshot {
            system: SystemInfo {
                hostname: "testhost".to_string(),
                distribution: Some("Void Linux 20250202".to_string()),
                distribution_id: Some("void".to_string()),
                kernel: "Linux 6.9.1-test".to_string(),
                architecture: "x86_64".to_string(),
                cpu_brand: Some("Test CPU".to_string()),
                physical_cores: Some(8),
                logical_cores: 16,
                total_memory: 32 * 1024 * 1024 * 1024,
                boot_time: 1_700_000_000,
            },
            dynamic: DynamicInfo {
                uptime: 93_600,
                load_one: 0.5,
                load_five: 0.4,
                load_fifteen: 0.3,
                running_tasks: Some(3),
                total_tasks: Some(420),
                temperatures: vec![Temperature {
                    label: "Package id 0".to_string(),
                    celsius: 48.0,
                    max_celsius: Some(100.0),
                    critical_celsius: Some(100.0),
                }],
            },
            ..Snapshot::default()
        }
    }

    fn app_with_system() -> App {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::System;
        app.update(system_snapshot());
        app
    }

    #[test]
    fn shows_distribution_and_kernel() {
        let mut app = app_with_system();
        let output = render(&mut app, 140, 40);
        assert!(output.contains("Void Linux"), "got: {output}");
        assert!(output.contains("6.9.1-test"), "got: {output}");
        assert!(output.contains("testhost"), "got: {output}");
    }

    #[test]
    fn shows_hardware_and_sensors() {
        let mut app = app_with_system();
        let output = render(&mut app, 160, 40);
        assert!(output.contains("Test CPU"), "got: {output}");
        assert!(output.contains("Package id 0"), "got: {output}");
    }

    #[test]
    fn reports_missing_sensors() {
        let mut app = app_with_system();
        let mut snapshot = app.snapshot().cloned().expect("snapshot");
        snapshot.dynamic.temperatures.clear();
        app.update(snapshot);
        let output = render(&mut app, 160, 40);
        assert!(output.contains("no temperature sensors"), "got: {output}");
    }

    #[test]
    fn temperatures_can_be_hidden() {
        let mut app = app_with_system();
        app.config.show_temperatures = false;
        let output = render(&mut app, 160, 40);
        assert!(!output.contains("Package id 0"), "got: {output}");
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        // 2024 is a leap year, so day 59 of the year is 29 February.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn boot_time_is_formatted() {
        assert_eq!(unix_to_local(0), format::NOT_AVAILABLE);
        assert!(unix_to_local(1_700_000_000).starts_with("2023-11-14 "));
    }
}
