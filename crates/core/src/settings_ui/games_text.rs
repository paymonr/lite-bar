//! Text for the Games page: play time, FPS and when a game was last played.

use crate::history::LocalTime;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `H:MM`, hours unbounded; `0:00` for a negative or invalid time.
pub(crate) fn played_short(secs: f64) -> String {
    // Negative and NaN become 0 in the cast; infinity would saturate, so it is zeroed too.
    let mins = if secs.is_finite() {
        (secs / 60.0) as u64
    } else {
        0
    };
    format!("{}:{:02}", mins / 60, mins % 60)
}

/// Whole FPS, or `—` when there is no value.
pub(crate) fn fps_whole(v: Option<f64>) -> String {
    match v {
        Some(v) if v.is_finite() => format!("{}", v.round() as i64),
        _ => "—".to_string(),
    }
}

/// `Today 12:18`, `Yesterday 21:04`, `Oct 27` (this year) or `Oct 27, 2025`. A time after
/// `now` (the clock went back) always shows its date.
pub(crate) fn last_played_text(when: LocalTime, now: LocalTime) -> String {
    let clock = format!("{:02}:{:02}", when.hour, when.minute);
    let days_ago = day_number(now) - day_number(when);
    if when <= now && days_ago == 0 {
        return format!("Today {clock}");
    }
    if when <= now && days_ago == 1 {
        return format!("Yesterday {clock}");
    }
    let month = MONTHS[usize::from(when.month.clamp(1, 12)) - 1];
    if when.year == now.year {
        format!("{month} {}", when.day)
    } else {
        format!("{month} {}, {}", when.day, when.year)
    }
}

/// Days since 1970-01-01 of `t`'s date (Howard Hinnant's `days_from_civil`).
fn day_number(t: LocalTime) -> i64 {
    let (m, d) = (i64::from(t.month), i64::from(t.day));
    let y = i64::from(t.year) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> LocalTime {
        LocalTime {
            year,
            month,
            day,
            hour,
            minute,
            second: 0,
        }
    }

    #[test]
    fn played_is_hours_and_minutes() {
        assert_eq!(played_short(49_906.0), "13:51");
        assert_eq!(played_short(150.0), "0:02");
        assert_eq!(played_short(-5.0), "0:00");
        assert_eq!(played_short(f64::NAN), "0:00");
        assert_eq!(played_short(360_000.0), "100:00");
    }

    #[test]
    fn fps_is_whole_or_a_dash() {
        assert_eq!(fps_whole(Some(135.8)), "136");
        assert_eq!(fps_whole(Some(66.0)), "66");
        assert_eq!(fps_whole(None), "—");
        assert_eq!(fps_whole(Some(f64::INFINITY)), "—");
    }

    #[test]
    fn last_played_today_and_yesterday() {
        let now = at(2026, 11, 3, 15, 0);
        assert_eq!(
            last_played_text(at(2026, 11, 3, 12, 18), now),
            "Today 12:18"
        );
        assert_eq!(
            last_played_text(at(2026, 11, 2, 21, 4), now),
            "Yesterday 21:04"
        );
    }

    #[test]
    fn yesterday_across_month_year_and_leap_day() {
        assert_eq!(
            last_played_text(at(2026, 12, 31, 23, 50), at(2027, 1, 1, 0, 10)),
            "Yesterday 23:50"
        );
        assert_eq!(
            last_played_text(at(2028, 2, 29, 9, 5), at(2028, 3, 1, 8, 0)),
            "Yesterday 09:05"
        );
        assert_eq!(
            last_played_text(at(2026, 10, 31, 9, 5), at(2026, 11, 1, 8, 0)),
            "Yesterday 09:05"
        );
    }

    #[test]
    fn older_days_show_the_date() {
        let now = at(2026, 11, 3, 15, 0);
        assert_eq!(last_played_text(at(2026, 10, 27, 14, 4), now), "Oct 27");
        assert_eq!(
            last_played_text(at(2025, 10, 27, 14, 4), now),
            "Oct 27, 2025"
        );
    }

    #[test]
    fn a_time_after_now_shows_its_date() {
        let now = at(2026, 11, 3, 15, 0);
        assert_eq!(last_played_text(at(2026, 11, 4, 9, 0), now), "Nov 4");
        assert_eq!(last_played_text(at(2026, 11, 3, 16, 0), now), "Nov 3");
    }
}
