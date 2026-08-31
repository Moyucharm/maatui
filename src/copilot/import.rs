//! 作业来源解析、远程下载与导入。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use super::cache::{CopilotEntry, CopilotEntrySource, CopilotOrigin};
use crate::http::{USER_AGENT, agent};
use crate::storage::atomic_write;
use crate::tile_alias::StageAliasIndex;

const COPILOT_API: &str = "https://prts.maa.plus/copilot/get/";
const COPILOT_SET_API: &str = "https://prts.maa.plus/set/get?id=";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    Single,
    Set,
}

#[derive(Debug, Clone)]
pub struct ImportProgress {
    pub completed: usize,
    pub total: usize,
    pub label: String,
}

#[derive(Debug)]
pub struct ImportReport {
    pub entries: Vec<CopilotEntry>,
    pub errors: Vec<String>,
    pub set_id: Option<u64>,
    pub set_name: Option<String>,
    pub set_description: Option<String>,
}

impl ImportReport {
    fn single(entries: Vec<CopilotEntry>) -> Self {
        Self {
            entries,
            errors: Vec::new(),
            set_id: None,
            set_name: None,
            set_description: None,
        }
    }
}

pub fn import_source(
    input: &str,
    kind: ImportKind,
    files_dir: &Path,
    progress: impl FnMut(ImportProgress),
) -> Result<ImportReport> {
    let agent = copilot_agent();
    import_source_with_agent(input, kind, files_dir, progress, &agent)
}

fn import_source_with_agent(
    input: &str,
    kind: ImportKind,
    files_dir: &Path,
    mut progress: impl FnMut(ImportProgress),
    agent: &ureq::Agent,
) -> Result<ImportReport> {
    let alias_index = StageAliasIndex::load();
    match kind {
        ImportKind::Single => match parse_single_source(input)? {
            ParsedSingleSource::Remote(id) => {
                progress(ImportProgress {
                    completed: 0,
                    total: 1,
                    label: format!("下载作业 #{id}"),
                });
                let parsed = fetch_remote_copilot(agent, id, files_dir, &alias_index)?;
                progress(ImportProgress {
                    completed: 1,
                    total: 1,
                    label: format!("已解析 {}", parsed.stage_name),
                });
                Ok(ImportReport::single(parsed.into_entries(
                    CopilotEntrySource::Remote { id },
                    CopilotOrigin::Single,
                )))
            }
            ParsedSingleSource::Local(path) => {
                let canonical = fs::canonicalize(&path)
                    .with_context(|| format!("定位本地作业失败: {}", path.display()))?;
                let raw = fs::read_to_string(&canonical)
                    .with_context(|| format!("读取本地作业失败: {}", canonical.display()))?;
                let parsed = parse_copilot_content(&serde_json::from_str(&raw)?, &alias_index)?;
                progress(ImportProgress {
                    completed: 1,
                    total: 1,
                    label: format!("已解析 {}", parsed.stage_name),
                });
                Ok(ImportReport::single(parsed.into_entries(
                    CopilotEntrySource::Local { path: canonical },
                    CopilotOrigin::Single,
                )))
            }
        },
        ImportKind::Set => {
            let id = parse_set_code(input)?;
            let set = fetch_copilot_set(agent, id)?;
            let total = set.copilot_ids.len();
            let origin = CopilotOrigin::Set {
                id,
                name: non_empty(set.name.clone()),
            };
            let mut entries = Vec::new();
            let mut errors = Vec::new();
            for (offset, copilot_id) in set.copilot_ids.iter().copied().enumerate() {
                progress(ImportProgress {
                    completed: offset,
                    total,
                    label: format!("下载作业 #{copilot_id}"),
                });
                match fetch_remote_copilot(agent, copilot_id, files_dir, &alias_index) {
                    Ok(parsed) => entries.extend(parsed.into_entries(
                        CopilotEntrySource::Remote { id: copilot_id },
                        origin.clone(),
                    )),
                    Err(error) => errors.push(format!("#{copilot_id}: {error:#}")),
                }
                progress(ImportProgress {
                    completed: offset + 1,
                    total,
                    label: format!("已处理 {}/{}", offset + 1, total),
                });
            }
            Ok(ImportReport {
                entries,
                errors,
                set_id: Some(id),
                set_name: non_empty(set.name),
                set_description: non_empty(set.description),
            })
        }
    }
}

#[derive(Debug)]
pub(super) enum ParsedSingleSource {
    Remote(u64),
    Local(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ParsedRemoteCode {
    Legacy(u64),
    Single(u64),
    Set(u64),
}

pub(super) fn parse_remote_code(value: &str) -> Result<Option<ParsedRemoteCode>> {
    if let Some(code) = value.strip_prefix("prts://s") {
        return parse_remote_id(code, "作业集代码").map(|id| Some(ParsedRemoteCode::Set(id)));
    }
    if let Some(code) = value.strip_prefix("prts://") {
        return if let Some(code) = code.strip_suffix('s') {
            parse_remote_id(code, "作业集代码").map(|id| Some(ParsedRemoteCode::Set(id)))
        } else {
            parse_remote_id(code, "作业代码").map(|id| Some(ParsedRemoteCode::Single(id)))
        };
    }
    if let Some(code) = value.strip_prefix("maa://") {
        return if let Some(code) = code.strip_suffix('s') {
            parse_remote_id(code, "作业集代码").map(|id| Some(ParsedRemoteCode::Set(id)))
        } else {
            parse_remote_id(code, "作业代码").map(|id| Some(ParsedRemoteCode::Legacy(id)))
        };
    }
    Ok(None)
}

fn parse_remote_id(value: &str, label: &str) -> Result<u64> {
    if value.is_empty() {
        bail!("{label}为空");
    }
    value.parse().with_context(|| format!("{label}无效"))
}

pub(super) fn parse_single_source(input: &str) -> Result<ParsedSingleSource> {
    let value = input.trim();
    if value.is_empty() {
        bail!("请输入作业代码、maa:// URI 或本地 JSON 路径");
    }
    if value.chars().all(|character| character.is_ascii_digit()) {
        return Ok(ParsedSingleSource::Remote(value.parse()?));
    }
    match parse_remote_code(value)? {
        Some(ParsedRemoteCode::Legacy(id) | ParsedRemoteCode::Single(id)) => {
            return Ok(ParsedSingleSource::Remote(id));
        }
        Some(ParsedRemoteCode::Set(_)) => {
            bail!("这是作业集代码，请使用“添加作业集”");
        }
        None => {}
    }
    let path = value.strip_prefix("file://").unwrap_or(value);
    if Path::new(path).extension().is_some() || Path::new(path).is_file() {
        return Ok(ParsedSingleSource::Local(PathBuf::from(path)));
    }
    bail!("无法识别作业来源；支持纯数字、maa://、prts:// 和本地 JSON")
}

pub(super) fn parse_set_code(input: &str) -> Result<u64> {
    let value = input.trim();
    if value.is_empty() {
        bail!("请输入作业集代码");
    }
    if value.chars().all(|character| character.is_ascii_digit()) {
        return value.parse().context("作业集代码无效");
    }
    match parse_remote_code(value)? {
        Some(ParsedRemoteCode::Legacy(id) | ParsedRemoteCode::Set(id)) => Ok(id),
        Some(ParsedRemoteCode::Single(_)) => {
            bail!("这是单作业代码，请使用“添加单作业”")
        }
        None => bail!("无法识别作业集代码；支持纯数字、prts://s<id> 和旧 maa://<id>"),
    }
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    status_code: u16,
    message: Option<String>,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct SingleData {
    content: JsonValue,
}

#[derive(Debug, Deserialize)]
struct SetData {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    copilot_ids: Vec<u64>,
}

#[derive(Debug)]
pub(super) struct ParsedCopilot {
    content: JsonValue,
    stage_name: String,
    title: Option<String>,
    difficulty: i64,
}

impl ParsedCopilot {
    pub(super) fn into_entries(
        self,
        source: CopilotEntrySource,
        origin: CopilotOrigin,
    ) -> Vec<CopilotEntry> {
        let mut entries = Vec::new();
        let difficulty = if self.difficulty == 0 {
            1
        } else {
            self.difficulty
        };
        if difficulty & 1 != 0 {
            entries.push(CopilotEntry {
                enabled: true,
                stage_name: self.stage_name.clone(),
                title: self.title.clone(),
                is_raid: false,
                source: source.clone(),
                origin: origin.clone(),
            });
        }
        if difficulty & 2 != 0 {
            entries.push(CopilotEntry {
                enabled: true,
                stage_name: self.stage_name,
                title: self.title,
                is_raid: true,
                source,
                origin,
            });
        }
        entries
    }
}

pub(super) fn copilot_agent() -> ureq::Agent {
    agent()
}

fn fetch_remote_copilot(
    agent: &ureq::Agent,
    id: u64,
    files_dir: &Path,
    alias_index: &StageAliasIndex,
) -> Result<ParsedCopilot> {
    let url = format!("{COPILOT_API}{id}");
    let response = agent
        .get(&url)
        .set("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("请求作业失败: {url}"))?;
    let raw = response
        .into_string()
        .with_context(|| format!("读取作业响应失败: {url}"))?;
    let envelope: ApiResponse<SingleData> =
        serde_json::from_str(&raw).with_context(|| format!("解析作业响应失败: {url}"))?;
    if envelope.status_code != 200 {
        bail!(
            "作业站返回 {}: {}",
            envelope.status_code,
            envelope.message.unwrap_or_else(|| "作业不存在".to_string())
        );
    }
    let content = envelope.data.context("作业响应缺少 data")?.content;
    let content = match content {
        JsonValue::String(raw) => serde_json::from_str(&raw).context("解析嵌套作业 JSON 失败")?,
        value => value,
    };
    let parsed = parse_copilot_content(&content, alias_index)?;
    fs::create_dir_all(files_dir)
        .with_context(|| format!("创建作业缓存目录失败: {}", files_dir.display()))?;
    let path = files_dir.join(format!("{id}.json"));
    let raw = format!("{}\n", serde_json::to_string_pretty(&parsed.content)?);
    let trusted_root = files_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(files_dir);
    atomic_write(&path, trusted_root, raw.as_bytes())?;
    Ok(parsed)
}

fn fetch_copilot_set(agent: &ureq::Agent, id: u64) -> Result<SetData> {
    let url = format!("{COPILOT_SET_API}{id}");
    let response = agent
        .get(&url)
        .set("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("请求作业集失败: {url}"))?;
    let raw = response
        .into_string()
        .with_context(|| format!("读取作业集响应失败: {url}"))?;
    let envelope: ApiResponse<SetData> =
        serde_json::from_str(&raw).with_context(|| format!("解析作业集响应失败: {url}"))?;
    if envelope.status_code != 200 {
        bail!(
            "作业站返回 {}: {}",
            envelope.status_code,
            envelope
                .message
                .unwrap_or_else(|| "作业集不存在".to_string())
        );
    }
    envelope.data.context("作业集响应缺少 data")
}

pub(super) fn parse_copilot_content(
    content: &JsonValue,
    alias_index: &StageAliasIndex,
) -> Result<ParsedCopilot> {
    let object = content.as_object().context("作业内容不是 JSON 对象")?;
    if object
        .get("type")
        .and_then(JsonValue::as_str)
        .is_some_and(|task_type| task_type.eq_ignore_ascii_case("SSS"))
    {
        bail!("暂不支持 SSS/保全派驻作业");
    }
    let raw_stage_name = object
        .get("stage_name")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|stage| !stage.is_empty())
        .context("作业缺少 stage_name")?;
    let stage_name = alias_index
        .resolve(raw_stage_name)
        .unwrap_or_else(|| raw_stage_name.to_string());
    let difficulty = object
        .get("difficulty")
        .and_then(JsonValue::as_i64)
        .unwrap_or(0);
    if !(0..=3).contains(&difficulty) {
        bail!("作业 difficulty 必须为 0-3");
    }
    let title = content
        .pointer("/doc/title")
        .or_else(|| content.pointer("/documentation/title"))
        .and_then(JsonValue::as_str)
        .and_then(|title| non_empty(title.to_string()));
    Ok(ParsedCopilot {
        content: content.clone(),
        stage_name,
        title,
        difficulty,
    })
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}
