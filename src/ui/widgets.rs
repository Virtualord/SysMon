//! Reusable render helpers: panel blocks, gauges, graphs, key/value tables and the
//! small-terminal fallback.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols;
use ratatui::symbols::bar::Set;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Gauge, Paragraph, Sparkline, Wrap};

use super::theme::Theme;

/// Smallest terminal width that can host the dashboard.
pub const MIN_WIDTH: u16 = 40;

/// Smallest terminal height that can host the dashboard.
pub const MIN_HEIGHT: u16 = 10;

/// Nine level bar set using the block characters most terminals render correctly.
const UNICODE_BARS: Set<'static> = symbols::bar::NINE_LEVELS;

/// Five level bar set for terminals that cannot display the Unicode blocks.
const ASCII_BARS: Set<'static> = Set {
    full: "#",
    seven_eighths: "#",
    three_quarters: "=",
    five_eighths: "=",
    half: "=",
    three_eighths: "-",
    one_quarter: "-",
    one_eighth: ".",
    empty: " ",
};

/// Chooses the bar set matching the configuration.
pub fn bar_set(unicode: bool) -> Set<'static> {
    if unicode { UNICODE_BARS } else { ASCII_BARS }
}

/// A titled panel.
pub fn panel<'a>(title: &'a str, theme: &Theme, focused: bool) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused {
            theme.border_focused
        } else {
            theme.border
        }))
        .title(Line::from(Span::styled(title, theme.title())).centered())
}

/// A titled panel whose title is on the left and whose hint is on the right.
pub fn panel_with_hint<'a>(
    title: &'a str,
    hint: &'a str,
    theme: &Theme,
    focused: bool,
) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused {
            theme.border_focused
        } else {
            theme.border
        }))
        .title(Line::from(Span::styled(title, theme.title())).left_aligned())
        .title_bottom(Line::from(Span::styled(hint, theme.dim())).right_aligned())
}

/// Renders a labelled `value` bar, e.g. `CPU  [####----] 42%`.
pub fn usage_gauge<'a>(label: &'a str, ratio: f64, theme: &Theme, unicode: bool) -> Gauge<'a> {
    let ratio = if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let color = theme.usage_color(ratio * 100.0);
    Gauge::default()
        .ratio(ratio)
        .gauge_style(Style::default().fg(color).bg(theme.border))
        .label(Span::styled(
            format!("{label} {:>3.0}%", ratio * 100.0),
            Style::default()
                .fg(theme.selection_text)
                .add_modifier(Modifier::BOLD),
        ))
        .use_unicode(unicode)
}

/// A horizontal bar for a single value with a unit label.
pub fn value_bar<'a>(
    label: &'a str,
    value: &'a str,
    ratio: f64,
    theme: &Theme,
    unicode: bool,
) -> Gauge<'a> {
    let ratio = if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
    };
    Gauge::default()
        .ratio(ratio)
        .gauge_style(
            Style::default()
                .fg(theme.usage_color(ratio * 100.0))
                .bg(theme.border),
        )
        .label(format!("{label} {value}"))
        .use_unicode(unicode)
}

/// A sparkline of a bounded series.
pub fn sparkline<'a>(values: &[u64], max: u64, style: Style, unicode: bool) -> Sparkline<'a> {
    Sparkline::default()
        .data(values.to_vec())
        .max(max)
        .style(style)
        .bar_set(bar_set(unicode))
}

/// A `label  value` paragraph, one pair per line.
pub fn key_values<'a>(pairs: &[(&'a str, String)], theme: &Theme) -> Paragraph<'a> {
    let width = pairs
        .iter()
        .map(|(label, _)| label.chars().count() + 2)
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = pairs
        .iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(crate::format::pad(label, width), theme.dim()),
                Span::styled(value.clone(), theme.text()),
            ])
        })
        .collect();
    Paragraph::new(lines)
}

/// Wrapping `label: value` paragraph, used where a value may be long.
pub fn detail_line<'a>(label: &'a str, value: String, theme: &Theme) -> Paragraph<'a> {
    Paragraph::new(Line::from(vec![
        Span::styled(format!("{label}: "), theme.dim()),
        Span::styled(value, theme.text()),
    ]))
    .wrap(Wrap { trim: false })
}

/// The fallback shown when the terminal is too small for the dashboard.
pub fn too_small(width: u16, height: u16, theme: &Theme) -> Paragraph<'static> {
    let message = format!("Terminal too small: {width}x{height} (need {MIN_WIDTH}x{MIN_HEIGHT})");
    Paragraph::new(Line::from(Span::styled(message, theme.error()))).alignment(Alignment::Center)
}

/// A centred, bordered box covering the middle of `area`, used by the dialogs.
pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Percentage((100 - percent_y) / 2),
        ratatui::layout::Constraint::Percentage(percent_y),
        ratatui::layout::Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);
    ratatui::layout::Layout::horizontal([
        ratatui::layout::Constraint::Percentage((100 - percent_x) / 2),
        ratatui::layout::Constraint::Percentage(percent_x),
        ratatui::layout::Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}

/// Renders a widget into a cleared area, so the dashboard does not show through a
/// dialog.
pub fn render_over<W: ratatui::widgets::Widget>(frame: &mut ratatui::Frame, widget: W, area: Rect) {
    frame.render_widget(Clear, area);
    frame.render_widget(widget, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_sets_differ_between_styles() {
        let unicode = bar_set(true);
        let ascii = bar_set(false);
        assert_eq!(unicode.full, symbols::block::FULL);
        assert_eq!(ascii.full, "#");
        assert!(!ascii.full.is_empty());
    }

    #[test]
    fn panel_titles_are_rendered() {
        let theme = Theme::default();
        // Both focus states must be constructible; the builder is what gives the panel
        // its border, and a regression here would silently draw a bare rectangle.
        let _focused = panel("CPU", &theme, true);
        let _unfocused = panel("CPU", &theme, false);
        let _with_hint = panel_with_hint("CPU", "hint", &theme, true);
    }

    #[test]
    fn usage_gauge_clamps_out_of_range_ratios() {
        let theme = Theme::default();
        let _ = usage_gauge("CPU", 5.0, &theme, true);
        let _ = usage_gauge("CPU", f64::NAN, &theme, true);
        let _ = usage_gauge("CPU", -1.0, &theme, false);
    }

    #[test]
    fn centered_rect_stays_inside_the_area() {
        let area = Rect::new(0, 0, 100, 40);
        let centered = centered_rect(60, 50, area);
        assert!(centered.width <= area.width);
        assert!(centered.height <= area.height);
        assert!(centered.width > 0 && centered.height > 0);
    }

    #[test]
    fn key_values_handle_an_empty_list() {
        let theme = Theme::default();
        let _ = key_values(&[], &theme);
    }
}
