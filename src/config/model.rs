//! 配置领域模型。

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    StringArray(Vec<String>),
    IntegerArray(Vec<i64>),
}

impl FieldValue {
    pub fn display(&self) -> String {
        match self {
            Self::Bool(value) => if *value { "开" } else { "关" }.to_string(),
            Self::Integer(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::String(value) => value.clone(),
            Self::StringArray(values) => values.join(", "),
            Self::IntegerArray(values) => values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    pub fn parse_like(input: &str, current: Option<&Self>) -> Result<Self> {
        match current {
            Some(Self::Bool(_)) => match input.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "on" | "yes" | "开" => Ok(Self::Bool(true)),
                "0" | "false" | "off" | "no" | "关" => Ok(Self::Bool(false)),
                _ => bail!("请输入 true/false、on/off 或 开/关"),
            },
            Some(Self::Integer(_)) => Ok(Self::Integer(
                input.trim().parse().context("请输入有效整数")?,
            )),
            Some(Self::Float(_)) => {
                Ok(Self::Float(input.trim().parse().context("请输入有效数字")?))
            }
            Some(Self::StringArray(_)) => Ok(Self::StringArray(
                input
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(ToOwned::to_owned)
                    .collect(),
            )),
            Some(Self::IntegerArray(_)) => Ok(Self::IntegerArray(
                input
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(|item| item.parse::<i64>().context("数组中包含无效整数"))
                    .collect::<Result<Vec<_>>>()?,
            )),
            _ => Ok(Self::String(input.to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskSummary {
    pub name: String,
    pub task_type: String,
    pub enabled: bool,
}
