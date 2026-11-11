pub(crate) fn iso(time: i64) -> String {
    // Gregorian civil date from days since the Unix epoch.
    let seconds = time.div_euclid(1_000_000);
    let days = seconds.div_euclid(86400) + 719468;
    let era = days.div_euclid(146097);
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let daytime = seconds.rem_euclid(86400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}Z",
        daytime / 3600,
        daytime / 60 % 60,
        daytime % 60,
        time.rem_euclid(1_000_000)
    )
}

/// Studio emits RFC3339 with microseconds. Compare instants, including offsets, not strings.
pub(crate) fn timestamp(time: &str) -> Option<i64> {
    if time.get(4..5) != Some("-")
        || time.get(7..8) != Some("-")
        || time.get(10..11) != Some("T")
        || time.get(13..14) != Some(":")
        || time.get(16..17) != Some(":")
    {
        return None;
    }
    let number = |start, end| time.get(start..end)?.parse::<i64>().ok();
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    let leap = |y| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=months[(month - 1) as usize]).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let y = year - 1;
    let days = 365 * (year - 1970) + y / 4 - 1969 / 4 - y / 100 + 1969 / 100 + y / 400 - 1969 / 400
        + months[..(month - 1) as usize].iter().sum::<i64>()
        + day
        - 1;
    let suffix = time.get(19..)?;
    let zone = suffix.find(['Z', '+', '-'])?;
    let fraction = match &suffix[..zone] {
        "" => "",
        part => part
            .strip_prefix('.')
            .filter(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))?,
    };
    let fractional = format!("{fraction:0<6}").get(..6)?.parse::<i64>().ok()?;
    let offset = match &suffix[zone..] {
        "Z" => 0,
        zone => {
            if zone.len() != 6 || zone.get(3..4) != Some(":") {
                return None;
            }
            let hours = zone.get(1..3)?.parse::<i64>().ok()?;
            let minutes = zone.get(4..6)?.parse::<i64>().ok()?;
            if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
                return None;
            }
            (hours * 60 + minutes) * if zone.starts_with('-') { -1 } else { 1 }
        }
    };
    Some(((days * 24 + hour) * 3600 + minute * 60 + second - offset * 60) * 1_000_000 + fractional)
}
