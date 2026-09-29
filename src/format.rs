//! Human readable formatting helpers shared by the collectors and the UI.
//!
//! Everything in this module is allocation-light and free of panicking operations so
//! that it can be called from both the monitoring thread and the render path.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const BYTE_UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

/// Placeholder rendered whenever a metric is unavailable on the running system.
pub const NOT_AVAILABLE: &str = "N/A";

/// Formats a byte count using binary (IEC) units, e.g. `1.5 MiB`.
///
/// Values below 1 KiB are rendered as plain bytes so that small counters keep their
/// exact value.
pub fn bytes(value: u64) -> String {
    let mut size = value as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit < BYTE_UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", value, BYTE_UNITS[0])
    } else {
        format!("{size:.1} {}", BYTE_UNITS[unit])
    }
}

/// Formats a transfer rate in bytes per second, e.g. `1.5 MiB/s`.
pub fn rate(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second < 0.0 {
        return format!("{NOT_AVAILABLE} {}/s", BYTE_UNITS[0]);
    }
    let mut size = bytes_per_second;
    let mut unit = 0usize;
    while size >= 1024.0 && unit < BYTE_UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size:.0} {}/s", BYTE_UNITS[0])
    } else {
        format!("{size:.1} {}/s", BYTE_UNITS[unit])
    }
}

/// Formats a percentage value with one decimal place.
///
/// Non finite or out of range values are clamped, which keeps a misbehaving driver
/// from producing nonsense output.
pub fn percent(value: f64) -> String {
    let clamped = if value.is_finite() {
        value.clamp(0.0, 999.0)
    } else {
        0.0
    };
    format!("{clamped:.1}%")
}

/// Formats a CPU usage value coming from `sysinfo` (a `0.0..=100.0` float).
pub fn cpu_percent(value: f32) -> String {
    percent(f64::from(value))
}

/// Formats a load average value with two decimals.
pub fn load_average(value: f64) -> String {
    if value.is_finite() { format!("{value:.2}") } else { NOT_AVAILABLE.to_string() }
}

/// Formats a duration given in seconds as `3d 4h 12m`, `4h 12m` or `12m 03s`.
///
/// The unit pair is chosen so the string stays compact enough for narrow panels.
pub fn duration(total_seconds: u64) -> String {
    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

/// Renders an `Option` metric, falling back to [`NOT_AVAILABLE`].
pub fn optional(value: Option<String>) -> String {
    value.unwrap_or_else(|| NOT_AVAILABLE.to_string())
}

/// Shortens `text` so that it fits in `max` display columns.
///
/// Text is truncated at the end and terminated with a `~` marker so users can tell
/// that information was cut. Width is measured in terminal columns, which means
/// wide (CJK) characters are accounted for.
pub fn truncate(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max {
        return text.to_string();
    }
    if max == 1 {
        return "~".to_string();
    }
    let budget = max - 1;
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('~');
    out
}

/// Shortens `text` from the middle, keeping both the beginning and the end visible.
///
/// Executable paths are the main use case: the mount prefix and the binary name are
/// far more useful than the middle of a deeply nested path.
pub fn truncate_middle(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max {
        return text.to_string();
    }
    // One column is reserved for the ellipsis; the rest is split between the two ends.
    let keep = max - 1;
    let head_budget = keep.div_ceil(2);
    let tail_budget = keep - head_budget;

    let mut head = String::new();
    let mut head_width = 0usize;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if head_width + w > head_budget {
            break;
        }
        head.push(ch);
        head_width += w;
    }

    let mut tail_rev: Vec<char> = Vec::new();
    let mut tail_width = 0usize;
    for ch in text.chars().rev() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if tail_width + w > tail_budget {
            break;
        }
        tail_rev.push(ch);
        tail_width += w;
    }
    tail_rev.reverse();

    let mut tail: String = tail_rev.into_iter().collect();
    tail.insert(0, '~');
    format!("{head}{tail}")
}

/// Pads `text` to exactly `width` columns, truncating when it does not fit.
pub fn pad(text: &str, width: usize) -> String {
    let current = UnicodeWidthStr::width(text);
    if current > width {
        return truncate(text, width);
    }
    let mut out = text.to_string();
    out.push_str(&" ".repeat(width - current));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_uses_binary_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(bytes(3 * 1024_u64.pow(4)), "3.0 TiB");
    }

    #[test]
    fn rate_appends_unit() {
        assert_eq!(rate(0.0), "0 B/s");
        assert_eq!(rate(999.0), "999 B/s");
        assert_eq!(rate(2048.0), "2.0 KiB/s");
    }

    #[test]
    fn rate_rejects_nan_and_negative() {
        assert!(rate(f64::NAN).ends_with("B/s"));
        assert!(rate(-1.0).ends_with("B/s"));
    }

    #[test]
    fn percent_clamps_out_of_range() {
        assert_eq!(percent(0.0), "0.0%");
        assert_eq!(percent(42.36), "42.4%");
        assert_eq!(percent(f64::NAN), "0.0%");
        assert_eq!(percent(-3.0), "0.0%");
        assert_eq!(percent(1000.0), "999.0%");
    }

    #[test]
    fn duration_picks_readable_units() {
        assert_eq!(duration(9), "9s");
        assert_eq!(duration(75), "1m 15s");
        assert_eq!(duration(3_661), "1h 1m");
        assert_eq!(duration(275_400), "3d 4h 30m");
    }

    #[test]
    fn truncate_respects_display_width() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 3), "he~");
        assert_eq!(truncate("hello", 0), "");
        assert_eq!(truncate("日本語テスト", 5), "日本~");
    }

    #[test]
    fn truncate_middle_keeps_both_ends() {
        assert_eq!(truncate_middle("/usr/bin/cargo", 20), "/usr/bin/cargo");
        // 9 columns of content plus the ellipsis: head "/usr/", tail "node".
        assert_eq!(truncate_middle("/usr/local/lib/node", 10), "/usr/~node");
        // Narrow widths still fill exactly `max` columns. At two columns the head
        // wins, because the beginning of a path is more useful than its end.
        assert_eq!(truncate_middle("abcdef", 3), "a~f");
        assert_eq!(truncate_middle("abcdef", 2), "a~");
        assert_eq!(truncate_middle("abcdef", 1), "~");
        assert_eq!(truncate_middle("abcdef", 0), "");
    }

    #[test]
    fn pad_fills_to_width() {
        assert_eq!(pad("ab", 5), "ab   ");
        assert_eq!(pad("abcdef", 3), "ab~");
    }
}
