//! Process manager: the sortable, searchable process table.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use super::theme::Theme;
use super::widgets;
use crate::app::{App, Focus};
use crate::collector::Snapshot;
use crate::collector::processes::ProcessInfo;
use crate::format;

/// How many rows the footer summary occupies.
const FOOTER_HEIGHT: u16 = 1;

/// Draws the process table into `area`.
pub fn draw(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let [table_area, footer_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(FOOTER_HEIGHT)]).areas(area);

    // Record how many rows fit before borrowing the snapshot, so PageUp/PageDown move
    // by a full screen. One line goes to the column header.
    const HEADER_ROWS: u16 = 1;
    app.visible_rows = usize::from(table_area.height.saturating_sub(HEADER_ROWS).max(1));

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

    let visible = app.visible_processes();
    let cores = snapshot.cpu.logical_cores.max(1);
    let selected = if app.selected_process < visible.len() {
        app.selected_process
    } else {
        0
    };
    let focused = app.focus == Focus::ProcessRow;

    let title = format!(
        "Processes ({}/{})",
        visible.len(),
        snapshot.processes.processes.len()
    );
    const HINT: &str = "↑↓ move · Enter details · k kill · c/m/p/n sort · f filter · / search";
    let block = widgets::panel_with_hint(&title, HINT, theme, focused);
    let table = Table::new(
        visible
            .iter()
            .map(|process| row(process, cores, snapshot, theme)),
        [
            Constraint::Length(7),
            Constraint::Min(12),
            Constraint::Length(7),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(3),
        ],
    )
    .header(header_row(app, theme))
    .column_spacing(1)
    .row_highlight_style(if focused {
        theme.selection()
    } else {
        Style::default()
    })
    .highlight_symbol(if focused { "▶ " } else { "  " })
    .flex(ratatui::layout::Flex::Start)
    .block(block);

    let mut state = TableState::default().with_selected(Some(selected));
    frame.render_stateful_widget(table, table_area, &mut state);

    frame.render_widget(footer(app, &visible, snapshot, theme), footer_area);
}

/// Builds the process row.
fn row<'a>(process: &ProcessInfo, cores: usize, snapshot: &Snapshot, theme: &Theme) -> Row<'a> {
    // CPU usage from sysinfo is relative to a single core; dividing by the core count
    // gives the percentage of the whole machine, which is what the header states.
    let cpu = (f64::from(process.cpu_usage) / cores as f64).clamp(0.0, 100.0);
    let memory_percent = if snapshot.memory.total == 0 {
        0.0
    } else {
        process.memory as f64 / snapshot.memory.total as f64 * 100.0
    };

    let cpu_style = Style::default().fg(theme.usage_color(cpu));
    let mem_style = Style::default().fg(theme.usage_color(memory_percent));

    Row::new(vec![
        Cell::from(process.pid.as_u32().to_string()),
        Cell::from(Span::raw(process.name.clone())),
        Cell::from(Span::styled(format::percent(cpu), cpu_style)),
        Cell::from(Span::styled(format::bytes(process.memory), mem_style)),
        Cell::from(process.user.clone()),
        Cell::from(Span::styled(
            process.state.chars().next().unwrap_or('?').to_string(),
            Style::default().fg(state_color(
                process.state.as_bytes().first().copied().unwrap_or(b'?'),
                theme,
            )),
        )),
    ])
}

/// Colour for a process state letter.
fn state_color(state: u8, theme: &Theme) -> ratatui::style::Color {
    match state {
        b'R' => theme.critical,
        b'S' => theme.dim,
        b'D' | b'Z' | b'T' | b't' => theme.warn,
        b'I' => theme.accent,
        _ => theme.text,
    }
}

/// The column header, with the active sort column marked.
fn header_row<'a>(app: &App, theme: &Theme) -> Row<'a> {
    let marker = |key: crate::collector::processes::SortKey, label: &'a str| {
        let active = app.sort_key == key;
        let arrow = if active {
            if app.sort_descending { "▼" } else { "▲" }
        } else {
            " "
        };
        let style = if active {
            Style::default()
                .fg(theme.title_active)
                .add_modifier(Modifier::BOLD)
        } else {
            theme.dim()
        };
        Span::styled(format!("{label}{arrow}"), style)
    };

    Row::new(vec![
        Cell::from(marker(crate::collector::processes::SortKey::Pid, "PID")),
        Cell::from(marker(crate::collector::processes::SortKey::Name, "NAME")),
        Cell::from(marker(crate::collector::processes::SortKey::Cpu, "CPU%")),
        Cell::from(marker(
            crate::collector::processes::SortKey::Memory,
            "MEMORY",
        )),
        Cell::from("USER"),
        Cell::from("S"),
    ])
    .style(theme.dim())
}

/// A one line summary under the table: details of the selected process, plus a count
/// of the processes that exited since the previous refresh.
fn footer<'a>(
    app: &App,
    visible: &[ProcessInfo],
    snapshot: &Snapshot,
    theme: &Theme,
) -> Paragraph<'a> {
    let selected = visible.get(app.selected_process.min(visible.len().saturating_sub(1)));
    let description = match selected {
        Some(process) => {
            let command = process
                .cmd
                .clone()
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string());
            let parent = process
                .parent
                .map(|pid| pid.as_u32().to_string())
                .unwrap_or_else(|| "-".to_string());
            format!(
                "#{} {} · {} · parent {parent} · {}",
                process.pid.as_u32(),
                process.name,
                command,
                format::bytes(process.memory)
            )
        }
        None => format!("{} processes, none selected", visible.len()),
    };
    let vanished = snapshot.processes.vanished;
    let note = if vanished > 0 {
        format!("{vanished} exited since last refresh")
    } else {
        String::new()
    };
    Paragraph::new(Line::from(vec![
        Span::styled(format::truncate(&description, 80), theme.text()),
        Span::styled(format!("  {note}"), theme.dim()),
    ]))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::collector::processes::ProcessList;
    use crate::config::Config;
    use crate::ui::tests_support::render;

    fn process(pid: u32, name: &str, cpu: f32, memory: u64) -> ProcessInfo {
        ProcessInfo {
            pid: sysinfo::Pid::from_u32(pid),
            name: name.to_string(),
            cmd: Some(format!("/usr/bin/{name} --flag")),
            exe: Some(PathBuf::from(format!("/usr/bin/{name}"))),
            user: "tester".to_string(),
            uid: Some(1000),
            state: "S".to_string(),
            cpu_usage: cpu,
            memory,
            parent: Some(sysinfo::Pid::from_u32(1)),
            threads: Some(2),
        }
    }

    fn snapshot_with_processes() -> Snapshot {
        let mut snapshot = Snapshot::default();
        snapshot.cpu.logical_cores = 4;
        snapshot.memory.total = 8 * 1024 * 1024 * 1024;
        snapshot.processes = ProcessList {
            processes: vec![
                process(1, "systemd", 5.0, 1024 * 1024),
                process(2, "bash", 50.0, 4 * 1024 * 1024),
                process(3, "firefox", 200.0, 2 * 1024 * 1024),
            ],
            vanished: 2,
        };
        snapshot
    }

    fn app_with_processes() -> App {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Processes;
        app.update(snapshot_with_processes());
        app
    }

    #[test]
    fn renders_the_table_with_headers() {
        let mut app = app_with_processes();
        let output = render(&mut app, 140, 40);
        assert!(output.contains("PID"), "got: {output}");
        assert!(output.contains("NAME"), "got: {output}");
        assert!(output.contains("CPU%"), "got: {output}");
        assert!(output.contains("MEMORY"), "got: {output}");
        assert!(output.contains("USER"), "got: {output}");
    }

    #[test]
    fn sorts_by_cpu_by_default() {
        let app = app_with_processes();
        let visible = app.visible_processes();
        assert_eq!(visible[0].pid.as_u32(), 3);
    }

    #[test]
    fn search_filters_the_table() {
        let mut app = app_with_processes();
        app.search.text = "bash".to_string();
        let output = render(&mut app, 140, 40);
        assert!(output.contains("bash"), "got: {output}");
        assert!(
            !output.contains("firefox"),
            "search should hide other rows: {output}"
        );
    }

    #[test]
    fn footer_shows_the_selected_process() {
        let mut app = app_with_processes();
        let output = render(&mut app, 160, 40);
        // The footer repeats the command line of the selected row, which is selected
        // automatically as the highest CPU consumer.
        assert!(output.contains("/usr/bin/firefox"), "got: {output}");
    }

    #[test]
    fn footer_follows_the_selection() {
        let mut app = app_with_processes();
        app.move_selection(1);
        let output = render(&mut app, 160, 40);
        assert!(output.contains("/usr/bin/bash"), "got: {output}");
    }

    #[test]
    fn vanished_processes_are_reported() {
        let mut app = app_with_processes();
        let output = render(&mut app, 160, 40);
        assert!(output.contains("exited"), "got: {output}");
    }

    #[test]
    fn narrow_terminal_does_not_panic() {
        let mut app = app_with_processes();
        let _ = render(&mut app, 45, 14);
    }

    #[test]
    fn empty_process_list_is_handled() {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Processes;
        app.update(Snapshot::default());
        let output = render(&mut app, 120, 30);
        assert!(
            output.contains("none selected") || output.contains("Processes"),
            "got: {output}"
        );
    }
}
