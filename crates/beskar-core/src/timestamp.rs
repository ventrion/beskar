//! UTC timestamps in RFC 3339 form (`2026-09-29T10:15:03Z`), without a
//! date library.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch, UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(i64);

impl Timestamp {
    pub fn now() -> Self {
        let seconds = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(elapsed) => elapsed.as_secs() as i64,
            Err(before) => -(before.duration().as_secs() as i64),
        };
        Timestamp(seconds)
    }

    pub fn from_unix(seconds: i64) -> Self {
        Timestamp(seconds)
    }

    pub fn unix(self) -> i64 {
        self.0
    }

    /// Parse `YYYY-MM-DDTHH:MM:SSZ`.
    pub fn parse(text: &str) -> Option<Self> {
        let b = text.as_bytes();
        if b.len() != 20
            || b[4] != b'-'
            || b[7] != b'-'
            || b[10] != b'T'
            || b[13] != b':'
            || b[16] != b':'
            || b[19] != b'Z'
        {
            return None;
        }
        let number = |range: std::ops::Range<usize>| -> Option<i64> {
            let digits = &text[range];
            digits
                .bytes()
                .all(|d| d.is_ascii_digit())
                .then(|| digits.parse().ok())?
        };
        let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
        let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
        if !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return None;
        }
        let days = days_from_civil(year, month, day);
        Some(Timestamp(
            days * 86_400 + hour * 3600 + minute * 60 + second,
        ))
    }

    /// The date part, `YYYY-MM-DD`.
    pub fn date(self) -> String {
        let (year, month, day) = civil_from_days(self.0.div_euclid(86_400));
        format!("{year:04}-{month:02}-{day:02}")
    }

    /// How long before `now` this was, in words: "just now", "5 minutes
    /// ago", "3 days ago", or the date for anything older than 30 days.
    pub fn ago(self, now: Timestamp) -> String {
        let seconds = now.0 - self.0;
        let plural =
            |n: i64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
        match seconds {
            s if s < 0 => format!("on {}", self.date()),
            0..=59 => "just now".to_string(),
            60..=3599 => plural(seconds / 60, "minute"),
            3600..=86_399 => plural(seconds / 3600, "hour"),
            86_400..=2_591_999 => plural(seconds / 86_400, "day"),
            _ => format!("on {}", self.date()),
        }
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seconds = self.0.rem_euclid(86_400);
        write!(
            f,
            "{}T{:02}:{:02}:{:02}Z",
            self.date(),
            seconds / 3600,
            seconds % 3600 / 60,
            seconds % 60
        )
    }
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        assert_eq!(Timestamp::from_unix(0).to_string(), "1970-01-01T00:00:00Z");
        assert_eq!(
            Timestamp::from_unix(951_782_400).to_string(),
            "2000-02-29T00:00:00Z"
        );
        assert_eq!(
            Timestamp::from_unix(1_790_676_903).to_string(),
            "2026-09-29T10:15:03Z"
        );
        assert_eq!(Timestamp::from_unix(-1).to_string(), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn parses_what_it_formats() {
        for seconds in [
            0,
            1,
            59,
            86_399,
            86_400,
            951_782_400,
            1_790_676_903,
            4_102_444_800,
            -86_401,
        ] {
            let stamp = Timestamp::from_unix(seconds);
            assert_eq!(Timestamp::parse(&stamp.to_string()), Some(stamp));
        }
    }

    #[test]
    fn rejects_malformed_timestamps() {
        for text in [
            "",
            "2026-09-29",
            "2026-09-29 10:15:03Z",
            "2026-13-01T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2025-02-29T00:00:00Z",
            "2026-09-29T24:00:00Z",
            "2026-09-29T10:15:03+02:00",
            "2026-0a-29T10:15:03Z",
        ] {
            assert_eq!(Timestamp::parse(text), None, "{text}");
        }
        assert!(Timestamp::parse("2024-02-29T00:00:00Z").is_some());
    }

    #[test]
    fn describes_elapsed_time() {
        let now = Timestamp::from_unix(1_000_000_000);
        let ago = |s: i64| Timestamp::from_unix(1_000_000_000 - s).ago(now);
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(7200), "2 hours ago");
        assert_eq!(ago(86_400 * 3), "3 days ago");
        assert_eq!(ago(86_400 * 40), "on 2001-07-31");
    }
}
