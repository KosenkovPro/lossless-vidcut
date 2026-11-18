//! Small formatting helpers shared by the user interface.

/// Format a number of seconds as `mm:ss` or `h:mm:ss`.
pub fn format_timecode(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "00:00".to_string();
    }
    let total = seconds.round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let secs = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes:02}:{secs:02}")
    }
}

/// Format a number of seconds as `mm:ss.mmm`, which is what a frame accurate
/// editor needs.
pub fn format_timecode_ms(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "00:00.000".to_string();
    }
    let total_millis = (seconds * 1000.0).round() as u64;
    let millis = total_millis % 1000;
    let total_seconds = total_millis / 1000;
    let minutes = total_seconds / 60;
    let secs = total_seconds % 60;
    format!("{minutes:02}:{secs:02}.{millis:03}")
}

/// Format a byte count in a human readable way.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Format a frame rate.
pub fn format_fps(fps: f64) -> String {
    if fps <= 0.0 {
        "—".to_string()
    } else if (fps - fps.round()).abs() < 0.01 {
        format!("{:.0} fps", fps.round())
    } else {
        format!("{fps:.2} fps")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_timecodes() {
        assert_eq!(format_timecode(0.0), "00:00");
        assert_eq!(format_timecode(61.4), "01:01");
        assert_eq!(format_timecode(3661.0), "1:01:01");
        assert_eq!(format_timecode(-5.0), "00:00");
    }

    #[test]
    fn formats_millisecond_timecodes() {
        assert_eq!(format_timecode_ms(0.0), "00:00.000");
        assert_eq!(format_timecode_ms(1.5), "00:01.500");
        assert_eq!(format_timecode_ms(61.234), "01:01.234");
    }

    #[test]
    fn formats_byte_counts() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KiB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MiB");
    }

    #[test]
    fn formats_frame_rates() {
        assert_eq!(format_fps(25.0), "25 fps");
        assert_eq!(format_fps(29.97), "29.97 fps");
        assert_eq!(format_fps(0.0), "—");
    }
}
