use serde_json::Value;

wit_bindgen::generate!({
    inline: r#"
        package helpcore:plugin;

        interface host {
            http-request: func(request-json: string) -> result<string, string>;
            data-read: func(path: string) -> result<string, string>;
            data-write: func(path: string, content: string) -> result<_, string>;
            config-read: func(key: string) -> result<string, string>;
        }

        world plugin {
            import host;
            export call: func(tool: string, input-json: string) -> result<string, string>;
        }
    "#,
    world: "plugin",
});

struct DateUtils;

impl Guest for DateUtils {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "day_of_week" => day_of_week(&input),
            "week_number" => week_number(&input),
            "date_diff" => date_diff(&input),
            "date_add" => date_add(&input),
            "unix_to_date" => unix_to_date(&input),
            "date_to_unix" => date_to_unix(&input),
            "is_leap_year" => is_leap_year(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(DateUtils);

#[derive(Debug, Clone)]
struct Date {
    year: i32,
    month: u32,
    day: u32,
}

fn parse_date(s: &str) -> Result<Date, String> {
    let parts: Vec<&str> = s.trim().split('-').collect();
    if parts.len() != 3 {
        return Err(format!("invalid date format: {s}, expected YYYY-MM-DD"));
    }
    let year: i32 = parts[0].parse().map_err(|_| format!("invalid year: {}", parts[0]))?;
    let month: u32 = parts[1].parse().map_err(|_| format!("invalid month: {}", parts[1]))?;
    let day: u32 = parts[2].parse().map_err(|_| format!("invalid day: {}", parts[2]))?;

    if month < 1 || month > 12 {
        return Err(format!("invalid month: {month}"));
    }
    let max_day = days_in_month(year, month);
    if day < 1 || day > max_day {
        return Err(format!("invalid day {day} for {year}-{month:02} (max {max_day})"));
    }

    Ok(Date { year, month, day })
}

fn is_leap_year_check(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap_year_check(year) { 29 } else { 28 },
        _ => 30,
    }
}

fn date_to_jdn(date: &Date) -> i64 {
    let y = date.year as i64;
    let m = date.month as i64;
    let d = date.day as i64;
    // Convert to Julian Day Number using standard formula
    let a = (14 - m) / 12;
    let y = y + 4800 - a;
    let m = m + 12 * a - 3;
    d + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045
}

fn jdn_to_date(jdn: i64) -> Date {
    let a = jdn + 32044;
    let b = (4 * a + 3) / 146097;
    let c = a - (146097 * b) / 4;
    let d = (4 * c + 3) / 1461;
    let e = c - (1461 * d) / 4;
    let m = (5 * e + 2) / 153;
    let day = (e - (153 * m + 2) / 5 + 1) as u32;
    let month = (m + 3 - 12 * (m / 10)) as u32;
    let year = (100 * b + d - 4800 + m / 10) as i32;
    Date { year, month, day }
}

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn get_f64(input: &Value, key: &str) -> Result<f64, String> {
    input
        .get(key)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| format!("{key} must be a number"))
}

const DAY_NAMES: &[&str] = &[
    "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday",
];

fn day_of_week(input: &Value) -> Result<String, String> {
    let date = parse_date(get_str(input, "date")?)?;
    let jdn = date_to_jdn(&date);
    // Julian Day 0 = Monday, JDN mod 7: 0=Mon, ..., 6=Sun
    let idx = jdn.rem_euclid(7) as usize;
    Ok(format!("{} is a {}", format_date(&date), DAY_NAMES[idx]))
}

fn week_number(input: &Value) -> Result<String, String> {
    let date = parse_date(get_str(input, "date")?)?;
    let week = iso_week(&date);
    Ok(format!("{} is in ISO week {}", format_date(&date), week))
}

fn iso_week(date: &Date) -> u32 {
    // ISO 8601: week 1 is the week containing the first Thursday of the year
    let jdn = date_to_jdn(date);
    let day_of_week = jdn.rem_euclid(7) as i64; // 0=Mon

    // Thursday of the current week
    let thursday_jdn = jdn + 3 - day_of_week;

    // Jan 4 of current year (always in week 1)
    let jan4 = date_to_jdn(&Date { year: date.year, month: 1, day: 4 });
    let jan4_dow = jan4.rem_euclid(7) as i64;
    let week1_thursday = jan4 + 3 - jan4_dow;

    let week = ((thursday_jdn - week1_thursday) / 7) as u32 + 1;

    if week >= 52 {
        // Check if this Thursday is actually in week 1 of next year
        let next_jan4 = date_to_jdn(&Date { year: date.year + 1, month: 1, day: 4 });
        let next_jan4_dow = next_jan4.rem_euclid(7) as i64;
        let next_week1_thursday = next_jan4 + 3 - next_jan4_dow;
        if thursday_jdn >= next_week1_thursday {
            return 1;
        }
        // Or if it's before week 1 of this year (week belongs to previous year)
        if thursday_jdn < week1_thursday {
            let prev_jan4 = date_to_jdn(&Date { year: date.year - 1, month: 1, day: 4 });
            let prev_jan4_dow = prev_jan4.rem_euclid(7) as i64;
            let prev_week1_thursday = prev_jan4 + 3 - prev_jan4_dow;
            return ((thursday_jdn - prev_week1_thursday) / 7) as u32 + 1;
        }
    }

    week
}

fn date_diff(input: &Value) -> Result<String, String> {
    let d1 = parse_date(get_str(input, "date1")?)?;
    let d2 = parse_date(get_str(input, "date2")?)?;
    let unit = get_str_opt(input, "unit").unwrap_or("days");

    let jdn1 = date_to_jdn(&d1);
    let jdn2 = date_to_jdn(&d2);
    let diff = jdn2 - jdn1;

    let result = match unit {
        "days" => format!("{diff} days"),
        "weeks" => {
            let weeks = diff as f64 / 7.0;
            format!("{weeks:.2} weeks ({diff} days)")
        }
        "months" => {
            let months = (d2.year - d1.year) as i64 * 12 + (d2.month as i64 - d1.month as i64);
            let approx = format!("~{months} months ({diff} days)");
            approx
        }
        "years" => {
            let years = diff as f64 / 365.25;
            format!("{years:.2} years ({diff} days)")
        }
        _ => format!("{diff} days"),
    };

    Ok(format!(
        "Between {} and {}: {}",
        format_date(&d1), format_date(&d2), result
    ))
}

fn date_add(input: &Value) -> Result<String, String> {
    let date = parse_date(get_str(input, "date")?)?;
    let amount = get_f64(input, "amount")? as i64;
    let unit = get_str(input, "unit")?;

    let result = match unit {
        "days" => jdn_to_date(date_to_jdn(&date) + amount),
        "weeks" => jdn_to_date(date_to_jdn(&date) + amount * 7),
        "months" => {
            let total_months = date.year as i64 * 12 + date.month as i64 - 1 + amount;
            let new_year = (total_months / 12) as i32;
            let new_month = ((total_months % 12) + 1) as u32;
            let max_day = days_in_month(new_year, new_month);
            let new_day = date.day.min(max_day);
            Date { year: new_year, month: new_month, day: new_day }
        }
        "years" => {
            let max_day = days_in_month(date.year + amount as i32, date.month);
            let new_day = date.day.min(max_day);
            Date { year: date.year + amount as i32, month: date.month, day: new_day }
        }
        _ => return Err(format!("unknown unit: {unit}")),
    };

    Ok(format!("{} {} {} = {}", format_date(&date), if amount >= 0 { "+" } else { "-" }, amount.abs(), format_date(&result)))
}

fn unix_to_date(input: &Value) -> Result<String, String> {
    let ts = get_f64(input, "timestamp")? as i64;
    let unix_epoch_jdn = date_to_jdn(&Date { year: 1970, month: 1, day: 1 });
    let days = ts / 86400;
    let date = jdn_to_date(unix_epoch_jdn + days);
    let remaining = ts % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;

    Ok(format!(
        "Unix timestamp {ts} = {} {:02}:{:02}:{:02} UTC",
        format_date(&date), hours, if minutes < 0 { -minutes } else { minutes }, if seconds < 0 { -seconds } else { seconds }
    ))
}

fn date_to_unix(input: &Value) -> Result<String, String> {
    let date = parse_date(get_str(input, "date")?)?;
    let unix_epoch_jdn = date_to_jdn(&Date { year: 1970, month: 1, day: 1 });
    let days = date_to_jdn(&date) - unix_epoch_jdn;
    let ts = days * 86400;
    Ok(format!("{} = Unix timestamp {ts} (midnight UTC)", format_date(&date)))
}

fn is_leap_year(input: &Value) -> Result<String, String> {
    let year = get_f64(input, "year")? as i32;
    let leap = is_leap_year_check(year);
    Ok(format!("{year} is {}a leap year", if leap { "" } else { "not " }))
}

fn get_str_opt<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn format_date(date: &Date) -> String {
    format!("{:04}-{:02}-{:02}", date.year, date.month, date.day)
}
