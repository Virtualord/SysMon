//! Storage dashboard: the physical disk inventory and the mounted filesystems.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use super::theme::Theme;
use super::widgets;
use crate::app::{App, Focus};
use crate::collector::Snapshot;
use crate::collector::disks::{DiskDevice, SensorReading};
use crate::collector::storage::{FilesystemInfo, FilesystemKind};
use crate::format;

/// Rows a bordered table spends on its frame.
const BORDER_ROWS: u16 = 2;
/// Rows a table with a header spends on that header.
const HEADER_ROWS: u16 = 1;

/// Draws the storage panel into `area`.
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

    let filesystems = &snapshot.storage;

    // The disk inventory sits above the mount table when it is enabled and the kernel
    // reported something. Both are two views of the same hardware, so they share the
    // storage tab rather than earning a seventh view of their own.
    let show_disks = app.config.show_disk_health && !snapshot.disks.is_empty();
    let disk_rows = if show_disks {
        snapshot.disks.len() as u16 + HEADER_ROWS + BORDER_ROWS
    } else {
        0
    };

    let [disk_area, table_area, legend] = Layout::vertical([
        Constraint::Length(disk_rows),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    if show_disks {
        draw_disks(frame, app, disk_area, theme, snapshot);
    }

    let focused = app.focus == Focus::StorageRow;

    let title = format!("Filesystems ({})", filesystems.len());
    const HINT: &str = "physical disks first · pseudo and network mounts are marked";
    let block = widgets::panel_with_hint(&title, HINT, theme, focused);
    // The widths are chosen so the whole table fits in 120 columns, which is the
    // narrowest terminal where the used-bar is still readable. Anything wider than
    // that gives the surplus to the mount point, which is the column that benefits.
    let table = Table::new(
        filesystems.iter().map(|fs| row(fs, theme)),
        [
            Constraint::Min(12),
            Constraint::Length(18),
            Constraint::Length(6),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(16),
            Constraint::Length(9),
        ],
    )
    .header(
        Row::new(vec![
            Cell::from("MOUNT POINT"),
            Cell::from("DEVICE"),
            Cell::from("TYPE"),
            Cell::from("KIND"),
            Cell::from("TOTAL"),
            Cell::from("USED"),
            Cell::from("AVAILABLE"),
        ])
        .style(theme.dim()),
    )
    .column_spacing(1)
    .row_highlight_style(if focused {
        theme.selection()
    } else {
        Style::default()
    })
    .highlight_symbol(if focused { "▶ " } else { "  " })
    .block(block);

    let mut state = TableState::default().with_selected(Some(
        app.selected_process
            .min(filesystems.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, table_area, &mut state);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Kind: physical = real disk · virtual = loop/device-mapper/VM · network = remote · pseudo = kernel",
            theme.dim(),
        ))),
        legend,
    );
}

/// Draws the physical disk inventory: model, firmware, bus, capacity and temperature.
///
/// The critical threshold is the column that earns its place: it is the point at which
/// the drive throttles or shuts itself down to protect the data, and unlike the wear
/// percentage it is published by the kernel and readable without root.
fn draw_disks(frame: &mut Frame, app: &App, area: Rect, theme: &Theme, snapshot: &Snapshot) {
    let disks = &snapshot.disks;
    let title = format!("Disks ({})", disks.len());
    const HINT: &str =
        "temps from the kernel · wear % needs `nvme smart-log`, see scripts/disk-health.sh";
    let block = widgets::panel_with_hint(&title, HINT, theme, false);

    let table = Table::new(
        disks.iter().map(|disk| disk_row(disk, theme)),
        [
            Constraint::Min(10),
            Constraint::Length(9),
            Constraint::Min(16),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(9),
            Constraint::Length(9),
        ],
    )
    .header(
        Row::new(vec![
            Cell::from("DEVICE"),
            Cell::from("TYPE"),
            Cell::from("MODEL"),
            Cell::from("FIRMWARE"),
            Cell::from("BUS"),
            Cell::from("CAPACITY"),
            Cell::from("TEMP"),
        ])
        .style(theme.dim()),
    )
    .column_spacing(1)
    .block(block);

    let mut state = TableState::default().with_selected(Some(
        app.selected_process.min(disks.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, area, &mut state);
}

/// One disk row.
///
/// The temperature is colour coded against the critical threshold where the drive
/// published one, and against the usual 70 °C consumer ceiling otherwise, so a drive
/// heading for trouble is visible without reading the number closely.
fn disk_row<'a>(disk: &DiskDevice, theme: &Theme) -> Row<'a> {
    let model = disk
        .model
        .clone()
        .unwrap_or_else(|| format::NOT_AVAILABLE.to_string());
    let type_style = Style::default().fg(if disk.is_ssd() {
        theme.ok
    } else {
        theme.secondary
    });
    let firmware = disk.firmware.clone().unwrap_or_else(|| "-".to_string());

    Row::new(vec![
        Cell::from(Span::raw(disk.name.clone())),
        Cell::from(Span::styled(disk.kind_label(), type_style)),
        Cell::from(Span::raw(model)),
        Cell::from(Span::raw(firmware)),
        Cell::from(Span::raw(
            disk.transport.clone().unwrap_or_else(|| "-".to_string()),
        )),
        Cell::from(Span::raw(format::bytes(disk.size_bytes))),
        Cell::from(temperature_cell(disk.primary_temperature(), theme)),
    ])
}

/// The temperature column, with the critical threshold beside it when known.
fn temperature_cell<'a>(reading: Option<&SensorReading>, theme: &Theme) -> Span<'a> {
    let Some(reading) = reading else {
        return Span::raw("-");
    };
    let ratio = reading
        .critical_celsius
        .filter(|critical| *critical > 0.0)
        .map(|critical| reading.celsius / critical)
        .unwrap_or_else(|| reading.celsius / 70.0)
        .clamp(0.0, 1.0);
    let text = match reading.critical_celsius {
        Some(critical) => format!("{:.1}°/{:.0}°", reading.celsius, critical),
        None => format!("{:.1}°", reading.celsius),
    };
    Span::styled(text, Style::default().fg(theme.usage_color(ratio * 100.0)))
}

/// Builds a filesystem row, including a text bar for the used percentage.
fn row<'a>(filesystem: &FilesystemInfo, theme: &Theme) -> Row<'a> {
    let percent = filesystem.used_percent();
    // Five blocks plus the percentage: 16 columns including the separating space.
    const BAR_WIDTH: usize = 5;
    let filled = ((percent / 100.0) * BAR_WIDTH as f64).round() as usize;
    let filled = filled.min(BAR_WIDTH);
    let bar: String = "▇".repeat(filled) + &"·".repeat(BAR_WIDTH - filled);

    let name_style = if filesystem.read_only {
        Style::default()
            .fg(theme.dim)
            .add_modifier(Modifier::ITALIC)
    } else {
        theme.text()
    };

    Row::new(vec![
        Cell::from(Span::styled(
            filesystem.mount_point.display().to_string(),
            name_style,
        )),
        Cell::from(Span::raw(format::truncate_middle(&filesystem.name, 17))),
        Cell::from(Span::raw(format::truncate(&filesystem.file_system, 5))),
        Cell::from(Span::styled(
            filesystem.kind.label().to_string(),
            Style::default().fg(kind_color(filesystem.kind, theme)),
        )),
        Cell::from(Span::raw(format::bytes(filesystem.total))),
        Cell::from(Span::styled(
            format!("{bar} {percent:>3.0}%"),
            Style::default().fg(theme.usage_color(percent)),
        )),
        Cell::from(Span::raw(format::bytes(filesystem.available))),
    ])
}

/// Colour that distinguishes the filesystem kinds.
fn kind_color(kind: FilesystemKind, theme: &Theme) -> ratatui::style::Color {
    match kind {
        FilesystemKind::Physical => theme.ok,
        FilesystemKind::Virtual => theme.primary,
        FilesystemKind::Network => theme.secondary,
        FilesystemKind::Pseudo => theme.dim,
        FilesystemKind::Unknown => theme.warn,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::collector::Snapshot;
    use crate::config::Config;
    use crate::ui::tests_support::render;

    fn filesystem(device: &str, mount: &str, kind: FilesystemKind, total: u64) -> FilesystemInfo {
        FilesystemInfo {
            name: device.to_string(),
            mount_point: PathBuf::from(mount),
            file_system: "ext4".to_string(),
            kind,
            total,
            used: total / 2,
            available: total / 2,
            read_only: false,
        }
    }

    fn storage_snapshot() -> Snapshot {
        Snapshot {
            storage: vec![
                filesystem(
                    "/dev/nvme0n1p2",
                    "/",
                    FilesystemKind::Physical,
                    500 * 1024 * 1024 * 1024,
                ),
                filesystem(
                    "server:/export",
                    "/mnt/net",
                    FilesystemKind::Network,
                    1024 * 1024 * 1024 * 1024,
                ),
                filesystem(
                    "tmpfs",
                    "/run",
                    FilesystemKind::Pseudo,
                    16 * 1024 * 1024 * 1024,
                ),
            ],
            ..Snapshot::default()
        }
    }

    /// A disk with a temperature and a critical threshold, as the kernel reports one.
    fn disk(name: &str, model: &str, celsius: f64, critical: Option<f64>) -> DiskDevice {
        DiskDevice {
            name: name.to_string(),
            model: Some(model.to_string()),
            firmware: Some("G001".to_string()),
            serial: Some("BTTE908".to_string()),
            transport: Some("nvme".to_string()),
            rotational: false,
            size_bytes: 1000 * 1024 * 1024 * 1024,
            state: Some("live".to_string()),
            temperatures: vec![SensorReading {
                label: Some("Composite".to_string()),
                celsius,
                max_celsius: Some(70.0),
                critical_celsius: critical,
            }],
        }
    }

    /// An app on the storage view carrying both filesystems and disks.
    fn app_with_disks() -> App {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Storage;
        let mut snapshot = storage_snapshot();
        snapshot.disks = vec![
            disk("nvme0n1", "INTEL HBRPEKNX0202A", 30.9, Some(80.0)),
            disk("nvme1n1", "SAMSUNG SSD 990", 41.8, None),
        ];
        app.update(snapshot);
        app
    }

    fn app_with_filesystems() -> App {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Storage;
        app.update(storage_snapshot());
        app
    }

    #[test]
    fn renders_the_filesystem_table() {
        let mut app = app_with_filesystems();
        let output = render(&mut app, 150, 40);
        assert!(output.contains("MOUNT POINT"), "got: {output}");
        assert!(output.contains("/mnt/net"), "got: {output}");
    }

    #[test]
    fn the_whole_table_fits_a_120_column_terminal() {
        // Every column must be readable at the documented minimum width; a truncated
        // percentage is worse than a narrower mount column.
        let mut app = app_with_filesystems();
        let output = render(&mut app, 120, 40);
        for header in ["TYPE", "KIND", "TOTAL", "USED", "AVAILABLE"] {
            assert!(
                output.contains(header),
                "header {header:?} was cut: {output}"
            );
        }
        assert!(
            output.contains("AVAIL"),
            "the header must not be cut: {output}"
        );
        assert!(
            output.contains("50%"),
            "the used percentage must be readable: {output}"
        );
        // 500 GiB and 1 TiB in the fixture, formatted by the shared byte helper.
        assert!(
            output.contains("500.0 GiB"),
            "the total must be readable: {output}"
        );
        assert!(
            output.contains("1.0 TiB"),
            "the total must be readable: {output}"
        );
    }

    #[test]
    fn shows_the_filesystem_kind() {
        let mut app = app_with_filesystems();
        app.config.show_pseudo_filesystems = true;
        let output = render(&mut app, 150, 40);
        assert!(output.contains("physical"), "got: {output}");
        assert!(output.contains("network"), "got: {output}");
    }

    #[test]
    fn empty_storage_is_handled() {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Storage;
        app.update(Snapshot::default());
        let output = render(&mut app, 120, 30);
        assert!(output.contains("Filesystems (0)"), "got: {output}");
    }

    #[test]
    fn renders_the_disk_inventory_above_the_filesystems() {
        let mut app = app_with_disks();
        let output = render(&mut app, 150, 45);

        assert!(output.contains("Disks (2)"), "got: {output}");
        assert!(output.contains("DEVICE"), "got: {output}");
        assert!(output.contains("nvme0n1"), "got: {output}");
        assert!(output.contains("INTEL HBRPEKNX0202A"), "got: {output}");
        assert!(
            output.contains("G001"),
            "the firmware must be shown: {output}"
        );
        assert!(
            output.contains("SSD"),
            "the drive type must be shown: {output}"
        );
        // Both tables have to fit on one screen.
        assert!(
            output.contains("Filesystems (3)"),
            "the mount table must still be there: {output}"
        );
    }

    #[test]
    fn shows_the_temperature_and_its_critical_threshold() {
        let mut app = app_with_disks();
        let output = render(&mut app, 150, 45);
        // The drive that published a threshold shows "current/threshold".
        assert!(output.contains("30.9°/80°"), "got: {output}");
        // The one that did not shows the temperature alone.
        assert!(output.contains("41.8°"), "got: {output}");
    }

    #[test]
    fn the_disk_table_is_hidden_when_disabled() {
        let mut app = app_with_disks();
        app.config.show_disk_health = false;
        let output = render(&mut app, 150, 45);
        assert!(
            !output.contains("Disks ("),
            "the inventory must be hidden: {output}"
        );
        assert!(output.contains("Filesystems (3)"), "got: {output}");
    }

    #[test]
    fn no_disks_is_not_an_error() {
        let mut app = app_with_filesystems();
        let output = render(&mut app, 150, 40);
        assert!(!output.contains("Disks ("), "got: {output}");
        assert!(output.contains("Filesystems (3)"), "got: {output}");
    }

    #[test]
    fn a_drive_without_a_sensor_shows_a_dash() {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Storage;
        let mut bare = disk("sda", "OLD SPINNING DISK", 40.0, None);
        bare.rotational = true;
        bare.temperatures.clear();
        bare.transport = Some("ata".to_string());
        app.update(Snapshot {
            disks: vec![bare],
            ..Snapshot::default()
        });

        let output = render(&mut app, 150, 40);
        assert!(
            output.contains("HDD"),
            "a spinning disk must be labelled: {output}"
        );
        assert!(output.contains("OLD SPINNING DISK"), "got: {output}");
        assert!(
            !output.contains("40.0°"),
            "a drive with no sensor has no temperature: {output}"
        );
    }

    #[test]
    fn the_disk_table_fits_a_120_column_terminal() {
        let mut app = app_with_disks();
        let output = render(&mut app, 120, 45);
        for header in [
            "DEVICE", "TYPE", "MODEL", "FIRMWARE", "BUS", "CAPACITY", "TEMP",
        ] {
            assert!(
                output.contains(header),
                "header {header:?} was cut: {output}"
            );
        }
    }

    #[test]
    fn narrow_terminal_does_not_panic() {
        let mut app = app_with_filesystems();
        let _ = render(&mut app, 45, 14);
    }

    #[test]
    fn long_device_names_are_truncated_in_the_middle() {
        // /dev/mapper/cryptroot is a real device name; the column must not push the
        // percentages off the right edge.
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Storage;
        app.update(Snapshot {
            storage: vec![filesystem(
                "/dev/mapper/cryptroot-very-long-name",
                "/",
                FilesystemKind::Virtual,
                500 * 1024 * 1024 * 1024,
            )],
            ..Snapshot::default()
        });
        let output = render(&mut app, 120, 40);
        assert!(
            output.contains("50%"),
            "the percentage must survive: {output}"
        );
        assert!(
            !output.contains("very-long-name"),
            "the name must be shortened: {output}"
        );
    }

    #[test]
    fn read_only_mounts_are_marked() {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Storage;
        let mut read_only = filesystem(
            "/dev/sr0",
            "/mnt/cdrom",
            FilesystemKind::Physical,
            700 * 1024 * 1024,
        );
        read_only.read_only = true;
        app.update(Snapshot {
            storage: vec![read_only],
            ..Snapshot::default()
        });
        // The row must still render; the italic style is what distinguishes it.
        let output = render(&mut app, 120, 40);
        assert!(output.contains("/mnt/cdrom"), "got: {output}");
    }
}
