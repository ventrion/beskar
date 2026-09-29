//! Timestamps in the one format Beskar writes to disk: `2026-09-29T10:15:00Z`.

use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Error;

/// A moment in time, in whole seconds since the Unix epoch, always UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    /// The current time.
    pub fn now() -> Timestamp {
        let seconds = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(after) => after.as_secs() as i64,
            Err(before) => -(before.duration().as_secs() as i64),
        };
        Timestamp(seconds)
    }

    /// A timestamp from seconds since the Unix epoch.
    pub fn from_secs(seconds: i64) -> Timestamp {
        Timestamp(seconds)
    }

    /// Seconds since the Unix epoch.
    pub fn secs(self) -> i64 {
        self.0
    }

    /// How long ago this was, relative to `now`, in words such as "3 hours ago".
    pub fn ago(self, now: Timestamp) -> String {
        let seconds = now.0 - self.0;
        if seconds < 0 {
            return "in the future".to_string();
        }
        let (count, unit) = match seconds {
            0..=59 => return "just now".to_string(),
            60..=3599 => (seconds / 60, "minute"),
            3600..=86_399 => (seconds / 3600, "hour"),
            86_400..=2_591_999 => (seconds / 86_400, "day"),
            2_592_000..=31_535_999 => (seconds / 2_592_000, "month"),
            _ => (seconds / 31_536_000, "year"),
        };
        format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let days = self.0.div_euclid(86_400);
        let rest = self.0.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        write!(
            f,
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            rest / 3600,
            rest % 3600 / 60,
            rest % 60
        )
    }
}

impl FromStr for Timestamp {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        let bad = || {
            Error::invalid(format!(
                "invalid timestamp '{text}': expected the form 2026-09-29T10:15:00Z"
            ))
        };
        let bytes = text.as_bytes();
        if bytes.len() != 20
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || bytes[10] != b'T'
            || bytes[13] != b':'
            || bytes[16] != b':'
            || bytes[19] != b'Z'
        {
            return Err(bad());
        }
        let number = |from: usize, to: usize| -> Result<i64, Error> {
            let digits = &text[from..to];
            if digits.bytes().all(|b| b.is_ascii_digit()) {
                digits.parse().map_err(|_| bad())
            } else {
                Err(bad())
            }
        };
        let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
        let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
        if !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return Err(bad());
        }
        Ok(Timestamp(
            days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second,
        ))
    }
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date for a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_moments() {
        for (seconds, text) in [
            (0, "1970-01-01T00:00:00Z"),
            (86_399, "1970-01-01T23:59:59Z"),
            (951_782_400, "2000-02-29T00:00:00Z"),
            (1_000_000_000, "2001-09-09T01:46:40Z"),
            (1_785_320_100, "2026-07-29T10:15:00Z"),
            (2_147_483_647, "2038-01-19T03:14:07Z"),
            (4_102_444_800, "2100-01-01T00:00:00Z"),
            (-1, "1969-12-31T23:59:59Z"),
        ] {
            assert_eq!(Timestamp::from_secs(seconds).to_string(), text);
            assert_eq!(text.parse::<Timestamp>().unwrap().secs(), seconds);
        }
    }

    #[test]
    fn round_trips_across_many_days() {
        let mut seconds = -5_000_000_000i64;
        while seconds < 5_000_000_000 {
            let stamp = Timestamp::from_secs(seconds);
            assert_eq!(stamp.to_string().parse::<Timestamp>().unwrap(), stamp);
            seconds += 86_399 * 7 + 13;
        }
    }

    #[test]
    fn rejects_malformed_and_impossible_times() {
        for bad in [
            "",
            "2026-09-29",
            "2026-09-29 10:15:00Z",
            "2026-09-29T10:15:00",
            "2026-13-01T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2025-02-29T00:00:00Z",
            "2026-09-29T24:00:00Z",
            "2026-09-29T10:60:00Z",
            "2026-09-29T10:15:60Z",
            "+026-09-29T10:15:00Z",
            "2026-09-2９T10:15:00Z",
        ] {
            assert!(bad.parse::<Timestamp>().is_err(), "{bad}");
        }
        assert!("2024-02-29T00:00:00Z".parse::<Timestamp>().is_ok());
        assert!("2000-02-29T00:00:00Z".parse::<Timestamp>().is_ok());
        assert!("1900-02-29T00:00:00Z".parse::<Timestamp>().is_err());
    }

    #[test]
    fn describes_elapsed_time() {
        let now = Timestamp::from_secs(10_000_000);
        let ago = |seconds: i64| Timestamp::from_secs(10_000_000 - seconds).ago(now);
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(3 * 3600), "3 hours ago");
        assert_eq!(ago(86_400), "1 day ago");
        assert_eq!(ago(40 * 86_400), "1 month ago");
        assert_eq!(ago(800 * 86_400), "2 years ago");
        assert_eq!(Timestamp::from_secs(10_000_100).ago(now), "in the future");
    }

    #[test]
    fn now_is_after_2025() {
        assert!(Timestamp::now().secs() > 1_735_689_600);
    }
}
