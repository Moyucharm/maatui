//! Daily 任务 schema、字段校验和编辑选项。

use std::fs;
use std::path::PathBuf;

use super::{FieldEditor, FieldScope, FieldSpec, FieldValue, HotUpdateVersion, SelectOption};
use crate::storage::maa_hot_update_dir;

pub(crate) fn task_fields(task_type: &str, advanced: bool) -> Vec<FieldSpec> {
    let mut fields = if advanced {
        Vec::new()
    } else {
        vec![
            field(
                "name",
                "名称",
                FieldScope::Task,
                FieldValue::String(String::new()),
            ),
            field("enable", "启用", FieldScope::Param, FieldValue::Bool(true)),
        ]
    };
    let definitions: &[(&str, &str, FieldValue, bool)] = match task_type {
        "StartUp" => &[
            ("client_type", "客户端", string("Official"), false),
            (
                "start_game_enabled",
                "自动启动游戏",
                FieldValue::Bool(true),
                false,
            ),
            ("account_name", "账号名称", string(""), true),
        ],
        "Recruit" => &[
            ("times", "招募次数", FieldValue::Integer(4), false),
            ("select", "选择星级", integers(&[4]), false),
            ("confirm", "确认星级", integers(&[3, 4]), false),
            ("refresh", "刷新三星标签", FieldValue::Bool(false), true),
            ("expedite", "使用加急许可", FieldValue::Bool(false), true),
            ("skip_robot", "跳过支援机械", FieldValue::Bool(true), true),
            ("first_tags", "首选标签", strings(&[]), true),
            (
                "extra_tags_mode",
                "额外标签模式",
                FieldValue::Integer(0),
                true,
            ),
            ("server", "统计服务器", string("CN"), true),
        ],
        "Fight" => &[
            ("stage", "关卡", string(""), false),
            ("times", "战斗次数", FieldValue::Integer(0), false),
            ("medicine", "理智药", FieldValue::Integer(0), false),
            (
                "medicine_expire_days",
                "临期理智药",
                FieldValue::Integer(0),
                false,
            ),
            ("series", "连战次数", FieldValue::Integer(1), false),
            ("stone", "碎石数量", FieldValue::Integer(0), true),
            (
                "report_to_penguin",
                "汇报企鹅物流",
                FieldValue::Bool(false),
                true,
            ),
            ("penguin_id", "企鹅物流 ID", string(""), true),
            (
                "report_to_yituliu",
                "汇报一图流",
                FieldValue::Bool(false),
                true,
            ),
            ("yituliu_id", "一图流 ID", string(""), true),
            ("client_type", "崩溃重连客户端", string(""), true),
            ("DrGrandet", "节省理智碎石", FieldValue::Bool(false), true),
        ],
        "Infrast" => &[
            (
                "facility",
                "设施顺序",
                strings(&[
                    "Mfg",
                    "Trade",
                    "Control",
                    "Power",
                    "Reception",
                    "Office",
                    "Dorm",
                ]),
                false,
            ),
            ("drones", "无人机用途", string("Money"), false),
            ("mode", "换班模式", FieldValue::Integer(0), false),
            ("threshold", "心情阈值", FieldValue::Float(0.3), true),
            ("replenish", "源石碎片补货", FieldValue::Bool(false), true),
            (
                "dorm_trust_enabled",
                "宿舍补信赖",
                FieldValue::Bool(false),
                true,
            ),
            ("filename", "自定义排班文件", string(""), true),
            ("plan_index", "排班方案序号", FieldValue::Integer(0), true),
        ],
        "Mall" => &[
            ("visit_friends", "访问好友", FieldValue::Bool(true), false),
            ("shopping", "信用购物", FieldValue::Bool(true), false),
            ("buy_first", "优先购买", strings(&[]), false),
            ("blacklist", "购物黑名单", strings(&[]), false),
            ("credit_fight", "信用助战", FieldValue::Bool(false), true),
            (
                "only_buy_discount",
                "仅买折扣",
                FieldValue::Bool(false),
                true,
            ),
            (
                "reserve_max_credit",
                "保留信用",
                FieldValue::Bool(false),
                true,
            ),
            ("formation_index", "助战编队", FieldValue::Integer(0), true),
        ],
        "Award" => &[
            ("award", "任务奖励", FieldValue::Bool(true), false),
            ("mail", "邮件奖励", FieldValue::Bool(false), false),
            ("recruit", "限定池单抽", FieldValue::Bool(false), true),
            ("orundum", "幸运墙", FieldValue::Bool(false), true),
            ("mining", "限时开采", FieldValue::Bool(false), true),
            ("specialaccess", "特殊月卡", FieldValue::Bool(false), true),
        ],
        "CloseDown" => &[("client_type", "客户端", string("Official"), false)],
        "Copilot" => &[
            ("filename", "作业文件", string(""), false),
            ("loop_times", "循环次数", FieldValue::Integer(1), false),
            ("formation", "自动编队", FieldValue::Bool(false), false),
            ("formation_index", "编队编号", FieldValue::Integer(0), true),
            (
                "use_sanity_potion",
                "使用理智药",
                FieldValue::Bool(false),
                true,
            ),
            ("add_trust", "补低信赖干员", FieldValue::Bool(false), true),
            (
                "ignore_requirements",
                "忽略练度要求",
                FieldValue::Bool(false),
                true,
            ),
            (
                "support_unit_usage",
                "助战模式",
                FieldValue::Integer(0),
                true,
            ),
            ("support_unit_name", "指定助战干员", string(""), true),
        ],
        _ => &[],
    };
    fields.extend(
        definitions
            .iter()
            .filter(|(_, _, _, is_advanced)| *is_advanced == advanced)
            .map(|(key, label, default, _)| field(key, label, FieldScope::Param, default.clone())),
    );
    fields
}

pub(crate) fn variant_fields() -> Vec<FieldSpec> {
    vec![
        field(
            "type",
            "条件类型",
            FieldScope::VariantCondition,
            string("Always"),
        ),
        field(
            "client",
            "活动客户端",
            FieldScope::VariantCondition,
            string("Official"),
        ),
        field(
            "start",
            "开始时间",
            FieldScope::VariantCondition,
            string(""),
        ),
        field("end", "结束时间", FieldScope::VariantCondition, string("")),
        field(
            "weekdays",
            "星期",
            FieldScope::VariantCondition,
            strings(&[]),
        ),
        field(
            "divisor",
            "周期除数",
            FieldScope::VariantCondition,
            FieldValue::Integer(1),
        ),
        field(
            "remainder",
            "周期余数",
            FieldScope::VariantCondition,
            FieldValue::Integer(0),
        ),
        field(
            "timezone",
            "时区/客户端",
            FieldScope::VariantCondition,
            string("Official"),
        ),
        field("stage", "关卡", FieldScope::VariantParam, string("")),
        field(
            "medicine",
            "理智药",
            FieldScope::VariantParam,
            FieldValue::Integer(0),
        ),
        field(
            "medicine_expire_days",
            "临期理智药",
            FieldScope::VariantParam,
            FieldValue::Integer(0),
        ),
        field(
            "expiring_medicine",
            "旧版过期药数量",
            FieldScope::VariantParam,
            FieldValue::Integer(0),
        ),
        field(
            "stone",
            "碎石数量",
            FieldScope::VariantParam,
            FieldValue::Integer(0),
        ),
        field(
            "times",
            "战斗次数",
            FieldScope::VariantParam,
            FieldValue::Integer(0),
        ),
        field(
            "series",
            "连战次数",
            FieldScope::VariantParam,
            FieldValue::Integer(1),
        ),
    ]
}

pub(super) fn validate_field_value(key: &str, value: &FieldValue) -> anyhow::Result<()> {
    match (key, value) {
        ("series", FieldValue::Integer(value)) if !(-1..=6).contains(value) => {
            anyhow::bail!("连战次数必须为 -1 到 6")
        }
        ("threshold", FieldValue::Float(value)) if !(0.0..=1.0).contains(value) => {
            anyhow::bail!("心情阈值必须为 0 到 1")
        }
        ("formation_index", FieldValue::Integer(value)) if !(0..=4).contains(value) => {
            anyhow::bail!("编队编号必须为 0 到 4")
        }
        ("support_unit_usage", FieldValue::Integer(value)) if !(0..=3).contains(value) => {
            anyhow::bail!("助战模式必须为 0 到 3")
        }
        (
            "times"
            | "medicine"
            | "medicine_expire_days"
            | "expiring_medicine"
            | "stone"
            | "loop_times"
            | "divisor"
            | "remainder",
            FieldValue::Integer(value),
        ) if *value < 0 => anyhow::bail!("该数值不能小于 0"),
        ("type", FieldValue::String(value)) if is_compound_condition_type(value) => {
            anyhow::bail!("组合条件暂不支持可视化创建，请保留并手动维护原配置")
        }
        _ => Ok(()),
    }
}

fn field(
    key: &'static str,
    label: &'static str,
    scope: FieldScope,
    default: FieldValue,
) -> FieldSpec {
    FieldSpec {
        key,
        label,
        scope,
        editor: editor_for(key),
        default,
    }
}

fn editor_for(key: &str) -> FieldEditor {
    match key {
        "client_type" | "client" | "timezone" => FieldEditor::Select {
            options: client_options(),
            allow_custom: true,
        },
        "series" => FieldEditor::Select {
            options: series_options(),
            allow_custom: false,
        },
        "mode" => FieldEditor::Select {
            options: infrast_mode_options(),
            allow_custom: false,
        },
        "drones" => FieldEditor::Select {
            options: drone_options(),
            allow_custom: true,
        },
        "medicine_expire_days" => FieldEditor::Select {
            options: medicine_expire_options(),
            allow_custom: false,
        },
        "formation_index" => FieldEditor::Select {
            options: copilot_formation_options(),
            allow_custom: false,
        },
        "support_unit_usage" => FieldEditor::Select {
            options: support_usage_options(),
            allow_custom: false,
        },
        "extra_tags_mode" => FieldEditor::Select {
            options: extra_tags_options(),
            allow_custom: false,
        },
        "server" => FieldEditor::Select {
            options: server_options(),
            allow_custom: false,
        },
        "select" | "confirm" => FieldEditor::MultiSelect {
            options: recruit_star_options(),
            allow_custom: false,
        },
        "facility" => FieldEditor::MultiSelect {
            options: facility_options(),
            allow_custom: true,
        },
        "stage" => FieldEditor::Stage { allow_custom: true },
        "type" => FieldEditor::Select {
            options: condition_type_options(),
            allow_custom: false,
        },
        _ => FieldEditor::Text,
    }
}

fn option(value: FieldValue, label: &str, description: &str) -> SelectOption {
    SelectOption {
        value,
        label: label.to_string(),
        description: description.to_string(),
    }
}

fn client_options() -> Vec<SelectOption> {
    [
        ("", "不指定", "沿用 maa-cli / MaaCore 的默认客户端"),
        ("Official", "官服", "Arknights 官方服务器"),
        ("Bilibili", "Bilibili", "Bilibili 服务器"),
        ("txwy", "森空岛 / txwy", "txwy 客户端"),
        ("YoStarEN", "国际服 EN", "YoStar 英文客户端"),
        ("YoStarJP", "国际服 JP", "YoStar 日文客户端"),
        ("YoStarKR", "国际服 KR", "YoStar 韩文客户端"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

pub(crate) fn series_options() -> Vec<SelectOption> {
    std::iter::once(option(
        FieldValue::Integer(-1),
        "禁用切换",
        "series=-1：不切换连战倍率",
    ))
    .chain(std::iter::once(option(
        FieldValue::Integer(0),
        "AUTO",
        "series=0：自动使用当前可用的最大连战次数",
    )))
    .chain((1..=6).map(|value| {
        option(
            FieldValue::Integer(value),
            &format!("固定 {value} 次"),
            &format!("series={value}：固定使用 {value} 次连战"),
        )
    }))
    .collect()
}

fn infrast_mode_options() -> Vec<SelectOption> {
    [
        (0, "默认模式", "单设施最优解；按 facility 顺序换班"),
        (10_000, "自定义排班", "读取 filename 指定的 JSON 排班计划"),
        (20_000, "一键轮换", "跳过控制中枢、电站、宿舍和办公室"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(FieldValue::Integer(value), label, description))
    .collect()
}

fn drone_options() -> Vec<SelectOption> {
    [
        ("_NotUse", "不使用", "不使用无人机加速"),
        ("Money", "贸易站：龙门币", "用于贸易站获取龙门币"),
        ("SyntheticJade", "贸易站：合成玉", "用于贸易站获取合成玉"),
        ("CombatRecord", "制造站：作战记录", "用于制造站生产作战记录"),
        ("PureGold", "制造站：赤金", "用于制造站生产赤金"),
        ("OriginStone", "制造站：源石", "用于制造站生产源石材料"),
        ("Chip", "制造站：芯片", "用于制造站生产芯片"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

fn medicine_expire_options() -> Vec<SelectOption> {
    std::iter::once(option(
        FieldValue::Integer(0),
        "不使用临期药",
        "medicine_expire_days=0：不使用即将到期的理智药",
    ))
    .chain((1..=7).map(|days| {
        let hours = days * 24;
        option(
            FieldValue::Integer(days),
            &format!("{hours} 小时内到期"),
            &format!("使用未来 {hours} 小时内到期的理智药；符合条件的药剂会持续使用"),
        )
    }))
    .collect()
}

fn recruit_star_options() -> Vec<SelectOption> {
    (1..=6)
        .rev()
        .map(|star| {
            option(
                FieldValue::Integer(star),
                &format!("{star}★"),
                &format!("允许选择 {star} 星级标签组合"),
            )
        })
        .collect()
}

fn facility_options() -> Vec<SelectOption> {
    [
        ("Mfg", "制造站", "Mfg"),
        ("Trade", "贸易站", "Trade"),
        ("Control", "控制中枢", "Control"),
        ("Power", "发电站", "Power"),
        ("Reception", "会客室", "Reception"),
        ("Office", "办公室", "Office"),
        ("Dorm", "宿舍", "Dorm"),
        ("Processing", "加工站", "Processing"),
        ("Training", "训练室", "Training"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

pub(super) fn copilot_formation_options() -> Vec<SelectOption> {
    std::iter::once(option(
        FieldValue::Integer(0),
        "当前编队",
        "formation_index=0：使用当前编队",
    ))
    .chain((1..=4).map(|index| {
        option(
            FieldValue::Integer(index),
            &format!("编队 {index}"),
            &format!("使用作业文件中的第 {index} 个编队"),
        )
    }))
    .collect()
}

fn extra_tags_options() -> Vec<SelectOption> {
    [
        (0, "默认", "默认标签选择策略"),
        (1, "强制三标签", "即使可能冲突也选择三个标签"),
        (2, "优先高星组合", "尽可能选择高星组合，即使可能冲突"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(FieldValue::Integer(value), label, description))
    .collect()
}

fn server_options() -> Vec<SelectOption> {
    [
        ("CN", "中国大陆", "向中国大陆统计服务汇报"),
        ("US", "美国", "向美国统计服务汇报"),
        ("JP", "日本", "向日本统计服务汇报"),
        ("KR", "韩国", "向韩国统计服务汇报"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

pub(super) fn support_usage_options() -> Vec<SelectOption> {
    [
        (0, "不使用助战", "不使用支援干员"),
        (1, "仅补一名缺失", "仅当恰好缺少一名干员时寻找助战"),
        (
            2,
            "缺失时补齐，否则指定",
            "缺少一名时补齐，否则使用指定助战干员",
        ),
        (
            3,
            "缺失时补齐，否则随机",
            "缺少一名时补齐，否则使用随机助战干员",
        ),
    ]
    .into_iter()
    .map(|(value, label, description)| option(FieldValue::Integer(value), label, description))
    .collect()
}

const EDITABLE_CONDITION_TYPES: [&str; 6] = [
    "Always",
    "Time",
    "DateTime",
    "Weekday",
    "DayMod",
    "OnSideStory",
];

pub(super) fn is_editable_condition_type(value: &FieldValue) -> bool {
    matches!(value, FieldValue::String(value) if EDITABLE_CONDITION_TYPES.contains(&value.as_str()))
}

pub(super) fn is_compound_condition_type(value: &str) -> bool {
    matches!(value, "And" | "Or" | "Not")
}

pub(crate) fn condition_type_options() -> Vec<SelectOption> {
    [
        ("Always", "始终", "始终匹配该变体"),
        ("Time", "时间段", "按每日时间段匹配"),
        ("DateTime", "日期时间", "按日期时间范围匹配"),
        ("Weekday", "星期", "按星期匹配"),
        ("DayMod", "日期周期", "按日期除数和余数匹配"),
        ("OnSideStory", "活动开放", "按客户端热更新活动状态匹配"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

pub(super) fn stage_options() -> Vec<SelectOption> {
    [
        (
            "",
            "当前/上次作战",
            "留空 stage，沿用游戏当前或最近一次作战入口",
        ),
        ("1-7", "主线 1-7", "固定选择主线 1-7"),
        ("CE-6", "CE-6 龙门币", "龙门币资源关"),
        ("LS-6", "LS-6 作战记录", "作战记录资源关"),
        ("CA-5", "CA-5 技能书", "技能概要资源关"),
        ("AP-5", "AP-5 采购凭证", "采购凭证资源关"),
        ("SK-5", "SK-5 建造材料", "建造材料资源关"),
        ("PR-A-2", "PR-A-2 芯片", "芯片资源关示例"),
        (
            "Annihilation",
            "剿灭 · 当期剿灭",
            "当期剿灭委托（MAA 固定取值 Annihilation）",
        ),
        (
            "Chernobog@Annihilation",
            "剿灭 · 切尔诺伯格",
            "切尔诺伯格剿灭（MAA 固定取值）",
        ),
        (
            "LungmenOutskirts@Annihilation",
            "剿灭 · 龙门外环",
            "龙门外环剿灭（MAA 固定取值）",
        ),
        (
            "LungmenDowntown@Annihilation",
            "剿灭 · 龙门市区",
            "龙门市区剿灭（MAA 固定取值）",
        ),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

pub(crate) fn string(value: &str) -> FieldValue {
    FieldValue::String(value.to_string())
}

fn strings(values: &[&str]) -> FieldValue {
    FieldValue::StringArray(values.iter().map(|value| (*value).to_string()).collect())
}

fn integers(values: &[i64]) -> FieldValue {
    FieldValue::IntegerArray(values.to_vec())
}

pub(super) fn looks_like_outdated_resource_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("tile-pos")
        || lower.contains("resources may be outdated")
        || lower.contains("unsupportedlevel")
        || text.contains("资源可能过旧")
        || text.contains("资源过旧")
}

pub(super) fn looks_like_ocr_to_as_t0_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("tile-pos") && lower.contains("t0-")
}

pub(super) fn hot_update_version_path() -> PathBuf {
    maa_hot_update_dir()
        .unwrap_or_else(|_| PathBuf::from(".local/share/maa/MaaResource"))
        .join("resource/version.json")
}

pub(super) fn read_hot_update_version() -> Option<HotUpdateVersion> {
    parse_hot_update_version(&fs::read_to_string(hot_update_version_path()).ok()?)
}

pub(crate) fn parse_hot_update_version(raw: &str) -> Option<HotUpdateVersion> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let activity_name = value
        .pointer("/activity/name")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_string();
    let last_updated = value
        .get("last_updated")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_string();
    if activity_name.is_empty() && last_updated.is_empty() {
        return None;
    }
    Some(HotUpdateVersion {
        activity_name,
        last_updated,
    })
}

pub(super) fn task_type_label(task_type: &str) -> &str {
    match task_type {
        "StartUp" => "启动游戏",
        "Recruit" => "公开招募",
        "Fight" => "刷理智",
        "Infrast" => "基建换班",
        "Mall" => "信用商店",
        "Award" => "领取奖励",
        "CloseDown" => "关闭游戏",
        "Copilot" => "自动战斗",
        _ => task_type,
    }
}
