//! Time and unit helpers.
//!
//! Deliberately dependency-free: the engine only needs Unix milliseconds, a
//! `YYYY-MM-DD` formatter for the DSL round-trip, and a handful of unit
//! parsers. Pulling in a full date-time crate for that would be wasteful in a
//! tool that advertises a sub-100 MB footprint.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MS_PER_SEC: i64 = 1_000;
const MS_PER_MIN: i64 = 60 * MS_PER_SEC;
const MS_PER_HOUR: i64 = 60 * MS_PER_MIN;
const MS_PER_DAY: i64 = 24 * MS_PER_HOUR;

/// Current wall-clock time as Unix milliseconds.
#[must_use]
pub fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(delta) => i64::try_from(delta.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

/// Convert Unix milliseconds to whole days since the Unix epoch.
#[must_use]
pub const fn unix_ms_to_days(ms: i64) -> i64 {
    if ms >= 0 {
        ms / MS_PER_DAY
    } else {
        (ms - (MS_PER_DAY - 1)) / MS_PER_DAY
    }
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
#[must_use]
pub fn days_to_civil(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let mp = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Days since 1970-01-01 for a civil date.
#[must_use]
pub const fn civil_to_days(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 } as i64;
    let day_of_year = (153 * mp + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Format Unix milliseconds as `YYYY-MM-DD` in UTC.
#[must_use]
pub fn format_unix_ms(ms: i64) -> String {
    let days = unix_ms_to_days(ms);
    let (year, month, day) = days_to_civil(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Format Unix milliseconds as `YYYY-MM-DD HH:MM:SS` in UTC.
#[must_use]
pub fn format_unix_ms_full(ms: i64) -> String {
    let clamped = ms.max(0);
    let days = unix_ms_to_days(clamped);
    let (year, month, day) = days_to_civil(days);
    let rem = clamped.rem_euclid(MS_PER_DAY);
    let hour = rem / MS_PER_HOUR;
    let minute = (rem % MS_PER_HOUR) / MS_PER_MIN;
    let second = (rem % MS_PER_MIN) / MS_PER_SEC;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Parse `YYYY-MM-DD` (also accepts `YYYY/MM/DD` and `YYYY-MM`) to Unix
/// milliseconds at midnight UTC.
#[must_use]
pub fn parse_date(raw: &str) -> Option<i64> {
    let normalized = raw.trim().replace('/', "-");
    let mut parts = normalized.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next().unwrap_or("1").parse().ok()?;
    let day: u32 = parts.next().unwrap_or("1").parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(civil_to_days(year, month, day) * MS_PER_DAY)
}

/// Render a duration in the most compact DSL-friendly unit.
#[must_use]
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    // Days only once the value is at least two of them, so a one day window
    // stays spelled `24h` the way the documented DSL writes it.
    if secs % 86_400 == 0 && secs >= 2 * 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs % 3_600 == 0 && secs >= 3_600 {
        format!("{}h", secs / 3_600)
    } else if secs % 60 == 0 && secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// Parse a compact duration such as `30s`, `15m`, `24h`, `7d` or `2w`.
#[must_use]
pub fn parse_duration(raw: &str) -> Option<Duration> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let split_at = trimmed
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split_at);
    if number.is_empty() {
        return None;
    }
    let value: f64 = number.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
        "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600.0,
        "d" | "day" | "days" => 86_400.0,
        "w" | "week" | "weeks" => 604_800.0,
        _ => return None,
    };
    let seconds = value * multiplier;
    if seconds > (u64::MAX / 2) as f64 {
        return None;
    }
    Some(Duration::from_secs_f64(seconds))
}

const SIZE_UNITS: [(&str, u64); 6] = [
    ("tb", 1_099_511_627_776),
    ("gb", 1_073_741_824),
    ("mb", 1_048_576),
    ("kb", 1_024),
    ("k", 1_024),
    ("b", 1),
];

/// Parse a size such as `512`, `64kb`, `10mb`, `2gb`.
#[must_use]
pub fn parse_size(raw: &str) -> Option<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let split_at = trimmed
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split_at);
    if number.is_empty() {
        return None;
    }
    let value: f64 = number.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let unit = unit.trim().to_ascii_lowercase();
    let multiplier = if unit.is_empty() || unit == "b" || unit == "byte" || unit == "bytes" {
        1
    } else {
        SIZE_UNITS
            .iter()
            .find(|(suffix, _)| *suffix == unit)
            .map(|(_, m)| *m)?
    };
    let bytes = value * multiplier as f64;
    if bytes > u64::MAX as f64 / 2.0 {
        return None;
    }
    Some(bytes.round() as u64)
}

/// Render a byte count in the most compact DSL-friendly unit.
#[must_use]
pub fn format_size(bytes: u64) -> String {
    for (suffix, multiplier) in SIZE_UNITS {
        if suffix != "b" && suffix != "k" && bytes >= multiplier && bytes % multiplier == 0 {
            return format!("{}{}", bytes / multiplier, suffix);
        }
    }
    format!("{bytes}b")
}

/// Render a byte count the way a user expects to read it.
#[must_use]
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
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

/// Milliseconds since the Unix epoch for `duration` ago.
#[must_use]
pub fn cutoff_ms(duration: Duration) -> i64 {
    let millis = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
    now_ms().saturating_sub(millis)
}
