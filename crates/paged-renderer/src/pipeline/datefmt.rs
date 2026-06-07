/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! W1.18a — InDesign date-variable format-token rendering.
//!
//! `CreationDate` / `ModificationDate` / `OutputDate` text variables
//! carry a `<DateVariablePreference Format="...">` (surfaced as
//! [`TextVariable::date_format`]). InDesign uses the Unicode/ICU
//! `SimpleDateFormat` token vocabulary; the subset real exports
//! actually write is implemented here. The renderer formats a concrete
//! [`DateParts`] — never the wall clock — so output is deterministic
//! and testable (see [`crate::pipeline::DocumentClock`]).
//!
//! ## Token table (the documented subset)
//!
//! | Token   | Meaning                          | Example (2026-03-09 14:07:05) |
//! |---------|----------------------------------|-------------------------------|
//! | `yyyy`  | 4-digit year                     | `2026`                        |
//! | `yy`    | 2-digit year                     | `26`                          |
//! | `MMMM`  | full month name                  | `March`                       |
//! | `MMM`   | abbreviated month name           | `Mar`                         |
//! | `MM`    | 2-digit month (01-12)            | `03`                          |
//! | `M`     | month number (1-12)              | `3`                           |
//! | `dd`    | 2-digit day of month (01-31)     | `09`                          |
//! | `d`     | day of month (1-31)              | `9`                           |
//! | `EEEE`  | full weekday name                | `Monday`                      |
//! | `EEE`   | abbreviated weekday name         | `Mon`                         |
//! | `HH`    | 2-digit hour, 24h (00-23)        | `14`                          |
//! | `H`     | hour, 24h (0-23)                 | `14`                          |
//! | `hh`    | 2-digit hour, 12h (01-12)        | `02`                          |
//! | `h`     | hour, 12h (1-12)                 | `2`                           |
//! | `mm`    | 2-digit minute (00-59)           | `07`                          |
//! | `m`     | minute (0-59)                    | `7`                           |
//! | `ss`    | 2-digit second (00-59)           | `05`                          |
//! | `s`     | second (0-59)                    | `5`                           |
//! | `a`     | AM/PM marker                     | `PM`                          |
//!
//! Letters are matched greedily (longest run first) so `MMMM` doesn't
//! get mis-read as two `MM`. Any run of an unknown ASCII letter is
//! emitted verbatim (InDesign does the same for unsupported designators
//! — better a literal than a dropped character). Text wrapped in single
//! quotes (`'literal'`) is passed through verbatim; `''` is a literal
//! apostrophe. Punctuation and spaces pass straight through.

/// A concrete civil date-time. All fields are explicit so date-variable
/// rendering never reads the wall clock — the document supplies these
/// via [`crate::pipeline::DocumentClock`], keeping renders deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateParts {
    /// Full year, e.g. `2026`.
    pub year: i32,
    /// Month, 1-12.
    pub month: u8,
    /// Day of month, 1-31.
    pub day: u8,
    /// Hour, 0-23.
    pub hour: u8,
    /// Minute, 0-59.
    pub minute: u8,
    /// Second, 0-59.
    pub second: u8,
}

impl DateParts {
    /// Day of the week. 0 = Sunday … 6 = Saturday, via Sakamoto's
    /// algorithm on the proleptic Gregorian calendar. Pure integer math
    /// — no `chrono`, no locale, no wall clock.
    fn weekday(self) -> usize {
        // Sakamoto's method.
        const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
        let mut y = self.year;
        let m = self.month as i32;
        if m < 3 {
            y -= 1;
        }
        let idx = (m - 1).clamp(0, 11) as usize;
        let w = (y + y / 4 - y / 100 + y / 400 + T[idx] + self.day as i32).rem_euclid(7);
        w as usize
    }
}

const MONTHS_FULL: [&str; 12] = [
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
];

const MONTHS_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const WEEKDAYS_FULL: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

const WEEKDAYS_ABBR: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Format `date` per the InDesign/ICU `pattern`. See the module docs for
/// the supported token subset. An empty pattern yields an empty string;
/// the caller decides what to do with that (it falls back to a sensible
/// default rather than rendering nothing).
pub fn format_date(pattern: &str, date: DateParts) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            // Quoted literal. `''` ⇒ a single apostrophe; otherwise copy
            // verbatim up to the closing quote (an unterminated quote
            // copies to end-of-string, matching ICU's lenient handling).
            if i + 1 < chars.len() && chars[i + 1] == '\'' {
                out.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                out.push(chars[i]);
                i += 1;
            }
            // Skip the closing quote when present.
            if i < chars.len() {
                i += 1;
            }
            continue;
        }
        if c.is_ascii_alphabetic() {
            // Greedily consume the run of this same letter (longest-token
            // match) so `MMMM` isn't split into `MM` + `MM`.
            let mut run = 1;
            while i + run < chars.len() && chars[i + run] == c {
                run += 1;
            }
            out.push_str(&render_token(c, run, date));
            i += run;
            continue;
        }
        // Punctuation, spaces, digits — verbatim.
        out.push(c);
        i += 1;
    }
    out
}

/// Render one field token: the designator letter `c` repeated `run`
/// times. Unknown designators echo the literal run (InDesign's own
/// fallback for tokens it can't compute).
fn render_token(c: char, run: usize, d: DateParts) -> String {
    let hour12 = {
        let h = d.hour % 12;
        if h == 0 {
            12
        } else {
            h
        }
    };
    match c {
        'y' => {
            if run <= 2 {
                // 2-digit year (last two digits, zero-padded).
                format!("{:02}", d.year.rem_euclid(100))
            } else {
                // Pad to at least `run` digits for `yyy`/`yyyy`/longer.
                format!("{:0width$}", d.year, width = run)
            }
        }
        'M' => match run {
            1 => d.month.to_string(),
            2 => format!("{:02}", d.month),
            3 => month_name(d.month, &MONTHS_ABBR),
            _ => month_name(d.month, &MONTHS_FULL),
        },
        'd' => {
            if run >= 2 {
                format!("{:02}", d.day)
            } else {
                d.day.to_string()
            }
        }
        'E' => {
            let w = d.weekday();
            if run >= 4 {
                WEEKDAYS_FULL[w].to_string()
            } else {
                WEEKDAYS_ABBR[w].to_string()
            }
        }
        'H' => {
            if run >= 2 {
                format!("{:02}", d.hour)
            } else {
                d.hour.to_string()
            }
        }
        'h' => {
            if run >= 2 {
                format!("{hour12:02}")
            } else {
                hour12.to_string()
            }
        }
        'm' => {
            if run >= 2 {
                format!("{:02}", d.minute)
            } else {
                d.minute.to_string()
            }
        }
        's' => {
            if run >= 2 {
                format!("{:02}", d.second)
            } else {
                d.second.to_string()
            }
        }
        'a' => {
            if d.hour < 12 {
                "AM".to_string()
            } else {
                "PM".to_string()
            }
        }
        // Unknown designator: echo it so nothing silently vanishes.
        _ => c.to_string().repeat(run),
    }
}

/// 1-based month → name from `table`, clamped so a malformed month
/// can't index out of bounds (it falls back to the nearest valid name).
fn month_name(month: u8, table: &[&str; 12]) -> String {
    let idx = (month.clamp(1, 12) - 1) as usize;
    table[idx].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-03-09 14:07:05 — a Monday in March (verifiable: 2026-03-09 is
    // indeed a Monday).
    const SAMPLE: DateParts = DateParts {
        year: 2026,
        month: 3,
        day: 9,
        hour: 14,
        minute: 7,
        second: 5,
    };

    #[test]
    fn weekday_is_correct() {
        // 2026-03-09 is a Monday (index 1).
        assert_eq!(SAMPLE.weekday(), 1);
        // 2026-01-01 is a Thursday (index 4).
        let nye = DateParts {
            year: 2026,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        };
        assert_eq!(nye.weekday(), 4);
        // A known Sunday: 2024-12-29.
        let sun = DateParts {
            year: 2024,
            month: 12,
            day: 29,
            ..SAMPLE
        };
        assert_eq!(sun.weekday(), 0);
    }

    #[test]
    fn year_tokens() {
        assert_eq!(format_date("yyyy", SAMPLE), "2026");
        assert_eq!(format_date("yy", SAMPLE), "26");
        // 2-digit year of a year < 100-padding edge: 2005 ⇒ "05".
        let y05 = DateParts {
            year: 2005,
            ..SAMPLE
        };
        assert_eq!(format_date("yy", y05), "05");
    }

    #[test]
    fn month_tokens() {
        assert_eq!(format_date("M", SAMPLE), "3");
        assert_eq!(format_date("MM", SAMPLE), "03");
        assert_eq!(format_date("MMM", SAMPLE), "Mar");
        assert_eq!(format_date("MMMM", SAMPLE), "March");
    }

    #[test]
    fn day_tokens() {
        assert_eq!(format_date("d", SAMPLE), "9");
        assert_eq!(format_date("dd", SAMPLE), "09");
    }

    #[test]
    fn weekday_tokens() {
        assert_eq!(format_date("EEE", SAMPLE), "Mon");
        assert_eq!(format_date("EEEE", SAMPLE), "Monday");
    }

    #[test]
    fn time_tokens_24h_and_12h() {
        assert_eq!(format_date("HH:mm:ss", SAMPLE), "14:07:05");
        assert_eq!(format_date("H", SAMPLE), "14");
        assert_eq!(format_date("h:mm a", SAMPLE), "2:07 PM");
        // Midnight + noon edge cases for the 12h clock + AM/PM marker.
        let midnight = DateParts {
            hour: 0,
            minute: 0,
            second: 0,
            ..SAMPLE
        };
        assert_eq!(format_date("hh:mm a", midnight), "12:00 AM");
        let noon = DateParts { hour: 12, ..SAMPLE };
        assert_eq!(format_date("hh a", noon), "12 PM");
    }

    #[test]
    fn common_indesign_patterns() {
        // The defaults the New Text Variable dialog offers.
        assert_eq!(format_date("MM/dd/yy", SAMPLE), "03/09/26");
        assert_eq!(format_date("MMMM d, yyyy", SAMPLE), "March 9, 2026");
        assert_eq!(
            format_date("EEEE, MMMM d, yyyy", SAMPLE),
            "Monday, March 9, 2026"
        );
        assert_eq!(format_date("dd.MM.yyyy", SAMPLE), "09.03.2026");
    }

    #[test]
    fn quoted_literals_pass_through() {
        // `'at'` is a literal; the tokens around it still resolve.
        assert_eq!(
            format_date("MMMM d 'at' h:mm a", SAMPLE),
            "March 9 at 2:07 PM"
        );
        // `''` is a literal apostrophe.
        assert_eq!(format_date("yyyy''", SAMPLE), "2026'");
        // A letter that would otherwise be a token, quoted, stays literal.
        assert_eq!(format_date("'M'M", SAMPLE), "M3");
    }

    #[test]
    fn unknown_designator_echoes() {
        // `G` (era) isn't modelled; echo it rather than dropping it.
        assert_eq!(format_date("G yyyy", SAMPLE), "G 2026");
    }

    #[test]
    fn empty_pattern_is_empty() {
        assert_eq!(format_date("", SAMPLE), "");
    }
}
