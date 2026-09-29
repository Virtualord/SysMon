//! Memory dashboard: RAM and swap gauges, the `/proc/meminfo` breakdown and history.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Chart, Dataset, GraphType, Paragraph};

use super::theme::Theme;
use super::widgets;
use crate::app::App;
use crate::collector::memory::MemorySnapshot;
use crate::format;

/// Draws the memory panel into `area`.
pub fn draw(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(memory) = app.snapshot().map(|snapshot| snapshot.memory) else {
        frame.render_widget(
            widgets::key_values(
                &[("status", "waiting for the first sample".to_string())],
                theme,
            ),
            area,
        );
        return;
    };

    let [gauges, bottom] =
        Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).areas(area);
    let [ram, swap] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(gauges);
    let [details, graph] =
        Layout::horizontal([Constraint::Length(38), Constraint::Min(0)]).areas(bottom);

    draw_ram(frame, ram, theme, app, &memory);
    draw_swap(frame, swap, theme, app, &memory);
    draw_details(frame, details, theme, &memory);
    draw_history(frame, graph, theme, app);
}

/// RAM gauge plus total / used / available / free.
fn draw_ram(frame: &mut Frame, area: Rect, theme: &Theme, app: &App, memory: &MemorySnapshot) {
    let block = widgets::panel("Memory", theme, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [gauge, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    let percent = memory.used_percent();
    frame.render_widget(
        widgets::value_bar(
            "RAM",
            &format!(
                "{}/{}",
                format::bytes(memory.used),
                format::bytes(memory.total)
            ),
            percent / 100.0,
            theme,
            app.config.unicode_graphs,
        ),
        gauge,
    );

    let pairs = vec![
        ("Total", format::bytes(memory.total)),
        ("Used", format::bytes(memory.used)),
        ("Available", format::bytes(memory.available)),
        ("Free", format::bytes(memory.free)),
    ];
    frame.render_widget(widgets::key_values(&pairs, theme), rest);
}

/// Swap gauge, or an explicit "no swap" note.
fn draw_swap(frame: &mut Frame, area: Rect, theme: &Theme, app: &App, memory: &MemorySnapshot) {
    let title = if memory.has_swap() {
        "Swap"
    } else {
        "Swap (not configured)"
    };
    let block = widgets::panel(title, theme, false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !memory.has_swap() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no swap partition on this system",
                theme.dim(),
            )))
            .wrap(ratatui::widgets::Wrap { trim: false }),
            inner,
        );
        return;
    }

    let [gauge, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    frame.render_widget(
        widgets::value_bar(
            "Swap",
            &format!(
                "{}/{}",
                format::bytes(memory.swap_used),
                format::bytes(memory.swap_total)
            ),
            memory.swap_used_percent() / 100.0,
            theme,
            app.config.unicode_graphs,
        ),
        gauge,
    );

    let pairs = vec![
        ("Total", format::bytes(memory.swap_total)),
        ("Used", format::bytes(memory.swap_used)),
        ("Free", format::bytes(memory.swap_free)),
        (
            "Cached",
            memory
                .swap_cached
                .map(format::bytes)
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
    ];
    frame.render_widget(widgets::key_values(&pairs, theme), rest);
}

/// The cache and buffer breakdown from `/proc/meminfo`, with an explanation of the
/// "used" figure so the number is not a black box.
fn draw_details(frame: &mut Frame, area: Rect, theme: &Theme, memory: &MemorySnapshot) {
    let block = widgets::panel("Breakdown", theme, false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let pairs = vec![
        (
            "Buffers",
            memory
                .buffers
                .map(format::bytes)
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
        (
            "Cached",
            memory
                .cached
                .map(format::bytes)
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
        (
            "Reclaimable",
            memory
                .reclaimable
                .map(format::bytes)
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
        (
            "Shmem",
            memory
                .shmem
                .map(format::bytes)
                .unwrap_or_else(|| format::NOT_AVAILABLE.to_string()),
        ),
    ];
    frame.render_widget(widgets::key_values(&pairs, theme), inner);
}

/// Scrolling RAM utilization graph.
fn draw_history(frame: &mut Frame, area: Rect, theme: &Theme, app: &App) {
    let block = widgets::panel("Memory history", theme, false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.memory_history.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("collecting...", theme.dim()))),
            inner,
        );
        return;
    }

    let points = app.memory_history.chart_points();
    let datasets = vec![
        Dataset::default()
            .name("ram %")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(theme.primary))
            .data(&points),
    ];
    let x_max = (app.memory_history.len().max(2) - 1) as f64;
    let chart = Chart::new(datasets)
        .style(Style::default().bg(theme.background).fg(theme.border))
        .x_axis(
            Axis::default()
                .style(theme.dim())
                .bounds([0.0, x_max])
                .labels(vec!["old".to_string(), "now".to_string()]),
        )
        .y_axis(
            Axis::default()
                .title("percent used")
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
    use crate::config::Config;
    use crate::ui::tests_support::render;

    fn memory_snapshot() -> MemorySnapshot {
        MemorySnapshot {
            total: 16 * 1024 * 1024 * 1024,
            used: 8 * 1024 * 1024 * 1024,
            available: 7 * 1024 * 1024 * 1024,
            free: 1024 * 1024 * 1024,
            buffers: Some(256 * 1024 * 1024),
            cached: Some(4 * 1024 * 1024 * 1024),
            reclaimable: Some(512 * 1024 * 1024),
            shmem: Some(128 * 1024 * 1024),
            swap_total: 2 * 1024 * 1024 * 1024,
            swap_used: 512 * 1024 * 1024,
            swap_free: 1536 * 1024 * 1024,
            swap_cached: Some(64 * 1024 * 1024),
        }
    }

    fn snapshot_with_memory() -> Snapshot {
        Snapshot {
            memory: memory_snapshot(),
            ..Snapshot::default()
        }
    }

    fn app_with_memory() -> App {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Memory;
        app.update(snapshot_with_memory());
        app.update(snapshot_with_memory());
        app
    }

    #[test]
    fn shows_ram_and_swap_figures() {
        let mut app = app_with_memory();
        let output = render(&mut app, 140, 45);
        assert!(output.contains("RAM"), "got: {output}");
        assert!(output.contains("16.0 GiB"), "got: {output}");
        assert!(output.contains("8.0 GiB"), "got: {output}");
        assert!(output.contains("Swap"), "got: {output}");
    }

    #[test]
    fn shows_the_cache_breakdown() {
        let mut app = app_with_memory();
        let output = render(&mut app, 160, 45);
        assert!(output.contains("Buffers"), "got: {output}");
        assert!(output.contains("Cached"), "got: {output}");
    }

    #[test]
    fn reports_missing_swap_explicitly() {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Memory;
        let mut memory = memory_snapshot();
        memory.swap_total = 0;
        memory.swap_used = 0;
        memory.swap_free = 0;
        app.update(Snapshot {
            memory,
            ..Snapshot::default()
        });
        let output = render(&mut app, 140, 45);
        assert!(output.contains("no swap"), "got: {output}");
    }

    #[test]
    fn shows_placeholders_when_meminfo_is_unavailable() {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Memory;
        let mut memory = memory_snapshot();
        memory.buffers = None;
        memory.cached = None;
        memory.reclaimable = None;
        memory.shmem = None;
        app.update(Snapshot {
            memory,
            ..Snapshot::default()
        });
        let output = render(&mut app, 160, 45);
        assert!(output.contains("N/A"), "got: {output}");
    }

    #[test]
    fn renders_without_a_snapshot() {
        let mut app = App::new(Config::default());
        let output = render(&mut app, 120, 30);
        assert!(output.contains("waiting"), "got: {output}");
    }
}
