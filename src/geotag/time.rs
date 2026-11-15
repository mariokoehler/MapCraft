//! Times for matching photos to a track (design.md §2.9): seconds since 1970 as `f64`, in UTC for
//! track points, and "as if UTC" for a photo's local capture time (the shift turns one into the
//! other). Our own small calendar arithmetic: the app needs only these few conversions.

/// Days since 1970-01-01 of a date in the proleptic Gregorian calendar (H. Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day since 1970: (year, month, day).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fff]` and an optional zone (`Z`, `+02:00`, `-0530`).
/// Returns the seconds of that clock reading (as if UTC) and the zone's offset in seconds.
pub fn parse(s: &str) -> Option<(f64, Option<i64>)> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    if b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') || b[13] != b':' {
        return None;
    }
    if b[16] != b':' {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut rest = &s[19..];
    let mut frac = 0.0;
    if let Some(r) = rest.strip_prefix('.') {
        let n = r.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        frac = format!("0.{}", &r[..n]).parse().ok()?;
        rest = &r[n..];
    }
    let zone = parse_offset(rest)?;
    let days = days_from_civil(y, mo as u32, d as u32);
    let t = (days * 86_400 + h * 3600 + mi * 60 + sec) as f64 + frac;
    Some((t, zone))
}

/// `""` → no zone; `Z` → 0; `+02:00`, `+0200`, `-05:30` → seconds east of UTC.
/// `Some(None)` is "no zone"; `None` is unreadable.
pub fn parse_offset(s: &str) -> Option<Option<i64>> {
    let s = s.trim();
    if s.is_empty() {
        return Some(None);
    }
    if s == "Z" || s == "z" {
        return Some(Some(0));
    }
    let (sign, r) = match s.as_bytes()[0] {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    let digits: String = r.chars().filter(|c| *c != ':').collect();
    if digits.len() != 4 || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (h, m): (i64, i64) = (digits[..2].parse().ok()?, digits[2..].parse().ok()?);
    if h > 14 || m > 59 {
        return None;
    }
    Some(Some(sign * (h * 3600 + m * 60)))
}

/// A UTC instant as `YYYY-MM-DDTHH:MM:SSZ` (whole seconds), as XMP dates are written.
pub fn format_utc(t: f64) -> String {
    let s = t.round() as i64;
    let (days, rem) = (s.div_euclid(86_400), s.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// A clock reading as `HH:MM:SS`.
pub fn clock(t: f64) -> String {
    let s = (t.round() as i64).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// A clock reading as `D Mon YYYY`.
pub fn day(t: f64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (y, m, d) = civil_from_days((t.floor() as i64).div_euclid(86_400));
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

/// A shift (seconds east of UTC) as `UTC+2:03:40` (seconds only when there are any).
pub fn shift_text(s: i64) -> String {
    let sign = if s < 0 { '−' } else { '+' };
    let a = s.abs();
    let (h, m, sec) = (a / 3600, a / 60 % 60, a % 60);
    if sec == 0 {
        format!("UTC{sign}{h}:{m:02}")
    } else {
        format!("UTC{sign}{h}:{m:02}:{sec:02}")
    }
}

/// A time zone (whole minutes) as `UTC+02:00`.
pub fn zone_text(s: i64) -> String {
    let sign = if s < 0 { '−' } else { '+' };
    let a = s.abs();
    format!("UTC{sign}{:02}:{:02}", a / 3600, a / 60 % 60)
}

/// This PC's UTC offset (seconds east) at a UTC instant, with that date's daylight saving.
#[cfg(windows)]
pub fn local_offset_at(utc: f64) -> Option<i64> {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;
    let st = |t: i64| {
        let (y, m, d) = civil_from_days(t.div_euclid(86_400));
        let r = t.rem_euclid(86_400);
        SYSTEMTIME {
            wYear: y as u16,
            wMonth: m as u16,
            wDayOfWeek: 0,
            wDay: d as u16,
            wHour: (r / 3600) as u16,
            wMinute: (r / 60 % 60) as u16,
            wSecond: (r % 60) as u16,
            wMilliseconds: 0,
        }
    };
    let t = utc.floor() as i64;
    let u = st(t);
    let mut l = st(0);
    // SAFETY: both pointers are to live SYSTEMTIMEs; a null zone means the PC's current zone.
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &u, &mut l) };
    if ok == 0 {
        return None;
    }
    let local = days_from_civil(l.wYear as i64, l.wMonth as u32, l.wDay as u32) * 86_400
        + l.wHour as i64 * 3600
        + l.wMinute as i64 * 60
        + l.wSecond as i64;
    Some(local - t)
}

#[cfg(not(windows))]
pub fn local_offset_at(_utc: f64) -> Option<i64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        for z in [-800_000, -1, 0, 59, 60, 11_016, 20_000, 2_000_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }

    #[test]
    fn timestamps_with_and_without_zones() {
        let (t, z) = parse("2026-10-12T08:12:23.582Z").unwrap();
        assert_eq!(z, Some(0));
        assert_eq!(format_utc(t), "2026-10-12T08:12:24Z");
        assert!((t.fract() - 0.582).abs() < 1e-6);
        let (local, z) = parse("2026-10-12T10:12:23+02:00").unwrap();
        assert_eq!(z, Some(7200));
        assert_eq!(local - 7200.0, t.floor());
        assert_eq!(parse("2012-08-18T18:45:38").unwrap().1, None);
        assert_eq!(parse("2012-08-18 18:45:38-0530").unwrap().1, Some(-19_800));
        for bad in [
            "",
            "2012-08-18",
            "2012-13-01T00:00:00",
            "2012-08-18T18:45:38+2",
            "x",
        ] {
            assert!(parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn texts() {
        assert_eq!(shift_text(7420), "UTC+2:03:40");
        assert_eq!(shift_text(-18_000), "UTC−5:00");
        assert_eq!(zone_text(19_800), "UTC+05:30");
        assert_eq!(clock(36_000.4), "10:00:00");
        assert_eq!(day(parse("2025-06-12T10:00:00").unwrap().0), "12 Jun 2025");
    }

    #[cfg(windows)]
    #[test]
    fn this_pcs_offset_is_whole_minutes() {
        let o = local_offset_at(parse("2025-06-12T10:00:00Z").unwrap().0).unwrap();
        assert_eq!(o % 60, 0);
        assert!(o.abs() <= 14 * 3600);
    }
}
