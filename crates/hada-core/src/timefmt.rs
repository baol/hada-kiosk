//! Minimal RFC 3339 parsing and relative-time formatting without a date crate.

use std::time::{SystemTime, UNIX_EPOCH};

/// Parse `YYYY-MM-DDTHH:MM:SS[.frac][Z|±HH:MM]` into Unix seconds.
pub fn parse_rfc3339(s: &str) -> Option<f64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { s.get(from..to)?.parse().ok() };
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    if b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') {
        return None;
    }
    let hour = num(11, 13)?;
    let minute = num(14, 16)?;
    let second = num(17, 19)?;
    let mut i = 19;
    let mut frac = 0.0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        let mut end = start;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        frac = format!("0.{}", &s[start..end]).parse().unwrap_or(0.0);
        i = end;
    }
    let offset: i64 = match b.get(i) {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let oh = num(i + 1, i + 3)?;
            let om = if b.get(i + 3) == Some(&b':') {
                num(i + 4, i + 6)?
            } else {
                num(i + 3, i + 5).unwrap_or(0)
            };
            let secs = oh * 3600 + om * 60;
            if *sign == b'+' {
                secs
            } else {
                -secs
            }
        }
        _ => return None,
    };
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    Some(secs as f64 + frac)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Current time in Unix seconds. `HADA_NOW` (Unix seconds) pins the clock,
/// which makes screenshots with relative times and calendars reproducible.
pub fn now_unix() -> f64 {
    static FIXED: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    if let Some(t) = FIXED.get_or_init(|| {
        std::env::var("HADA_NOW")
            .ok()
            .and_then(|v| v.trim().parse().ok())
    }) {
        return *t;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// "3 d 4 h", "12 min", "45 s", or "in 2 h" for future timestamps.
pub fn relative(ts: f64, now: f64) -> String {
    let delta = now - ts;
    let future = delta < 0.0;
    let mut secs = delta.abs().round() as u64;
    let days = secs / 86_400;
    secs %= 86_400;
    let hours = secs / 3600;
    secs %= 3600;
    let mins = secs / 60;
    secs %= 60;
    let body = if days > 0 {
        format!("{days} d {hours} h")
    } else if hours > 0 {
        format!("{hours} h {mins} min")
    } else if mins > 0 {
        format!("{mins} min")
    } else {
        format!("{secs} s")
    };
    if future {
        format!("in {body}")
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_variants() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_rfc3339("1970-01-02T00:00:00+00:00"), Some(86_400.0));
        assert_eq!(
            parse_rfc3339("2024-03-10T12:34:56.5+02:00"),
            Some(1_710_066_896.5)
        );
        assert_eq!(parse_rfc3339("2024-03-10 12:34:56"), Some(1_710_074_096.0));
        assert_eq!(parse_rfc3339("garbage"), None);
    }

    #[test]
    fn relative_strings() {
        assert_eq!(relative(0.0, 45.0), "45 s");
        assert_eq!(relative(0.0, 125.0), "2 min");
        assert_eq!(relative(0.0, 3_700.0), "1 h 1 min");
        assert_eq!(relative(0.0, 90_000.0), "1 d 1 h");
        assert_eq!(relative(7_200.0, 0.0), "in 2 h 0 min");
    }
}

/// Wall-clock date/time as written in a Home Assistant ISO string (local to HA).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WallClock {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    /// `false` for date-only strings (all-day events).
    pub has_time: bool,
}

impl WallClock {
    /// Parse `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM[:SS][offset]`, keeping the wall-clock fields.
    pub fn parse(s: &str) -> Option<WallClock> {
        let s = s.trim();
        let b = s.as_bytes();
        if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let num = |from: usize, to: usize| -> Option<i64> { s.get(from..to)?.parse().ok() };
        let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
        if b.len() >= 16 && (b[10] == b'T' || b[10] == b' ') {
            Some(WallClock {
                year,
                month,
                day,
                hour: num(11, 13)?,
                minute: num(14, 16)?,
                has_time: true,
            })
        } else {
            Some(WallClock {
                year,
                month,
                day,
                hour: 0,
                minute: 0,
                has_time: false,
            })
        }
    }

    /// From Unix seconds plus a UTC offset in seconds.
    pub fn from_unix(ts: f64, offset_secs: i64) -> WallClock {
        let secs = ts.floor() as i64 + offset_secs;
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        WallClock {
            year,
            month,
            day,
            hour: rem / 3600,
            minute: (rem % 3600) / 60,
            has_time: true,
        }
    }

    pub fn days(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    pub fn same_day(&self, other: &WallClock) -> bool {
        (self.year, self.month, self.day) == (other.year, other.month, other.day)
    }

    /// "Mon 10 Nov"
    pub fn day_label(&self) -> String {
        format!(
            "{} {} {}",
            weekday_name(self.days()),
            self.day,
            MONTHS[(self.month.clamp(1, 12) - 1) as usize]
        )
    }

    /// "09:05"
    pub fn time_label(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }
}

/// UTC offset (seconds) written at the end of an ISO string, if any.
pub fn parse_offset(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.ends_with('Z') || s.ends_with('z') {
        return Some(0);
    }
    if s.len() < 25 {
        return None;
    }
    let tail = &s[s.len() - 6..];
    let sign = match tail.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let h: i64 = tail[1..3].parse().ok()?;
    let m: i64 = tail[4..6].parse().ok()?;
    Some(sign * (h * 3600 + m * 60))
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

pub fn weekday_name(days_since_epoch: i64) -> &'static str {
    // 1970-01-01 was a Thursday.
    WEEKDAYS[((days_since_epoch + 4).rem_euclid(7)) as usize]
}

/// Inverse of `days_from_civil` (Howard Hinnant).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod wallclock_tests {
    use super::*;

    #[test]
    fn wall_clock() {
        let w = WallClock::parse("2025-11-10T09:05:00+01:00").unwrap();
        assert_eq!(
            (w.year, w.month, w.day, w.hour, w.minute, w.has_time),
            (2025, 11, 10, 9, 5, true)
        );
        assert_eq!(w.day_label(), "Mon 10 Nov");
        assert_eq!(w.time_label(), "09:05");
        let d = WallClock::parse("2025-11-12").unwrap();
        assert!(!d.has_time);
        assert_eq!(d.day_label(), "Wed 12 Nov");
        assert_eq!(parse_offset("2025-11-10T09:05:00+01:00"), Some(3600));
        assert_eq!(parse_offset("2025-11-10T09:05:00-05:30"), Some(-19_800));
        assert_eq!(parse_offset("2025-11-10"), None);
        let u = WallClock::from_unix(0.0, 3600);
        assert_eq!((u.year, u.month, u.day, u.hour), (1970, 1, 1, 1));
        let x = WallClock::from_unix(1_762_700_000.0, 0);
        assert_eq!((x.year, x.month, x.day), (2025, 11, 9));
        assert_eq!(weekday_name(0), "Thu");
    }
}

/// Calendar vocabulary for the UI language (Home Assistant translations do
/// not cover day/month names; the frontend uses the browser's Intl for them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locale {
    pub code: &'static str,
    pub weekdays: [&'static str; 7],
    pub weekdays_short: [&'static str; 7],
    pub months: [&'static str; 12],
    pub months_short: [&'static str; 12],
    pub today: &'static str,
    pub tomorrow: &'static str,
    pub all_day: &'static str,
    pub no_events: &'static str,
    pub loading: &'static str,
    /// Day-month-year order; `false` = month-day-year (en-US).
    pub dmy: bool,
}

pub const LOCALE_EN: Locale = Locale {
    code: "en",
    weekdays: [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ],
    weekdays_short: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
    months: [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
    months_short: [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    today: "Today",
    tomorrow: "Tomorrow",
    all_day: "All day",
    no_events: "No upcoming events",
    loading: "Loading…",
    dmy: false,
};

pub const LOCALE_IT: Locale = Locale {
    code: "it",
    weekdays: [
        "domenica",
        "lunedì",
        "martedì",
        "mercoledì",
        "giovedì",
        "venerdì",
        "sabato",
    ],
    weekdays_short: ["dom", "lun", "mar", "mer", "gio", "ven", "sab"],
    months: [
        "gennaio",
        "febbraio",
        "marzo",
        "aprile",
        "maggio",
        "giugno",
        "luglio",
        "agosto",
        "settembre",
        "ottobre",
        "novembre",
        "dicembre",
    ],
    months_short: [
        "gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic",
    ],
    today: "Oggi",
    tomorrow: "Domani",
    all_day: "Tutto il giorno",
    no_events: "Nessun evento in programma",
    loading: "Caricamento…",
    dmy: true,
};

pub const LOCALE_DE: Locale = Locale {
    code: "de",
    weekdays: [
        "Sonntag",
        "Montag",
        "Dienstag",
        "Mittwoch",
        "Donnerstag",
        "Freitag",
        "Samstag",
    ],
    weekdays_short: ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"],
    months: [
        "Januar",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ],
    months_short: [
        "Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ],
    today: "Heute",
    tomorrow: "Morgen",
    all_day: "Ganztägig",
    no_events: "Keine anstehenden Termine",
    loading: "Lädt…",
    dmy: true,
};

pub const LOCALE_FR: Locale = Locale {
    code: "fr",
    weekdays: [
        "dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi",
    ],
    weekdays_short: ["dim", "lun", "mar", "mer", "jeu", "ven", "sam"],
    months: [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ],
    months_short: [
        "janv", "févr", "mars", "avr", "mai", "juin", "juil", "août", "sept", "oct", "nov", "déc",
    ],
    today: "Aujourd'hui",
    tomorrow: "Demain",
    all_day: "Toute la journée",
    no_events: "Aucun événement à venir",
    loading: "Chargement…",
    dmy: true,
};

pub const LOCALE_ES: Locale = Locale {
    code: "es",
    weekdays: [
        "domingo",
        "lunes",
        "martes",
        "miércoles",
        "jueves",
        "viernes",
        "sábado",
    ],
    weekdays_short: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
    months: [
        "enero",
        "febrero",
        "marzo",
        "abril",
        "mayo",
        "junio",
        "julio",
        "agosto",
        "septiembre",
        "octubre",
        "noviembre",
        "diciembre",
    ],
    months_short: [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
    ],
    today: "Hoy",
    tomorrow: "Mañana",
    all_day: "Todo el día",
    no_events: "No hay eventos próximos",
    loading: "Cargando…",
    dmy: true,
};

impl Locale {
    pub fn for_language(code: &str) -> &'static Locale {
        match code
            .split(['-', '_'])
            .next()
            .unwrap_or("en")
            .to_lowercase()
            .as_str()
        {
            "it" => &LOCALE_IT,
            "de" => &LOCALE_DE,
            "fr" => &LOCALE_FR,
            "es" => &LOCALE_ES,
            _ => &LOCALE_EN,
        }
    }

    pub fn weekday(&self, w: &WallClock) -> &'static str {
        self.weekdays[((w.days() + 4).rem_euclid(7)) as usize]
    }

    /// "9 novembre 2026" / "November 9, 2026"
    pub fn long_date(&self, w: &WallClock) -> String {
        let m = self.months[(w.month.clamp(1, 12) - 1) as usize];
        if self.dmy {
            format!("{} {} {}", w.day, m, w.year)
        } else {
            format!("{} {}, {}", m, w.day, w.year)
        }
    }

    /// "9 – 15 nov 2026" / "Nov 9 – 15, 2026" (same month) or "28 nov – 4 dic 2026".
    pub fn range_label(&self, from: &WallClock, to: &WallClock) -> String {
        let mf = self.months_short[(from.month.clamp(1, 12) - 1) as usize];
        let mt = self.months_short[(to.month.clamp(1, 12) - 1) as usize];
        if from.month == to.month && from.year == to.year {
            if self.dmy {
                format!("{} – {} {} {}", from.day, to.day, mf, to.year)
            } else {
                format!("{} {} – {}, {}", mf, from.day, to.day, to.year)
            }
        } else if self.dmy {
            format!("{} {} – {} {} {}", from.day, mf, to.day, mt, to.year)
        } else {
            format!("{} {} – {} {}, {}", mf, from.day, mt, to.day, to.year)
        }
    }
}

#[cfg(test)]
mod locale_tests {
    use super::*;

    #[test]
    fn italian_labels() {
        let it = Locale::for_language("it_IT");
        let d = WallClock::parse("2026-11-09T13:00:00+01:00").unwrap();
        assert_eq!(it.weekday(&d), "lunedì");
        assert_eq!(it.long_date(&d), "9 novembre 2026");
        let to = WallClock::parse("2026-11-15").unwrap();
        assert_eq!(it.range_label(&d, &to), "9 – 15 nov 2026");
        let en = Locale::for_language("en");
        assert_eq!(en.long_date(&d), "November 9, 2026");
        assert_eq!(en.range_label(&d, &to), "Nov 9 – 15, 2026");
        let dec = WallClock::parse("2026-12-04").unwrap();
        assert_eq!(it.range_label(&d, &dec), "9 nov – 4 dic 2026");
        assert_eq!(Locale::for_language("xx").code, "en");
    }
}
