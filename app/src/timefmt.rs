//! Dates as the grid shows them. UTC for now: the local zone arrives with the settings work.

use mail_core::Timestamp;

const DAY: i64 = 86_400;
const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"]; // 1970-01-01 was a Thursday
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// (year, month 1–12, day 1–31) of a day count since the epoch (Howard Hinnant's civil-from-days).
fn civil(days: i64) -> (i64, usize, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month as usize, day)
}

fn weekday(days: i64) -> &'static str { WEEKDAYS[days.rem_euclid(7) as usize] }

pub fn clock(t: Timestamp) -> String {
    let secs = t.rem_euclid(DAY);
    format!("{:02}:{:02}", secs / 3_600, secs % 3_600 / 60)
}

/// The list's time column (six cells at most): the time today, the weekday this week, the day
/// this year (`9 Oct`), else the month and year (`Oct 25`).
pub fn short(t: Timestamp, now: Timestamp) -> String {
    let (day, today) = (t.div_euclid(DAY), now.div_euclid(DAY));
    if day == today { return clock(t); }
    if day < today && today - day < 7 { return weekday(day).to_owned(); }
    let (year, month, d) = civil(day);
    if year == civil(today).0 { format!("{d} {}", MONTHS[month - 1]) } else { format!("{} {:02}", MONTHS[month - 1], year.rem_euclid(100)) }
}

/// The reader's Date header: `Fri 13 Nov 2026 10:40`.
pub fn long(t: Timestamp) -> String {
    let day = t.div_euclid(DAY);
    let (year, month, d) = civil(day);
    format!("{} {d} {} {year} {}", weekday(day), MONTHS[month - 1], clock(t))
}

/// Sizes as attachment chips show them: `2.1 MB`, `840 KB`.
pub fn bytes(n: u64) -> String {
    if n >= 1_000_000 { format!("{:.1} MB", n as f64 / 1e6) } else if n >= 1_000 { format!("{} KB", n / 1_000) } else { format!("{n} B") }
}
