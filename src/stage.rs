//! MAA 活动关卡目录：缓存、热更新与活动状态解析。

mod model;
mod refresh;
mod time;

#[allow(unused_imports)]
pub use model::{StageCatalog, StageEntry, StageRefreshEvent};

#[cfg(test)]
mod tests {
    use super::model::Availability;
    use super::time::{availability, parse_timestamp, parse_timezone_offset};
    use super::*;

    #[test]
    fn parses_stage_activity_entries() {
        let json = serde_json::json!({
            "Official": {
                "sideStoryStage": {
                    "TEST": {
                        "MinimumRequired": "v6.0.0",
                        "Activity": {
                            "Tip": "测试活动",
                            "StageName": "TEST",
                            "UtcStartTime": "2099/01/01 00:00:00",
                            "UtcExpireTime": "2099/01/02 00:00:00",
                            "TimeZone": 8
                        },
                        "Stages": [{
                            "Display": "TEST-8",
                            "Value": "TEST-8",
                            "Drop": "测试材料"
                        }]
                    }
                }
            }
        });
        let catalog = StageCatalog::from_json(&json);
        let entries = catalog.entries_for("Official");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].value, "TEST-8");
        assert!(entries[0].label.contains("TEST-8"));
        assert!(entries[0].description.contains("测试材料"));
    }

    #[test]
    fn parses_activity_timestamp_with_timezone_offset() {
        assert_eq!(parse_timestamp("1970/01/01 00:00:00", 0), Some(0));
        assert_eq!(parse_timestamp("1970/01/01 08:00:00", 8 * 3_600), Some(0));
        assert_eq!(parse_timestamp("1969/12/31 19:00:00", -5 * 3_600), Some(0));
        assert!(parse_timestamp("not-a-date", 0).is_none());
    }

    #[test]
    fn parses_numeric_and_string_timezones_safely() {
        assert_eq!(parse_timezone_offset(&serde_json::json!(8)), Some(28_800));
        assert_eq!(
            parse_timezone_offset(&serde_json::json!("-5.5")),
            Some(-19_800)
        );
        assert_eq!(parse_timezone_offset(&serde_json::json!(25)), None);
        assert_eq!(parse_timezone_offset(&serde_json::json!("invalid")), None);
    }

    #[test]
    fn applies_timezone_before_calculating_activity_status() {
        let json = serde_json::json!({
            "Official": {
                "sideStoryStage": {
                    "TEST": {
                        "Activity": {
                            "UtcStartTime": "1970/01/01 08:00:00",
                            "UtcExpireTime": "1970/01/01 09:00:00",
                            "TimeZone": 8
                        },
                        "Stages": [{"Display": "TEST-1", "Value": "TEST-1"}]
                    }
                }
            }
        });

        let catalog = StageCatalog::from_json_at(&json, 30 * 60);
        let entries = catalog.entries_for("Official");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].available);
        assert!(entries[0].label.contains("开放中"));
    }

    #[test]
    fn handles_activity_availability_boundaries() {
        assert_eq!(
            availability(Some(100), Some(200), 99),
            Availability::Upcoming
        );
        assert_eq!(availability(Some(100), Some(200), 100), Availability::Open);
        assert_eq!(availability(Some(100), Some(200), 200), Availability::Open);
        assert_eq!(
            availability(Some(100), Some(200), 201),
            Availability::Expired
        );
        assert_eq!(availability(None, Some(200), 100), Availability::Unknown);
    }

    #[test]
    fn keeps_client_entries_separate() {
        let json = serde_json::json!({
            "Official": {"sideStoryStage": {}},
            "Bilibili": {"sideStoryStage": {}}
        });
        let catalog = StageCatalog::from_json(&json);
        assert!(catalog.entries_for("Official").is_empty());
        assert!(catalog.entries_for("Bilibili").is_empty());
    }
}
