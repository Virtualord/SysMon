//! Network dashboard: per-interface throughput table and live rx/tx graphs.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Cell, Chart, Dataset, GraphType, Paragraph, Row, Table, TableState};

use super::theme::Theme;
use super::widgets;
use crate::app::{App, Focus};
use crate::collector::network::InterfaceStats;
use crate::format;

/// Draws the network panel into `area`.
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

    let [table_area, graphs] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(10)]).areas(area);
    let network = &snapshot.network;
    let focused = app.focus == Focus::NetworkRow;

    let title = format!("Interfaces ({})", network.interfaces.len());
    const HINT: &str =
        "rates are per-second averages · totals are cumulative since the link came up";
    let block = widgets::panel_with_hint(&title, HINT, theme, focused);
    let table = Table::new(
        network
            .interfaces
            .iter()
            .map(|interface| row(interface, theme)),
        [
            Constraint::Min(10),
            Constraint::Length(6),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(8),
        ],
    )
    .header(
        Row::new(vec![
            Cell::from("INTERFACE"),
            Cell::from("STATE"),
            Cell::from("RX/s"),
            Cell::from("TX/s"),
            Cell::from("RX TOTAL"),
            Cell::from("TX TOTAL"),
            Cell::from("MAC"),
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
            .min(network.interfaces.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, table_area, &mut state);

    let [rx_area, tx_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(graphs);
    draw_graph(frame, rx_area, theme, app, true);
    draw_graph(frame, tx_area, theme, app, false);
}

/// One row per interface.
fn row<'a>(interface: &InterfaceStats, theme: &Theme) -> Row<'a> {
    let state = if !interface.is_up {
        Span::styled("down", Style::default().fg(theme.warn))
    } else if interface.is_loopback {
        Span::styled("loop", theme.dim())
    } else {
        Span::styled("up", Style::default().fg(theme.ok))
    };

    let errors = interface.counters.errors_received + interface.counters.errors_transmitted;
    let error_style = if errors > 0 {
        Style::default()
            .fg(theme.critical)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    Row::new(vec![
        Cell::from(Span::raw(interface.name.clone())),
        Cell::from(state),
        Cell::from(Span::styled(
            format::rate(interface.receive_rate),
            Style::default().fg(theme.primary),
        )),
        Cell::from(Span::styled(
            format::rate(interface.transmit_rate),
            Style::default().fg(theme.secondary),
        )),
        Cell::from(format::bytes(interface.total_received)),
        Cell::from(format::bytes(interface.total_transmitted)),
        Cell::from(Span::styled(
            interface
                .mac_address
                .clone()
                .unwrap_or_else(|| "-".to_string()),
            error_style,
        )),
    ])
}

/// A live sparkline graph of the aggregate receive or transmit rate.
fn draw_graph(frame: &mut Frame, area: Rect, theme: &Theme, app: &App, receive: bool) {
    let title = if receive {
        "Receive rate"
    } else {
        "Transmit rate"
    };
    let block = widgets::panel(title, theme, false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let series = if receive {
        &app.network_rx_history
    } else {
        &app.network_tx_history
    };
    if series.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("collecting...", theme.dim()))),
            inner,
        );
        return;
    }

    // Rates are unbounded, so the axis is scaled to the observed peak with a floor that
    // keeps a flat line from filling the whole chart.
    let peak = series.max().max(1.0);
    let values: Vec<f64> = series.iter().map(|value| value / peak * 100.0).collect();
    let points: Vec<(f64, f64)> = values
        .iter()
        .enumerate()
        .map(|(index, value)| (index as f64, *value))
        .collect();
    let color = if receive {
        theme.primary
    } else {
        theme.secondary
    };
    let datasets = vec![
        Dataset::default()
            .name(title)
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(color))
            .data(&points),
    ];
    let x_max = (values.len().max(2) - 1) as f64;
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
                .style(theme.dim())
                .bounds([0.0, 100.0])
                .labels(vec![
                    "0".to_string(),
                    format::rate(peak / 2.0),
                    format::rate(peak),
                ]),
        );
    frame.render_widget(chart, inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::Snapshot;
    use crate::collector::network::{InterfaceCounters, NetworkSnapshot};
    use crate::config::Config;
    use crate::ui::tests_support::render;

    fn interface(name: &str) -> InterfaceStats {
        InterfaceStats {
            name: name.to_string(),
            total_received: 123_456_789,
            total_transmitted: 9_876_543,
            receive_rate: 125_000.0,
            transmit_rate: 12_500.0,
            counters: InterfaceCounters::default(),
            mac_address: Some("aa:bb:cc:dd:ee:ff".to_string()),
            is_up: true,
            is_loopback: false,
        }
    }

    fn network_snapshot() -> NetworkSnapshot {
        NetworkSnapshot {
            interfaces: vec![interface("wlp2s0"), interface("docker0")],
            receive_rate: 137_500.0,
            transmit_rate: 12_500.0,
            total_received: 133_333_332,
            total_transmitted: 9_876_543,
        }
    }

    fn app_with_interfaces() -> App {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Network;
        let snapshot = Snapshot {
            network: network_snapshot(),
            ..Snapshot::default()
        };
        app.update(snapshot.clone());
        app.update(snapshot);
        app
    }

    #[test]
    fn renders_the_interface_table() {
        let mut app = app_with_interfaces();
        let output = render(&mut app, 150, 40);
        assert!(output.contains("INTERFACE"), "got: {output}");
        assert!(output.contains("wlp2s0"), "got: {output}");
        assert!(output.contains("RX/s"), "got: {output}");
    }

    #[test]
    fn distinguishes_rates_from_totals() {
        let mut app = app_with_interfaces();
        let output = render(&mut app, 150, 40);
        assert!(
            output.contains("KiB/s") || output.contains("MiB/s"),
            "rates missing: {output}"
        );
        assert!(output.contains("RX TOTAL"), "totals missing: {output}");
    }

    #[test]
    fn shows_both_graphs() {
        let mut app = app_with_interfaces();
        let output = render(&mut app, 150, 40);
        assert!(output.contains("Receive rate"), "got: {output}");
        assert!(output.contains("Transmit rate"), "got: {output}");
    }

    #[test]
    fn empty_network_is_handled() {
        let mut app = App::new(Config::default());
        app.view = crate::app::View::Network;
        app.update(Snapshot::default());
        let output = render(&mut app, 120, 30);
        assert!(output.contains("Interfaces (0)"), "got: {output}");
    }

    #[test]
    fn narrow_terminal_does_not_panic() {
        let mut app = app_with_interfaces();
        let _ = render(&mut app, 45, 14);
    }
}
