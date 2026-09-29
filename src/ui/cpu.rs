//! CPU dashboard: utilization gauge, per-core detail, state breakdown and history.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Chart, Dataset, GraphType, Paragraph};

use super::theme::Theme;
use super::widgets;
use crate::app::App;
use crate::collector::cpu::CpuSnapshot;
use crate::format;

/// Draws the CPU panel into `area`.
pub fn draw(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(cpu) = app.snapshot().map(|snapshot| &snapshot.cpu) else {
        frame.render_widget(
            widgets::key_values(
                &[("status", "waiting for the first sample".to_string())],
                theme,
            ),
            area,
        );
        return;
    };

    let [top, bottom] = Layout::vertical([Constraint::Length(9), Constraint::Min(0)]).areas(area);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(top);

    draw_summary(frame, app, left, theme, cpu);
    draw_cores(frame, right, theme, cpu);
    draw_graph(frame, bottom, theme, app);
}

/// Model, core counts, utilization gauge and the state breakdown.
fn draw_summary(frame: &mut Frame, app: &App, area: Rect, theme: &Theme, cpu: &CpuSnapshot) {
    let inner = widgets::panel("CPU", theme, true).inner(area);
    frame.render_widget(widgets::panel("CPU", theme, true), area);

    let usage = cpu.usage.clamp(0.0, 100.0);
    let [gauge_area, rest] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    frame.render_widget(
        widgets::usage_gauge(
            "CPU",
            usage as f64 / 100.0,
            theme,
            app.config.unicode_graphs,
        ),
        gauge_area,
    );

    let pairs = vec![
        ("Model", format::optional(cpu.brand.clone())),
        (
            "Cores",
            match cpu.physical_cores {
                Some(physical) => format!("{physical} physical / {} logical", cpu.logical_cores),
                None => format!("{} logical", cpu.logical_cores),
            },
        ),
        (
            "Frequency",
            cpu.frequency_mhz
                .map(|mhz| format!("{mhz} MHz"))
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
        (
            "Temperature",
            cpu.temperature
                .map(|c| format!("{c:.1} °C"))
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
        ("Load", format::percent(f64::from(cpu.busy_percent()))),
    ];
    frame.render_widget(widgets::key_values(&pairs, theme), rest);
}

/// Per-core utilization as a compact list of labelled bars.
fn draw_cores(frame: &mut Frame, area: Rect, theme: &Theme, cpu: &CpuSnapshot) {
    frame.render_widget(widgets::panel("Per core", theme, false), area);
    if cpu.cores.is_empty() {
        return;
    }
    let inner = widgets::panel("Per core", theme, false).inner(area);

    // A 16 column core grid keeps 8+ core machines readable without scrolling.
    let columns = 4usize;
    let rows = cpu.cores.len().div_ceil(columns);
    let [grid_area, legend] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let chunks =
        Layout::horizontal(vec![Constraint::Ratio(1, columns as u32); columns]).split(grid_area);
    let mut lines: Vec<Vec<Line>> = vec![Vec::new(); rows];
    for (index, core) in cpu.cores.iter().enumerate() {
        let row = index % rows;
        let column = index / rows;
        if column >= columns {
            continue;
        }
        let value = core.usage.clamp(0.0, 100.0) as f64 / 100.0;
        let color = theme.usage_color(value * 100.0);
        let bar_width = 8usize;
        let filled = (value * bar_width as f64).round() as usize;
        let bar: String =
            "█".repeat(filled.min(bar_width)) + &"·".repeat(bar_width - filled.min(bar_width));
        let text = format!("{:>3} {:<8} {:>5.1}", index + 1, bar, core.usage);
        lines[row].push(Line::from(Span::styled(
            format::truncate(&text, chunks[0].width as usize),
            Style::default().fg(color),
        )));
    }
    let visible_rows = grid_area.height as usize;
    let paragraph = Paragraph::new(
        lines
            .into_iter()
            .take(visible_rows)
            .flatten()
            .collect::<Vec<_>>(),
    );
    frame.render_widget(paragraph, grid_area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  user / system / idle / iowait",
            theme.dim(),
        ))),
        legend,
    );
}

/// The `times` breakdown and the scrolling utilization graph.
fn draw_graph(frame: &mut Frame, area: Rect, theme: &Theme, app: &App) {
    let [left, right] =
        Layout::horizontal([Constraint::Length(30), Constraint::Min(0)]).areas(area);
    draw_times(frame, left, theme, app);
    draw_history(frame, right, theme, app);
}

/// The user / system / idle / iowait percentages of the sampling interval.
fn draw_times(frame: &mut Frame, area: Rect, theme: &Theme, app: &App) {
    let cpu = app.snapshot().map(|snapshot| &snapshot.cpu);
    frame.render_widget(widgets::panel("CPU times", theme, false), area);
    let Some(cpu) = cpu else { return };
    let inner = widgets::panel("CPU times", theme, false).inner(area);

    if !cpu.times.valid {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "second sample pending",
                theme.dim(),
            ))),
            inner,
        );
        return;
    }

    let times = cpu.times;
    let rows = [
        ("user", times.user),
        ("system", times.system),
        ("interrupt", times.interrupt),
        ("steal", times.steal),
        ("iowait", times.iowait),
        ("idle", times.idle),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(label, value)| {
            let ratio = (f64::from(*value) / 100.0).clamp(0.0, 1.0);
            let filled = (ratio * 10.0).round() as usize;
            let bar: String = "▇".repeat(filled.min(10)) + &" ".repeat(10 - filled.min(10));
            let color = if *label == "idle" {
                theme.dim
            } else {
                theme.usage_color(100.0 - f64::from(*value))
            };
            Line::from(vec![
                Span::styled(format::pad(label, 10), theme.dim()),
                Span::styled(bar, Style::default().fg(color)),
                Span::styled(format!("{:>6.1}%", value), theme.text()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    let _ = app;
}

/// A scrolling line chart of the total and per-core utilization.
fn draw_history(frame: &mut Frame, area: Rect, theme: &Theme, app: &App) {
    let block = widgets::panel("History (last samples)", theme, false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.cpu_history.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("collecting...", theme.dim()))),
            inner,
        );
        return;
    }

    let total = app.cpu_history.chart_points();
    let datasets = vec![
        Dataset::default()
            .name("total")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(theme.primary))
            .data(&total),
    ];

    let x_max = (app.cpu_history.len().max(2) - 1) as f64;
    let chart = Chart::new(datasets)
        .style(Style::default().bg(theme.background).fg(theme.border))
        .x_axis(
            Axis::default()
                .title("sample")
                .style(theme.dim())
                .bounds([0.0, x_max])
                .labels(vec!["old".to_string(), "now".to_string()]),
        )
        .y_axis(
            Axis::default()
                .title("percent")
                .style(theme.dim())
                .bounds([0.0, 100.0])
                .labels(vec!["0".to_string(), "50".to_string(), "100".to_string()]),
        );
    frame.render_widget(chart, inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::Snapshot;
    use crate::collector::cpu::{CpuCore, CpuTimes};
    use crate::config::Config;
    use crate::ui::tests_support::render;

    fn snapshot_with_cpu() -> Snapshot {
        Snapshot {
            cpu: CpuSnapshot {
                brand: Some("Test CPU 9000".to_string()),
                physical_cores: Some(8),
                logical_cores: 4,
                frequency_mhz: Some(4200),
                usage: 42.5,
                cores: (0..4)
                    .map(|index| CpuCore {
                        usage: 10.0 * (index as f32 + 1.0),
                        frequency_mhz: Some(4200),
                        temperature: None,
                    })
                    .collect(),
                times: CpuTimes {
                    user: 20.0,
                    system: 10.0,
                    idle: 60.0,
                    iowait: 5.0,
                    interrupt: 3.0,
                    steal: 2.0,
                    valid: true,
                },
                temperature: Some(48.0),
            },
            ..Snapshot::default()
        }
    }

    fn app_with_cpu() -> App {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Cpu;
        app.update(snapshot_with_cpu());
        app.update(snapshot_with_cpu());
        app
    }

    #[test]
    fn shows_model_and_core_counts() {
        let mut app = app_with_cpu();
        let output = render(&mut app, 140, 45);
        assert!(output.contains("Test CPU 9000"), "got: {output}");
        assert!(output.contains("8 physical"), "got: {output}");
    }

    #[test]
    fn shows_the_state_breakdown() {
        let mut app = app_with_cpu();
        let output = render(&mut app, 140, 45);
        assert!(output.contains("user"), "got: {output}");
        assert!(output.contains("iowait"), "got: {output}");
    }

    #[test]
    fn renders_without_a_snapshot() {
        let mut app = App::new(Config::default());
        let output = render(&mut app, 120, 30);
        assert!(output.contains("waiting"), "got: {output}");
    }

    #[test]
    fn shows_placeholder_for_missing_brand() {
        let mut app = App::new(Config::default());
        let mut snapshot = snapshot_with_cpu();
        snapshot.cpu.brand = None;
        snapshot.cpu.frequency_mhz = None;
        snapshot.cpu.temperature = None;
        app.update(snapshot);
        let output = render(&mut app, 140, 45);
        assert!(output.contains("N/A"), "got: {output}");
    }

    #[test]
    fn narrow_terminal_does_not_panic() {
        let mut app = app_with_cpu();
        let _ = render(&mut app, 45, 12);
    }
}
