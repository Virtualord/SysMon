//! Frame layout: header, tab bar, the active panel and the status bar.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::theme::Theme;
use super::widgets;
use crate::app::{App, Focus, View};
use crate::collector::processes::{ProcessFilter, SearchField};
use crate::format;

/// Fixed height of the header line.
const HEADER_HEIGHT: u16 = 1;
/// Fixed height of the tab bar.
const TAB_HEIGHT: u16 = 1;
/// Fixed height of the status bar.
const STATUS_HEIGHT: u16 = 1;

/// Draws the whole frame chrome and the active view.
pub fn draw(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let area = frame.area();
    let [header, tabs, body, status] = Layout::vertical([
        Constraint::Length(HEADER_HEIGHT),
        Constraint::Length(TAB_HEIGHT),
        Constraint::Min(0),
        Constraint::Length(STATUS_HEIGHT),
    ])
    .areas(area);

    draw_header(frame, app, theme, header);
    draw_tabs(frame, app, theme, tabs);
    super::draw_view(frame, app, body, theme);
    draw_status_bar(frame, app, theme, status);
}

/// Application name, version, host, distribution, kernel, uptime and load average.
fn draw_header(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let snapshot = app.snapshot();
    let clock = local_clock();
    let mut spans = vec![
        Span::styled(
            format!(" {} ", crate::NAME),
            Style::default()
                .fg(theme.selection_text)
                .bg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(crate::VERSION, theme.dim()),
    ];

    if let Some(snapshot) = snapshot {
        spans.push(Span::styled("  host ", theme.dim()));
        spans.push(Span::styled(snapshot.system.hostname.clone(), theme.text()));
        if let Some(distribution) = &snapshot.system.distribution {
            spans.push(Span::styled("  ", theme.dim()));
            spans.push(Span::styled(distribution.clone(), theme.text()));
        }
        spans.push(Span::styled("  kernel ", theme.dim()));
        spans.push(Span::styled(
            format::truncate(&snapshot.system.kernel, 30),
            theme.text(),
        ));
        spans.push(Span::styled("  up ", theme.dim()));
        spans.push(Span::styled(
            format::duration(snapshot.dynamic.uptime),
            theme.text(),
        ));
        if app.config.show_load_average && snapshot.dynamic.load_available() > 0 {
            spans.push(Span::styled("  load ", theme.dim()));
            spans.push(Span::styled(
                format!(
                    "{} {} {}",
                    format::load_average(snapshot.dynamic.load_one),
                    format::load_average(snapshot.dynamic.load_five),
                    format::load_average(snapshot.dynamic.load_fifteen)
                ),
                theme.text(),
            ));
        }
    } else {
        spans.push(Span::styled(
            "  waiting for the first sample...",
            theme.dim(),
        ));
    }

    // The wall clock is right aligned; its width is known from the rendered prefix.
    let prefix_width: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    let clock_width = clock.len() + 2;
    if area.width as usize > prefix_width + clock_width {
        let padding = " ".repeat(area.width as usize - prefix_width - clock_width);
        spans.push(Span::raw(padding));
        spans.push(Span::styled(clock, theme.dim()));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.background)),
        area,
    );
}

/// The current local time formatted as `HH:MM:SS`.
///
/// The conversion goes through the platform's `localtime_r`, which is the only way to
/// honour `/etc/localtime` and `TZ` without pulling in a timezone database crate.
/// `chrono`/`time` were rejected here to keep the dependency count down: the header
/// needs a wall clock and nothing else.
pub fn local_clock() -> String {
    let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return "--:--:--".to_string();
    };
    let seconds = elapsed.as_secs() as libc::time_t;

    // SAFETY: `localtime_r` writes a `struct tm` into the provided pointer and returns
    // a pointer to it, or null on failure. The buffer is a local `MaybeUninit` that
    // lives until the end of the statement, the pointer is checked for null, and
    // `localtime_r` is documented to be reentrant, so no shared state is touched. No
    // reference outlives the buffer.
    let local = unsafe {
        let mut buffer: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&seconds, &mut buffer).is_null() {
            None
        } else {
            Some(buffer)
        }
    };

    match local {
        Some(time) => format!("{:02}:{:02}:{:02}", time.tm_hour, time.tm_min, time.tm_sec),
        None => {
            // Fall back to UTC, which is still a correct clock rather than a wrong one.
            let day = seconds.rem_euclid(86_400);
            format!("{:02}:{:02}:{:02}", day / 3600, (day % 3600) / 60, day % 60)
        }
    }
}

/// The tab bar, with the active view highlighted.
fn draw_tabs(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let mut spans = Vec::new();
    for (index, view) in View::ALL.iter().enumerate() {
        let active = *view == app.view;
        let label = format!(" {} {} ", index + 1, view.title());
        let style = if active {
            theme.title_active()
        } else {
            theme.dim()
        };
        spans.push(Span::styled(label, style));
        if index + 1 < View::ALL.len() {
            spans.push(Span::styled("│", theme.border));
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.background)),
        area,
    );
}

/// The status bar: active view, sort state, search state and transient messages.
fn draw_status_bar(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let mut spans = Vec::new();
    spans.push(Span::styled(
        format!(" {} ", app.view.title()),
        theme.title_active(),
    ));

    if app.view == View::Processes {
        spans.push(Span::styled(
            format!("{}/{} procs", app.process_rows(), app.total_processes()),
            theme.dim(),
        ));
        let direction = if app.sort_descending { "desc" } else { "asc" };
        spans.push(Span::styled(
            format!("  sort {} {}", app.sort_key.label(), direction),
            theme.dim(),
        ));
        spans.push(Span::styled(
            format!("  filter {}", filter_label(app.filter)),
            theme.dim(),
        ));
        if !app.search.text.is_empty() {
            let field = match app.search.field {
                SearchField::Name => "name",
                SearchField::Pid => "pid",
                SearchField::All => "all",
            };
            spans.push(Span::styled(
                format!("  /{field}:{} ", app.search.text),
                theme.accent_style(),
            ));
        }
        let vanished = app.vanished_processes();
        if vanished > 0 {
            spans.push(Span::styled(format!("  {vanished} exited"), theme.dim()));
        }
    }

    if app.focus != Focus::Main {
        spans.push(Span::styled("  [focus: table]", theme.dim()));
    }

    // A message overrides the hints so an error cannot be missed.
    if let Some(message) = &app.message {
        spans.push(Span::raw("  │ "));
        let style = if message.is_error {
            theme.error()
        } else {
            theme.accent_style()
        };
        spans.push(Span::styled(format::truncate(&message.text, 60), style));
    } else {
        spans.push(Span::styled(
            "  │  ?:help  q:quit  Tab:view  /:search  r:refresh",
            theme.dim(),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.background)),
        area,
    );
}

/// Draws the inline search prompt over the status bar.
pub fn draw_search_prompt(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let height = STATUS_HEIGHT.min(area.height);
    let prompt_area = Rect {
        x: area.x,
        y: area.y + area.height - height,
        width: area.width,
        height,
    };
    let block = Block::default();
    let inner = block.inner(prompt_area);
    let field = match app.search.field {
        SearchField::Name => "name",
        SearchField::Pid => "pid",
        SearchField::All => "all",
    };
    let text = Line::from(vec![
        Span::styled(
            " search ",
            Style::default().fg(theme.selection_text).bg(theme.accent),
        ),
        Span::styled(format!("[{field}] > {}", app.search.text), theme.text()),
        Span::styled("█", theme.accent_style()),
        Span::styled(
            "  (Enter accept · Esc cancel · Tab accept & close)",
            theme.dim(),
        ),
    ]);
    frame.render_widget(Paragraph::new(text), inner);
    let _ = widgets::MIN_WIDTH;
}

/// Label for a process filter.
fn filter_label(filter: ProcessFilter) -> &'static str {
    match filter {
        ProcessFilter::All => "all",
        ProcessFilter::User => "mine",
        ProcessFilter::UserTasks => "tasks",
    }
}

impl Theme {
    /// Accent style used for the search prompt and informational messages.
    fn accent_style(&self) -> Style {
        Style::default().fg(self.accent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ui::tests_support::render;

    #[test]
    fn header_shows_waiting_state_without_data() {
        let mut app = App::new(Config::default());
        let output = render(&mut app, 120, 30);
        assert!(output.contains("waiting for the first sample"));
    }

    #[test]
    fn tabs_are_all_rendered() {
        let mut app = App::new(Config::default());
        let output = render(&mut app, 120, 30);
        for view in View::ALL {
            assert!(
                output.contains(view.title()),
                "tab {} missing",
                view.title()
            );
        }
    }

    #[test]
    fn status_bar_shows_help_hint() {
        let mut app = App::new(Config::default());
        let output = render(&mut app, 120, 30);
        assert!(output.contains("?:help"));
    }

    #[test]
    fn filter_labels_are_distinct() {
        let labels = [
            filter_label(ProcessFilter::All),
            filter_label(ProcessFilter::User),
            filter_label(ProcessFilter::UserTasks),
        ];
        assert_eq!(labels, ["all", "mine", "tasks"]);
    }
}
