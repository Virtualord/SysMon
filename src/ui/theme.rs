//! Colour palettes and shared widget styling.
//!
//! Themes are plain data: the render code asks for a [`Theme`] and gets a [`Style`].
//! Monochrome maps every role onto the terminal's default colours, which is what makes
//! `--no-color` and `theme = "monochrome"` render correctly on a 16 colour terminal.

use ratatui::style::{Color, Modifier, Style};

use crate::config::Theme as ThemeSetting;

/// A resolved colour palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Base text colour.
    pub text: Color,
    /// Dimmed text for labels and units.
    pub dim: Color,
    /// Panel background.
    pub background: Color,
    /// Panel border.
    pub border: Color,
    /// Border of the panel that has keyboard focus.
    pub border_focused: Color,
    /// Panel title.
    pub title: Color,
    /// Title of the active tab.
    pub title_active: Color,
    /// Accent used for bars and sparklines.
    pub primary: Color,
    /// Secondary accent, used for the second graph of a pair.
    pub secondary: Color,
    /// Colour for healthy resource usage.
    pub ok: Color,
    /// Colour for elevated resource usage.
    pub warn: Color,
    /// Colour for critical resource usage.
    pub critical: Color,
    /// Background of the selected table row.
    pub selection: Color,
    /// Foreground of the selected table row.
    pub selection_text: Color,
    /// Colour of the search prompt.
    pub accent: Color,
    /// Colour used for error messages.
    pub error: Color,
}

impl Theme {
    /// Resolves a configuration theme into a palette.
    pub fn from_setting(setting: ThemeSetting) -> Self {
        match setting {
            ThemeSetting::Dark => Self::dark(),
            ThemeSetting::Light => Self::light(),
            ThemeSetting::Monochrome => Self::monochrome(),
        }
    }

    /// The dark palette, the default.
    pub const fn dark() -> Self {
        Self {
            text: Color::Rgb(0xD8, 0xDE, 0xE9),
            dim: Color::Rgb(0x7A, 0x84, 0x94),
            background: Color::Rgb(0x1C, 0x20, 0x28),
            border: Color::Rgb(0x3E, 0x46, 0x54),
            border_focused: Color::Rgb(0x61, 0xAF, 0xEF),
            title: Color::Rgb(0xAB, 0xB2, 0xBF),
            title_active: Color::Rgb(0x98, 0xC3, 0x79),
            primary: Color::Rgb(0x61, 0xAF, 0xEF),
            secondary: Color::Rgb(0xE5, 0xC0, 0x7B),
            ok: Color::Rgb(0x98, 0xC3, 0x79),
            warn: Color::Rgb(0xE5, 0xC0, 0x7B),
            critical: Color::Rgb(0xE0, 0x6C, 0x75),
            selection: Color::Rgb(0x2E, 0x37, 0x41),
            selection_text: Color::Rgb(0xFF, 0xFF, 0xFF),
            accent: Color::Rgb(0x56, 0xB6, 0xC2),
            error: Color::Rgb(0xE0, 0x6C, 0x75),
        }
    }

    /// The light palette.
    pub const fn light() -> Self {
        Self {
            text: Color::Rgb(0x2E, 0x34, 0x40),
            dim: Color::Rgb(0x6B, 0x72, 0x80),
            background: Color::Rgb(0xFA, 0xFA, 0xFA),
            border: Color::Rgb(0xC0, 0xC4, 0xCC),
            border_focused: Color::Rgb(0x1A, 0x5F, 0xB4),
            title: Color::Rgb(0x4E, 0x55, 0x60),
            title_active: Color::Rgb(0x2F, 0x6F, 0x2F),
            primary: Color::Rgb(0x1A, 0x5F, 0xB4),
            secondary: Color::Rgb(0xA0, 0x62, 0x00),
            ok: Color::Rgb(0x2F, 0x6F, 0x2F),
            warn: Color::Rgb(0xA0, 0x62, 0x00),
            critical: Color::Rgb(0xA1, 0x1B, 0x1B),
            selection: Color::Rgb(0xC8, 0xDC, 0xF0),
            selection_text: Color::Rgb(0x10, 0x10, 0x10),
            accent: Color::Rgb(0x0B, 0x63, 0x70),
            error: Color::Rgb(0xA1, 0x1B, 0x1B),
        }
    }

    /// A palette that only uses the terminal's own 16 colours.
    pub const fn monochrome() -> Self {
        Self {
            text: Color::Reset,
            dim: Color::DarkGray,
            background: Color::Reset,
            border: Color::Gray,
            border_focused: Color::White,
            title: Color::White,
            title_active: Color::White,
            primary: Color::White,
            secondary: Color::Gray,
            ok: Color::White,
            warn: Color::Gray,
            critical: Color::White,
            selection: Color::Rgb(0x30, 0x30, 0x30),
            selection_text: Color::White,
            accent: Color::Cyan,
            error: Color::Red,
        }
    }

    /// The colour for a usage percentage, following the green/amber/red convention.
    ///
    /// The thresholds are deliberately conservative: 90% sustained use is normal on a
    /// busy machine, and only the last 10% is worth flagging.
    pub fn usage_color(&self, percent: f64) -> Color {
        if percent >= 90.0 {
            self.critical
        } else if percent >= 70.0 {
            self.warn
        } else {
            self.ok
        }
    }

    /// Style for a usage percentage.
    pub fn usage_style(&self, percent: f64) -> Style {
        Style::default().fg(self.usage_color(percent))
    }

    /// Style for dimmed secondary text.
    pub fn dim(&self) -> Style {
        Style::default().fg(self.dim)
    }

    /// Style for normal text.
    pub fn text(&self) -> Style {
        Style::default().fg(self.text)
    }

    /// Style for a panel title.
    pub fn title(&self) -> Style {
        Style::default().fg(self.title).add_modifier(Modifier::BOLD)
    }

    /// Style for the title of the active tab.
    pub fn title_active(&self) -> Style {
        Style::default()
            .fg(self.title_active)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }

    /// Style for the selected table row.
    pub fn selection(&self) -> Style {
        Style::default().bg(self.selection).fg(self.selection_text)
    }

    /// Style for an error message.
    pub fn error(&self) -> Style {
        Style::default().fg(self.error).add_modifier(Modifier::BOLD)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_setting(ThemeSetting::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_resolve_to_distinct_palettes() {
        let dark = Theme::from_setting(ThemeSetting::Dark);
        let light = Theme::from_setting(ThemeSetting::Light);
        let mono = Theme::from_setting(ThemeSetting::Monochrome);
        assert_ne!(dark, light);
        assert_ne!(dark, mono);
        assert_ne!(light, mono);
    }

    #[test]
    fn default_theme_is_dark() {
        assert_eq!(Theme::default(), Theme::dark());
    }

    #[test]
    fn usage_colour_follows_the_thresholds() {
        let theme = Theme::dark();
        assert_eq!(theme.usage_color(0.0), theme.ok);
        assert_eq!(theme.usage_color(69.9), theme.ok);
        assert_eq!(theme.usage_color(70.0), theme.warn);
        assert_eq!(theme.usage_color(89.9), theme.warn);
        assert_eq!(theme.usage_color(90.0), theme.critical);
        assert_eq!(theme.usage_color(100.0), theme.critical);
    }

    #[test]
    fn styles_carry_the_expected_modifiers() {
        let theme = Theme::dark();
        assert!(theme.title().add_modifier.contains(Modifier::BOLD));
        assert!(
            theme
                .title_active()
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        assert!(theme.error().add_modifier.contains(Modifier::BOLD));
        assert_eq!(theme.selection().bg, Some(theme.selection));
    }
}
