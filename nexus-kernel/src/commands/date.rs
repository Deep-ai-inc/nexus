//! The `date` command - display and format dates.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct DateCommand;

impl NexusCommand for DateCommand {
    fn name(&self) -> &'static str {
        "date"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut format: Option<&str> = None;
        let mut timestamp: Option<i64> = None;
        let mut utc = false;

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];

            if arg == "-u" || arg == "--utc" || arg == "--universal" {
                utc = true;
            } else if arg == "-d" || arg == "--date" {
                if i + 1 < args.len() {
                    // Parse relative or absolute date
                    // For now, just support @timestamp format
                    let date_str = &args[i + 1];
                    if date_str.starts_with('@') {
                        timestamp = date_str[1..].parse().ok();
                    }
                    i += 1;
                }
            } else if arg.starts_with('+') {
                // Format string
                format = Some(&arg[1..]);
            } else if arg == "-I" || arg == "--iso-8601" {
                format = Some("%Y-%m-%d");
            } else if arg == "-R" || arg == "--rfc-email" {
                format = Some("%a, %d %b %Y %H:%M:%S %z");
            } else if arg == "--rfc-3339" {
                if i + 1 < args.len() {
                    match args[i + 1].as_str() {
                        "date" => format = Some("%Y-%m-%d"),
                        "seconds" => format = Some("%Y-%m-%d %H:%M:%S%:z"),
                        "ns" => format = Some("%Y-%m-%d %H:%M:%S.%N%:z"),
                        _ => {}
                    }
                    i += 1;
                }
            }

            i += 1;
        }

        let now = if let Some(ts) = timestamp {
            UNIX_EPOCH + std::time::Duration::from_secs(ts as u64)
        } else {
            SystemTime::now()
        };

        let secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

        // Without chrono, we do basic formatting
        let formatted = if let Some(fmt) = format {
            format_date(secs, fmt, utc)
        } else {
            // Default format: "Sun Jan 24 12:34:56 UTC 2026"
            format_date_default(secs, utc)
        };

        Ok(Value::String(formatted))
    }
}

fn format_date(secs: u64, fmt: &str, _utc: bool) -> String {
    // Basic date formatting without chrono
    // This is a simplified implementation
    let (year, month, day, hour, min, sec, weekday) = timestamp_to_components(secs);

    let mut result = fmt.to_string();
    result = result.replace("%Y", &format!("{:04}", year));
    result = result.replace("%m", &format!("{:02}", month));
    result = result.replace("%d", &format!("{:02}", day));
    result = result.replace("%H", &format!("{:02}", hour));
    result = result.replace("%M", &format!("{:02}", min));
    result = result.replace("%S", &format!("{:02}", sec));
    result = result.replace("%s", &format!("{}", secs));

    let month_names = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let weekday_names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let weekday_full = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let month_full = [
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

    result = result.replace("%b", month_names[month as usize - 1]);
    result = result.replace("%B", month_full[month as usize - 1]);
    result = result.replace("%a", weekday_names[weekday as usize]);
    result = result.replace("%A", weekday_full[weekday as usize]);
    result = result.replace("%y", &format!("{:02}", year % 100));
    result = result.replace("%e", &format!("{:2}", day));
    result = result.replace("%j", &format!("{:03}", day_of_year(year, month, day)));
    result = result.replace("%n", "\n");
    result = result.replace("%t", "\t");
    result = result.replace("%%", "%");

    // Time of day
    let (hour12, ampm) = if hour == 0 {
        (12, "AM")
    } else if hour < 12 {
        (hour, "AM")
    } else if hour == 12 {
        (12, "PM")
    } else {
        (hour - 12, "PM")
    };
    result = result.replace("%I", &format!("{:02}", hour12));
    result = result.replace("%p", ampm);
    result = result.replace("%P", &ampm.to_lowercase());

    // Week number (simplified - not fully accurate)
    result = result.replace("%W", &format!("{:02}", (day_of_year(year, month, day) + 6) / 7));
    result = result.replace("%U", &format!("{:02}", (day_of_year(year, month, day) + 6) / 7));

    result
}

fn format_date_default(secs: u64, utc: bool) -> String {
    let (year, month, day, hour, min, sec, weekday) = timestamp_to_components(secs);

    let month_names = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let weekday_names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

    let tz = if utc { "UTC" } else { "Local" };

    format!(
        "{} {} {:2} {:02}:{:02}:{:02} {} {}",
        weekday_names[weekday as usize],
        month_names[month as usize - 1],
        day,
        hour,
        min,
        sec,
        tz,
        year
    )
}

fn timestamp_to_components(secs: u64) -> (u32, u32, u32, u32, u32, u32, u32) {
    // Convert Unix timestamp to date components
    // This is a simplified implementation (assumes UTC)
    let days = (secs / 86400) as i64;
    let time = secs % 86400;

    let hour = (time / 3600) as u32;
    let min = ((time % 3600) / 60) as u32;
    let sec = (time % 60) as u32;

    // Days since 1970-01-01
    // Calculate year, month, day
    let (year, month, day) = days_to_ymd(days);
    let weekday = ((days + 4) % 7) as u32; // 1970-01-01 was Thursday (4)

    (year, month, day, hour, min, sec, weekday)
}

fn days_to_ymd(days: i64) -> (u32, u32, u32) {
    // Algorithm to convert days since epoch to year/month/day
    let mut y = 1970;
    let mut remaining = days;

    loop {
        let days_in_year = if is_leap_year(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }

    let days_in_month = if is_leap_year(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut m = 1u32;
    for &days in &days_in_month {
        if remaining < days {
            break;
        }
        remaining -= days;
        m += 1;
    }

    (y, m, (remaining + 1) as u32)
}

fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn day_of_year(year: u32, month: u32, day: u32) -> u32 {
    let days_in_month = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut doy = day;
    for i in 0..(month - 1) as usize {
        doy += days_in_month[i];
    }
    doy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_days_to_ymd() {
        // 1970-01-01
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        // 1970-01-02
        assert_eq!(days_to_ymd(1), (1970, 1, 2));
        // 1971-01-01
        assert_eq!(days_to_ymd(365), (1971, 1, 1));
    }

    #[test]
    fn test_is_leap_year() {
        assert!(!is_leap_year(1970));
        assert!(is_leap_year(1972));
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));
    }

    #[test]
    fn test_format_date() {
        let secs = 0u64; // 1970-01-01 00:00:00
        let formatted = format_date(secs, "%Y-%m-%d", true);
        assert_eq!(formatted, "1970-01-01");
    }
}
