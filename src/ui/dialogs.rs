//! Modal dialogs: the help overlay, the process detail view and the kill confirmation.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::theme::Theme;
use super::widgets;
use crate::app::{App, Dialog};
use crate::collector::processes::TerminateSignal;
use crate::format;

/// Draws the active dialog over the dashboard.
pub fn draw(frame: &mut Frame, app: &App, theme: &Theme, dialog: Dialog) {
    let area = frame.area();
    match dialog {
        Dialog::Help => {
            let rect = widgets::centered_rect(70, 80, area);
            draw_help(frame, rect, theme);
        }
        Dialog::ProcessDetails => {
            let rect = widgets::centered_rect(80, 80, area);
            draw_details(frame, rect, theme, app);
        }
        Dialog::ConfirmKill { pid, name, signal } => {
            let rect = widgets::centered_rect(60, 30, area);
            draw_confirm_kill(frame, rect, theme, pid, &name, signal);
        }
    }
}

/// The keyboard shortcut reference.
fn draw_help(frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = widgets::panel("Keyboard shortcuts", theme, true);
    let inner = block.inner(area);
    widgets::render_over(frame, block, area);

    #[allow(clippy::type_complexity)]
    let entries: [(&str, &str); 20] = [
        ("q", "quit"),
        ("?", "toggle this help"),
        ("Tab / Shift+Tab", "next / previous dashboard"),
        (
            "1 .. 6",
            "jump to CPU, memory, processes, storage, network, system",
        ),
        ("↑ ↓ / PgUp PgDn", "move the selection"),
        ("Home / End", "first / last row"),
        ("Enter", "details of the selected process"),
        ("/", "search processes"),
        ("Esc", "close the dialog or cancel the search"),
        ("c / m / p / n", "sort by CPU, memory, PID, name"),
        ("s", "reverse the sort order"),
        ("f", "cycle the process filter (all, mine, tasks)"),
        ("i", "cycle the search field (name, pid, all)"),
        ("k", "open the termination dialog"),
        ("y / n", "confirm or cancel a termination"),
        ("k (in dialog)", "switch between SIGTERM and SIGKILL"),
        ("r", "force an immediate refresh"),
        ("g", "toggle Unicode / ASCII graphs"),
        ("Ctrl+C", "quit"),
        ("mouse wheel", "scroll the tables"),
    ];

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner);
    let half = entries.len() / 2 + entries.len() % 2;
    let mut left_lines = Vec::with_capacity(half);
    let mut right_lines = Vec::with_capacity(half);
    for (index, (key, description)) in entries.iter().enumerate() {
        let line = Line::from(vec![
            Span::styled(
                format::pad(key, 17),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(*description, theme.text()),
        ]);
        if index < half {
            left_lines.push(line);
        } else {
            right_lines.push(line);
        }
    }
    left_lines.push(Line::from(""));
    left_lines.push(Line::from(Span::styled(
        "syswatch never kills a process without an explicit confirmation.",
        theme.dim,
    )));
    frame.render_widget(Paragraph::new(left_lines), left);
    frame.render_widget(Paragraph::new(right_lines), right);
}

/// The full detail of the selected process.
fn draw_details(frame: &mut Frame, area: Rect, theme: &Theme, app: &App) {
    let block = widgets::panel("Process details", theme, true);
    let inner = block.inner(area);
    widgets::render_over(frame, block, area);

    let Some(process) = app.selected_process_info() else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "the selected process is gone",
                theme.dim(),
            ))),
            inner,
        );
        return;
    };

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner);
    let identity = [
        ("PID", process.pid.as_u32().to_string()),
        ("Name", process.name.clone()),
        ("User", process.user.clone()),
        (
            "UID",
            process
                .uid
                .map(|uid| uid.to_string())
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
        (
            "PPID",
            process
                .parent
                .map(|pid| pid.as_u32().to_string())
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
        ("State", process.state.clone()),
        (
            "Threads",
            process
                .threads
                .map(|t| t.to_string())
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
    ];
    frame.render_widget(widgets::key_values(&identity, theme), left);

    let usage = [
        ("CPU", format::percent(f64::from(process.cpu_usage))),
        ("Memory", format::bytes(process.memory)),
    ];
    let executable = process
        .exe
        .as_ref()
        .map(|exe| exe.display().to_string())
        .unwrap_or_else(|| format::NOT_AVAILABLE.to_string());
    let mut lines = vec![
        Line::from(Span::styled("Usage", theme.title())),
        Line::from(format::pad("  CPU", 10) + &usage[0].1),
        Line::from(format::pad("  Memory", 10) + &usage[1].1),
        Line::from(""),
        Line::from(Span::styled("Executable", theme.title())),
        Line::from(executable),
        Line::from(""),
        Line::from(Span::styled("Command line", theme.title())),
    ];
    match &process.cmd {
        Some(cmd) if !cmd.is_empty() => lines.push(Line::from(cmd.clone())),
        _ => lines.push(Line::from(Span::styled(format::NOT_AVAILABLE, theme.dim()))),
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), right);
}

/// The termination confirmation dialog.
///
/// The dialog always states which signal will be sent, to which PID, and that this is
/// irreversible for `SIGKILL`. Nothing is sent until the user presses `y` or Enter.
fn draw_confirm_kill(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    pid: u32,
    name: &str,
    signal: TerminateSignal,
) {
    let title = Dialog::ConfirmKill {
        pid,
        name: name.to_string(),
        signal,
    }
    .title();
    let block = widgets::panel(&title, theme, true);
    let inner = block.inner(area);
    widgets::render_over(frame, block, area);

    let warning_style = if signal == TerminateSignal::Kill {
        theme.error()
    } else {
        Style::default().fg(theme.warn).add_modifier(Modifier::BOLD)
    };

    let lines = vec![
        Line::from(Span::styled(
            format!("Send {signal} to process {pid} ({name})?"),
            theme.text(),
        )),
        Line::from(""),
        match signal {
            TerminateSignal::Term => Line::from(Span::styled(
                "SIGTERM asks the process to shut down cleanly. It may ignore the signal.",
                theme.dim(),
            )),
            TerminateSignal::Kill => Line::from(Span::styled(
                "SIGKILL cannot be caught, blocked or ignored.",
                warning_style,
            )),
        },
        Line::from(""),
        Line::from(vec![
            Span::styled(
                " y ",
                Style::default().fg(theme.selection_text).bg(theme.critical),
            ),
            Span::styled(" send signal   ", theme.dim()),
            Span::styled(
                " n ",
                Style::default().fg(theme.selection_text).bg(theme.border),
            ),
            Span::styled(" cancel   ", theme.dim()),
            Span::styled(
                " k ",
                Style::default().fg(theme.selection_text).bg(theme.border),
            ),
            Span::styled(" switch signal", theme.dim()),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::collector::Snapshot;
    use crate::collector::processes::{ProcessInfo, ProcessList};
    use crate::config::Config;
    use crate::ui::tests_support::render;

    fn process() -> ProcessInfo {
        ProcessInfo {
            pid: sysinfo::Pid::from_u32(4242),
            name: "sleep".to_string(),
            cmd: Some("/usr/bin/sleep 1000".to_string()),
            exe: Some(PathBuf::from("/usr/bin/sleep")),
            user: "tester".to_string(),
            uid: Some(1000),
            state: "S".to_string(),
            cpu_usage: 0.5,
            memory: 1024 * 1024,
            parent: Some(sysinfo::Pid::from_u32(1)),
            threads: Some(1),
        }
    }

    fn app() -> App {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Processes;
        app.update(Snapshot {
            processes: ProcessList {
                processes: vec![process()],
                vanished: 0,
            },
            ..Snapshot::default()
        });
        app
    }

    #[test]
    fn help_lists_the_shortcuts() {
        let mut app = app();
        app.dialog = Some(Dialog::Help);
        let output = render(&mut app, 140, 45);
        assert!(output.contains("quit"), "got: {output}");
        assert!(output.contains("sort by CPU"), "got: {output}");
        assert!(output.contains("search processes"), "got: {output}");
    }

    #[test]
    fn details_show_the_executable_and_command() {
        let mut app = app();
        app.dialog = Some(Dialog::ProcessDetails);
        let output = render(&mut app, 160, 45);
        assert!(output.contains("/usr/bin/sleep"), "got: {output}");
        assert!(output.contains("4242"), "got: {output}");
    }

    #[test]
    fn confirmation_names_the_process_and_signal() {
        let mut app = app();
        app.dialog = Some(Dialog::ConfirmKill {
            pid: 4242,
            name: "sleep".to_string(),
            signal: TerminateSignal::Term,
        });
        let output = render(&mut app, 140, 45);
        assert!(output.contains("SIGTERM"), "got: {output}");
        assert!(output.contains("4242"), "got: {output}");
        assert!(output.contains("sleep"), "got: {output}");
    }

    #[test]
    fn kill_dialog_warns_about_sigkill() {
        let mut app = app();
        app.dialog = Some(Dialog::ConfirmKill {
            pid: 4242,
            name: "sleep".to_string(),
            signal: TerminateSignal::Kill,
        });
        let output = render(&mut app, 140, 45);
        assert!(output.contains("SIGKILL"), "got: {output}");
        assert!(output.contains("cannot be caught"), "got: {output}");
    }

    #[test]
    fn details_of_a_vanished_process_are_handled() {
        let mut app = app();
        app.dialog = Some(Dialog::ProcessDetails);
        app.update(crate::collector::Snapshot::default());
        let output = render(&mut app, 140, 45);
        assert!(output.contains("gone"), "got: {output}");
    }
}
