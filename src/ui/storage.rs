//! Storage dashboard: mounted filesystems with capacity bars and a kind column.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use super::theme::Theme;
use super::widgets;
use crate::app::{App, Focus};
use crate::collector::storage::{FilesystemInfo, FilesystemKind};
use crate::format;

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
    let [table_area, legend] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    let focused = app.focus == Focus::StorageRow;

    let title = format!("Filesystems ({})", filesystems.len());
    const HINT: &str = "physical disks first · pseudo and network mounts are marked";
    let block = widgets::panel_with_hint(&title, HINT, theme, focused);
    let table = Table::new(
        filesystems.iter().map(|fs| row(fs, theme)),
        [
            Constraint::Min(16),
            Constraint::Min(10),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(11),
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

/// Builds a filesystem row, including a text bar for the used percentage.
fn row<'a>(filesystem: &FilesystemInfo, theme: &Theme) -> Row<'a> {
    let percent = filesystem.used_percent();
    let bar_width = 10usize;
    let filled = ((percent / 100.0) * bar_width as f64).round() as usize;
    let filled = filled.min(bar_width);
    let bar: String = "▇".repeat(filled) + &" ".repeat(bar_width - filled);

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
        Cell::from(Span::raw(format::truncate_middle(&filesystem.name, 24))),
        Cell::from(filesystem.file_system.clone()),
        Cell::from(Span::styled(
            filesystem.kind.label().to_string(),
            Style::default().fg(kind_color(filesystem.kind, theme)),
        )),
        Cell::from(format::bytes(filesystem.total)),
        Cell::from(Span::styled(
            format!("{bar} {:>3.0}%", percent),
            Style::default().fg(theme.usage_color(percent)),
        )),
        Cell::from(format::bytes(filesystem.available)),
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
    fn narrow_terminal_does_not_panic() {
        let mut app = app_with_filesystems();
        let _ = render(&mut app, 45, 14);
    }
}
