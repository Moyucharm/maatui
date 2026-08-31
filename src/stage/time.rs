//! Stage 活动时间与时区解析。

use super::model::Availability;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn availability(start: Option<i64>, expire: Option<i64>, now: i64) -> Availability {
    match (start, expire) {
        (Some(start), Some(_expire)) if now < start => Availability::Upcoming,
        (Some(_), Some(expire)) if now <= expire => Availability::Open,
        (Some(_), Some(_)) => Availability::Expired,
        _ => Availability::Unknown,
    }
}

pub(super) fn parse_timezone_offset(value: &Value) -> Option<i64> {
    let hours = value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())?;
    if !hours.is_finite() || !(-24.0..=24.0).contains(&hours) {
        return None;
    }
    Some((hours * 3_600.0).round() as i64)
}

pub(super) fn parse_timestamp(value: &str, timezone_offset: i64) -> Option<i64> {
    let mut parts = value.split(['/', ' ', ':']);
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    let hour = parts.next()?.parse::<i64>().ok()?;
    let minute = parts.next()?.parse::<i64>().ok()?;
    let second = parts.next()?.parse::<i64>().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    Some(
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
            - timezone_offset,
    )
}

// Howard Hinnant 的 civil date 算法：不引入重量级日期依赖即可比较 UTC 时间。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub(super) fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}
