//! Rendering.
//!
//! Every function in this module is a pure function of [`crate::app::App`] and the
//! frame. Nothing here touches the filesystem, the clock or the terminal state, which
//! is what makes the views testable with a `TestBackend`.

pub mod cpu;
pub mod dashboard;
pub mod dialogs;
pub mod memory;
pub mod network;
pub mod processes;
pub mod storage;
pub mod system;
pub mod theme;
pub mod widgets;

#[cfg(test)]
pub(crate) mod tests_support {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::App;

    /// Renders the app into an in-memory buffer and returns it as text.
    pub fn render(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| super::draw(frame, app))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

use ratatui::Frame;

use crate::app::{App, View};
use theme::Theme;

/// Draws the whole screen for the current state.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let theme = Theme::from_setting(app.config.theme);
    let area = frame.area();

    if area.width < widgets::MIN_WIDTH || area.height < widgets::MIN_HEIGHT {
        frame.render_widget(widgets::too_small(area.width, area.height, &theme), area);
        return;
    }

    dashboard::draw(frame, app, &theme);

    if let Some(dialog) = app.dialog.clone() {
        dialogs::draw(frame, app, &theme, dialog);
    }

    // The search prompt is an inline overlay on the status bar, not a modal.
    if app.search_owns_input() {
        dashboard::draw_search_prompt(frame, app, &theme, area);
    }
}

/// Draws the panel of the active view.
///
/// `app` is mutable because the process table records how many rows fit, which the
/// event loop needs for PageUp/PageDown. Each view reads the snapshot through
/// [`App::snapshot`] rather than taking it as an argument, which keeps this dispatch
/// free of overlapping borrows.
pub fn draw_view(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect, theme: &Theme) {
    match app.view {
        View::Cpu => cpu::draw(frame, app, area, theme),
        View::Memory => memory::draw(frame, app, area, theme),
        View::Processes => processes::draw(frame, app, area, theme),
        View::Storage => storage::draw(frame, app, area, theme),
        View::Network => network::draw(frame, app, area, theme),
        View::System => system::draw(frame, app, area, theme),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn app() -> App {
        App::new(Config::default())
    }

    #[test]
    fn renders_without_data() {
        let mut app = app();
        let output = super::tests_support::render(&mut app, 120, 40);
        assert!(output.contains("syswatch"));
    }

    #[test]
    fn renders_every_view() {
        let mut app = app();
        for view in View::ALL {
            app.view = view;
            let output = super::tests_support::render(&mut app, 120, 40);
            assert!(!output.trim().is_empty(), "{view:?} rendered nothing");
        }
    }

    #[test]
    fn small_terminals_show_a_hint_instead_of_panicking() {
        let mut app = app();
        let output = super::tests_support::render(&mut app, 20, 5);
        assert!(output.to_lowercase().contains("too small"), "got: {output}");
    }

    #[test]
    fn one_by_one_terminal_does_not_panic() {
        let mut app = app();
        let _ = super::tests_support::render(&mut app, 1, 1);
    }

    #[test]
    fn dialogs_render_over_the_dashboard() {
        let mut app = app();
        app.dialog = Some(crate::app::Dialog::Help);
        let output = super::tests_support::render(&mut app, 120, 40);
        assert!(output.contains("Keyboard"));
    }
}
