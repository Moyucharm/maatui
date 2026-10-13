//! maa-cli 默认 profile 的连接覆盖项，保留未知配置并原子保存。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;
use toml_edit::{DocumentMut, Item, Table, value};

use crate::storage::{atomic_write, maa_config_dir};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionField {
    AdbPath,
    Address,
    Config,
    Preset,
}

impl ConnectionField {
    pub const ALL: [Self; 4] = [Self::AdbPath, Self::Address, Self::Config, Self::Preset];

    pub fn key(self) -> &'static str {
        match self {
            Self::AdbPath => "adb_path",
            Self::Address => "address",
            Self::Config => "config",
            Self::Preset => "preset",
        }
    }

    fn alias(self) -> Option<&'static str> {
        match self {
            Self::Address => Some("device"),
            Self::Preset => Some("type"),
            Self::AdbPath | Self::Config => None,
        }
    }

    fn keys(self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.key()).chain(self.alias())
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::AdbPath => "ADB 路径",
            Self::Address => "连接地址",
            Self::Config => "连接配置",
            Self::Preset => "连接预设",
        }
    }
}

#[derive(Debug, Clone)]
enum ConnectionDocument {
    Toml(DocumentMut),
    Yaml(YamlValue),
    Json(JsonValue),
}

#[derive(Debug)]
pub struct ConnectionConfig {
    path: PathBuf,
    config_dir: PathBuf,
    document: ConnectionDocument,
}

impl ConnectionConfig {
    pub fn load_default() -> Result<Self> {
        Self::load_from_config_dir(&maa_config_dir()?)
    }

    pub fn load_from_config_dir(config_dir: &Path) -> Result<Self> {
        let profiles = config_dir.join("profiles");
        let mut matches = profile_matches(&profiles, "default")?;
        if matches.is_empty() {
            matches = profile_matches(config_dir, "asst")?;
        }
        let (path, document) = match matches.as_slice() {
            [] => (
                profiles.join("default.toml"),
                ConnectionDocument::Toml(DocumentMut::new()),
            ),
            [path] => {
                let raw = fs::read_to_string(path)
                    .with_context(|| format!("读取默认 profile 失败: {}", path.display()))?;
                let document = match path.extension().and_then(|extension| extension.to_str()) {
                    Some("toml") => {
                        ConnectionDocument::Toml(raw.parse().context("解析默认 profile TOML 失败")?)
                    }
                    Some("yaml" | "yml") => ConnectionDocument::Yaml(
                        serde_yaml::from_str(&raw).context("解析默认 profile YAML 失败")?,
                    ),
                    Some("json") => ConnectionDocument::Json(
                        serde_json::from_str(&raw).context("解析默认 profile JSON 失败")?,
                    ),
                    _ => unreachable!(),
                };
                (path.clone(), document)
            }
            _ => bail!(
                "发现多个默认 profile，请只保留一个: {}",
                matches
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        let config = Self {
            path,
            config_dir: config_dir.to_path_buf(),
            document,
        };
        config
            .validate()
            .with_context(|| format!("默认 profile 无效: {}", config.path.display()))?;
        Ok(config)
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn value(&self, field: ConnectionField) -> Option<&str> {
        field.keys().find_map(|key| match &self.document {
            ConnectionDocument::Toml(document) => document
                .get("connection")?
                .as_table_like()?
                .get(key)?
                .as_str(),
            ConnectionDocument::Yaml(root) => root
                .as_mapping()?
                .get(YamlValue::String("connection".into()))?
                .as_mapping()?
                .get(YamlValue::String(key.into()))?
                .as_str(),
            ConnectionDocument::Json(root) => root.get("connection")?.get(key)?.as_str(),
        })
    }

    pub fn set_field(&mut self, field: ConnectionField, input: &str) -> Result<()> {
        let input = input.trim();
        // 修改副本；只有原子保存成功后才提交内存状态。
        let mut next = self.document.clone();
        match &mut next {
            ConnectionDocument::Toml(document) => {
                if !document.contains_key("connection") && !input.is_empty() {
                    document.insert("connection", Item::Table(Table::new()));
                }
                if let Some(connection) = document.get_mut("connection") {
                    let connection = connection
                        .as_table_like_mut()
                        .context("connection 不是对象")?;
                    if input.is_empty() {
                        for key in field.keys() {
                            connection.remove(key);
                        }
                    } else {
                        // 保留已有字段名（包括旧别名），避免引入 maa-cli 拒绝的重复字段。
                        let key = field
                            .keys()
                            .find(|key| connection.contains_key(key))
                            .unwrap_or(field.key());
                        let mut replacement = value(input);
                        // 替换已有值时保留其空白与行尾注释，包括 inline table。
                        if let Some(previous) = connection.get(key).and_then(Item::as_value) {
                            *replacement.as_value_mut().unwrap().decor_mut() =
                                previous.decor().clone();
                        }
                        connection.insert(key, replacement);
                    }
                }
            }
            ConnectionDocument::Yaml(root) => {
                let root = root
                    .as_mapping_mut()
                    .context("默认 profile 根节点不是对象")?;
                let section = YamlValue::String("connection".into());
                if !root.contains_key(&section) && !input.is_empty() {
                    root.insert(section.clone(), YamlValue::Mapping(Default::default()));
                }
                if let Some(connection) = root.get_mut(&section) {
                    let connection = connection.as_mapping_mut().context("connection 不是对象")?;
                    if input.is_empty() {
                        for key in field.keys() {
                            connection.remove(YamlValue::String(key.into()));
                        }
                    } else {
                        let key = field
                            .keys()
                            .find(|key| connection.contains_key(YamlValue::String((*key).into())))
                            .unwrap_or(field.key());
                        connection.insert(
                            YamlValue::String(key.into()),
                            YamlValue::String(input.into()),
                        );
                    }
                }
            }
            ConnectionDocument::Json(root) => {
                let root = root
                    .as_object_mut()
                    .context("默认 profile 根节点不是对象")?;
                if !root.contains_key("connection") && !input.is_empty() {
                    root.insert("connection".into(), JsonValue::Object(Default::default()));
                }
                if let Some(connection) = root.get_mut("connection") {
                    let connection = connection.as_object_mut().context("connection 不是对象")?;
                    if input.is_empty() {
                        for key in field.keys() {
                            connection.remove(key);
                        }
                    } else {
                        let key = field
                            .keys()
                            .find(|key| connection.contains_key(*key))
                            .unwrap_or(field.key());
                        connection.insert(key.into(), JsonValue::String(input.into()));
                    }
                }
            }
        }
        let content = match &next {
            ConnectionDocument::Toml(document) => document.to_string(),
            ConnectionDocument::Yaml(root) => serde_yaml::to_string(root)?,
            ConnectionDocument::Json(root) => format!("{}\n", serde_json::to_string_pretty(root)?),
        };
        fs::create_dir_all(self.path.parent().context("默认 profile 路径没有父目录")?)
            .context("创建 profiles 目录失败")?;
        atomic_write(&self.path, &self.config_dir, content.as_bytes())?;
        self.document = next;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        match &self.document {
            ConnectionDocument::Toml(document) => {
                if let Some(connection) = document.get("connection") {
                    let connection = connection.as_table_like().context("connection 不是对象")?;
                    validate_connection_fields(|key| {
                        connection.get(key).map(|value| value.as_str().is_some())
                    })?;
                }
            }
            ConnectionDocument::Yaml(root) => {
                let root = root.as_mapping().context("默认 profile 根节点不是对象")?;
                if let Some(connection) = root.get(YamlValue::String("connection".into())) {
                    let connection = connection.as_mapping().context("connection 不是对象")?;
                    validate_connection_fields(|key| {
                        connection
                            .get(YamlValue::String(key.into()))
                            .map(|value| value.as_str().is_some())
                    })?;
                }
            }
            ConnectionDocument::Json(root) => {
                let root = root.as_object().context("默认 profile 根节点不是对象")?;
                if let Some(connection) = root.get("connection") {
                    let connection = connection.as_object().context("connection 不是对象")?;
                    validate_connection_fields(|key| {
                        connection.get(key).map(JsonValue::is_string)
                    })?;
                }
            }
        }
        Ok(())
    }
}

/// 三种文档格式共用字段及别名校验，拒绝 maa-cli 无法反序列化的重复表示。
fn validate_connection_fields(is_string: impl Fn(&str) -> Option<bool>) -> Result<()> {
    for field in ConnectionField::ALL {
        let mut previous = None;
        for key in field.keys() {
            if let Some(is_string) = is_string(key) {
                if let Some(previous) = previous {
                    bail!("connection.{previous} 与 connection.{key} 重复，请只保留一个");
                }
                if !is_string {
                    bail!("connection.{key} 必须是字符串");
                }
                previous = Some(key);
            }
        }
    }
    Ok(())
}

fn profile_matches(directory: &Path, basename: &str) -> Result<Vec<PathBuf>> {
    let mut matches = Vec::new();
    for extension in ["toml", "yaml", "yml", "json"] {
        let path = directory.join(format!("{basename}.{extension}"));
        // 包括断开的符号链接，避免将无法读取的现有配置当作缺省配置覆盖。
        match fs::symlink_metadata(&path) {
            Ok(_) => matches.push(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("读取连接档案元信息失败: {}", path.display()));
            }
        }
    }
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "maatui-connection-{}-{timestamp}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn profile(&self, extension: &str, content: &str) -> PathBuf {
            let profiles = self.0.join("profiles");
            fs::create_dir_all(&profiles).unwrap();
            let path = profiles.join(format!("default.{extension}"));
            fs::write(&path, content).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn edits_four_formats_preserving_unknown_fields_and_clears_override() {
        for (extension, raw) in [
            (
                "toml",
                "# profile comment\nunknown = [1, 2]\n[connection]\naddress = 'old' # address comment\nfuture = { enabled = true }\n",
            ),
            (
                "yaml",
                "unknown: [1, 2]\nconnection:\n  address: old\n  future: {enabled: true}\n",
            ),
            (
                "yml",
                "unknown: [1, 2]\nconnection:\n  address: old\n  future: {enabled: true}\n",
            ),
            (
                "json",
                "{\"unknown\":[1,2],\"connection\":{\"address\":\"old\",\"future\":{\"enabled\":true}}}",
            ),
        ] {
            let dir = TempDir::new();
            let path = dir.profile(extension, raw);
            let mut config = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
            assert_eq!(config.path(), path);
            config
                .set_field(ConnectionField::Address, "  emulator-5554  ")
                .unwrap();
            let saved = fs::read_to_string(&path).unwrap();
            let root: JsonValue = match extension {
                "toml" => {
                    assert!(saved.contains("# profile comment"));
                    assert!(saved.contains("# address comment"));
                    let document: DocumentMut = saved.parse().unwrap();
                    assert_eq!(document["unknown"].as_array().unwrap().len(), 2);
                    assert_eq!(
                        document["connection"]["future"]["enabled"].as_bool(),
                        Some(true)
                    );
                    JsonValue::Null
                }
                "json" => serde_json::from_str(&saved).unwrap(),
                _ => serde_yaml::from_str(&saved).unwrap(),
            };
            if extension != "toml" {
                assert_eq!(root["unknown"], serde_json::json!([1, 2]));
                assert_eq!(root["connection"]["future"]["enabled"], true);
            }
            let mut reloaded = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
            assert_eq!(
                reloaded.value(ConnectionField::Address),
                Some("emulator-5554")
            );
            reloaded
                .set_field(ConnectionField::Address, " \t ")
                .unwrap();
            let cleared = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
            assert_eq!(cleared.value(ConnectionField::Address), None);
            assert!(!fs::read_to_string(path).unwrap().contains("address"));
        }
    }

    #[test]
    fn edits_inline_connection_and_accepts_custom_strings() {
        let dir = TempDir::new();
        let path = dir.profile(
            "toml",
            "connection = { address = 'USB-serial', future = 42 } # inline\n",
        );
        let mut config = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
        config
            .set_field(ConnectionField::Preset, "FuturePreset")
            .unwrap();
        config
            .set_field(ConnectionField::AdbPath, "/custom/adb")
            .unwrap();
        config
            .set_field(ConnectionField::Config, "CustomConfig")
            .unwrap();
        config.set_field(ConnectionField::Address, "").unwrap();
        let reloaded = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
        assert_eq!(
            reloaded.value(ConnectionField::Preset),
            Some("FuturePreset")
        );
        assert_eq!(
            reloaded.value(ConnectionField::AdbPath),
            Some("/custom/adb")
        );
        assert_eq!(
            reloaded.value(ConnectionField::Config),
            Some("CustomConfig")
        );
        let saved = fs::read_to_string(path).unwrap();
        assert!(saved.contains("# inline"));
        let document: DocumentMut = saved.parse().unwrap();
        assert!(document["connection"].as_inline_table().is_some());
        assert_eq!(document["connection"]["future"].as_integer(), Some(42));
    }

    #[derive(serde::Deserialize)]
    struct MaaConnection {
        #[serde(default, alias = "device")]
        address: Option<String>,
        #[serde(default, alias = "type")]
        preset: Option<String>,
    }

    fn parse_maa_connection(extension: &str, raw: &str) -> MaaConnection {
        let connection = match extension {
            "toml" => {
                let document: DocumentMut = raw.parse().unwrap();
                JsonValue::Object(
                    document["connection"]
                        .as_table_like()
                        .unwrap()
                        .iter()
                        .map(|(key, value)| {
                            (
                                key.to_string(),
                                JsonValue::String(value.as_str().unwrap().into()),
                            )
                        })
                        .collect(),
                )
            }
            "json" => serde_json::from_str::<JsonValue>(raw).unwrap()["connection"].clone(),
            _ => serde_yaml::from_str::<JsonValue>(raw).unwrap()["connection"].clone(),
        };
        // 与 maa-cli 相同的 serde 别名规则也会拒绝标准字段与别名同时存在。
        serde_json::from_value(connection).unwrap()
    }

    #[test]
    fn legacy_aliases_are_loaded_edited_and_cleared_across_formats() {
        for (extension, raw) in [
            (
                "toml",
                "[connection]\ndevice = 'old-device' # device comment\ntype = 'ADB' # type comment\nfuture = 'kept'\n",
            ),
            (
                "toml",
                "connection = { device = 'old-device', type = 'ADB', future = 'kept' } # inline\n",
            ),
            (
                "yaml",
                "connection: {device: old-device, type: ADB, future: kept}\n",
            ),
            (
                "yml",
                "connection: {device: old-device, type: ADB, future: kept}\n",
            ),
            (
                "json",
                r#"{"connection":{"device":"old-device","type":"ADB","future":"kept"}}"#,
            ),
        ] {
            let dir = TempDir::new();
            let path = dir.profile(extension, raw);
            let mut config = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
            assert_eq!(config.value(ConnectionField::Address), Some("old-device"));
            assert_eq!(config.value(ConnectionField::Preset), Some("ADB"));
            assert_eq!(fs::read_to_string(&path).unwrap(), raw, "加载不能迁移字段");

            config
                .set_field(ConnectionField::Address, "new-device")
                .unwrap();
            config
                .set_field(ConnectionField::Preset, "Waydroid")
                .unwrap();
            let saved = fs::read_to_string(&path).unwrap();
            let parsed = parse_maa_connection(extension, &saved);
            assert_eq!(parsed.address.as_deref(), Some("new-device"));
            assert_eq!(parsed.preset.as_deref(), Some("Waydroid"));
            assert!(
                !saved.contains("address") && !saved.contains("preset"),
                "{saved}"
            );
            assert!(saved.contains("kept"), "{saved}");
            if raw.contains("# device comment") {
                assert!(saved.contains("# device comment") && saved.contains("# type comment"));
            }
            if raw.contains("# inline") {
                assert!(saved.contains("# inline"));
            }
            let reloaded = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
            assert_eq!(reloaded.value(ConnectionField::Address), Some("new-device"));
            assert_eq!(reloaded.value(ConnectionField::Preset), Some("Waydroid"));

            config.set_field(ConnectionField::Address, " ").unwrap();
            config.set_field(ConnectionField::Preset, "").unwrap();
            let saved = fs::read_to_string(&path).unwrap();
            let parsed = parse_maa_connection(extension, &saved);
            assert!(
                parsed.address.is_none() && parsed.preset.is_none(),
                "{saved}"
            );
            assert!(
                !saved.contains("device") && !saved.contains("type"),
                "{saved}"
            );
            assert!(saved.contains("kept"), "{saved}");
            let reloaded = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
            assert_eq!(reloaded.value(ConnectionField::Address), None);
            assert_eq!(reloaded.value(ConnectionField::Preset), None);
        }
    }

    #[test]
    fn rejects_invalid_aliases_and_duplicate_fields_without_writing() {
        for extension in ["toml", "yaml", "yml", "json"] {
            for (alias, key) in [("device", "address"), ("type", "preset")] {
                for duplicate in [false, true] {
                    let raw = match (extension, duplicate) {
                        ("toml", false) => format!("[connection]\n{alias} = 42\n"),
                        ("toml", true) => format!("[connection]\n{alias} = 'old'\n{key} = 'new'\n"),
                        ("json", false) => format!(r#"{{"connection":{{"{alias}":42}}}}"#),
                        ("json", true) => {
                            format!(r#"{{"connection":{{"{alias}":"old","{key}":"new"}}}}"#)
                        }
                        (_, false) => format!("connection: {{{alias}: 42}}\n"),
                        (_, true) => format!("connection: {{{alias}: old, {key}: new}}\n"),
                    };
                    let dir = TempDir::new();
                    let path = dir.profile(extension, &raw);
                    let error = ConnectionConfig::load_from_config_dir(&dir.0).unwrap_err();
                    let message = format!("{error:#}");
                    assert!(message.contains(alias), "{message}");
                    assert!(
                        message.contains(if duplicate {
                            "重复"
                        } else {
                            "必须是字符串"
                        }),
                        "{message}"
                    );
                    assert_eq!(fs::read_to_string(path).unwrap(), raw);
                }
            }
        }
    }

    #[test]
    fn loading_missing_profile_creates_nothing_until_first_save() {
        let dir = TempDir::new();
        let root = dir.0.join("not-created");
        let mut config = ConnectionConfig::load_from_config_dir(&root).unwrap();
        assert!(!root.exists());
        for field in ConnectionField::ALL {
            assert_eq!(config.value(field), None);
        }
        config
            .set_field(ConnectionField::Address, "USB-serial")
            .unwrap();
        assert_eq!(config.path(), root.join("profiles/default.toml"));
        let reloaded = ConnectionConfig::load_from_config_dir(&root).unwrap();
        assert_eq!(reloaded.value(ConnectionField::Address), Some("USB-serial"));
        assert_eq!(reloaded.value(ConnectionField::Config), None);
    }

    #[test]
    fn rejects_multiple_default_profiles_without_writing() {
        let dir = TempDir::new();
        let toml = dir.profile("toml", "");
        let yaml = dir.profile("yaml", "{}");
        let error = ConnectionConfig::load_from_config_dir(&dir.0)
            .unwrap_err()
            .to_string();
        assert!(error.contains("多个默认 profile"));
        assert!(error.contains("default.toml") && error.contains("default.yaml"));
        assert_eq!(fs::read_to_string(toml).unwrap(), "");
        assert_eq!(fs::read_to_string(yaml).unwrap(), "{}");
    }

    #[test]
    fn rejects_malformed_documents_and_non_string_known_fields_without_writing() {
        for (extension, raw, expected) in [
            ("toml", "[connection", "解析"),
            ("yaml", "connection: [", "解析"),
            ("json", "{", "解析"),
            ("yaml", "[]", "根节点不是对象"),
            ("json", "null", "根节点不是对象"),
            ("toml", "connection = 'bad'", "connection 不是对象"),
            ("yaml", "connection: null", "connection 不是对象"),
            ("json", "{\"connection\":[]}", "connection 不是对象"),
            (
                "toml",
                "[connection]\nadb_path = 42",
                "connection.adb_path 必须是字符串",
            ),
            (
                "yaml",
                "connection: {address: false}",
                "connection.address 必须是字符串",
            ),
            (
                "json",
                "{\"connection\":{\"config\":null}}",
                "connection.config 必须是字符串",
            ),
            (
                "toml",
                "[connection]\npreset = []",
                "connection.preset 必须是字符串",
            ),
        ] {
            let dir = TempDir::new();
            let path = dir.profile(extension, raw);
            let error = ConnectionConfig::load_from_config_dir(&dir.0).unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
            assert_eq!(fs::read_to_string(path).unwrap(), raw);
        }
    }

    #[cfg(unix)]
    #[test]
    fn atomic_save_rejects_external_link_without_changing_memory_or_file() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new();
        let outside = TempDir::new();
        let raw = "[connection]\naddress = 'old'\n";
        let target = outside.0.join("profile.toml");
        fs::write(&target, raw).unwrap();
        fs::create_dir(dir.0.join("profiles")).unwrap();
        symlink(&target, dir.0.join("profiles/default.toml")).unwrap();
        let mut config = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
        let error = config
            .set_field(ConnectionField::Address, "new")
            .unwrap_err();
        assert!(error.to_string().contains("拒绝"));
        assert_eq!(config.value(ConnectionField::Address), Some("old"));
        assert_eq!(fs::read_to_string(target).unwrap(), raw);
        assert!(
            fs::symlink_metadata(config.path())
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    #[test]
    fn falls_back_to_legacy_formats_without_shadowing_existing_configuration() {
        for (extension, raw) in [
            (
                "toml",
                "[resource]\npath = 'custom'\n[instance_options]\ntouch_mode = 'maatouch'\n[connection]\naddress = 'old'\n",
            ),
            (
                "yaml",
                "resource: {path: custom}\ninstance_options: {touch_mode: maatouch}\nconnection: {address: old}\n",
            ),
            (
                "yml",
                "resource: {path: custom}\ninstance_options: {touch_mode: maatouch}\nconnection: {address: old}\n",
            ),
            (
                "json",
                "{\"resource\":{\"path\":\"custom\"},\"instance_options\":{\"touch_mode\":\"maatouch\"},\"connection\":{\"address\":\"old\"}}",
            ),
        ] {
            let dir = TempDir::new();
            let path = dir.0.join(format!("asst.{extension}"));
            fs::write(&path, raw).unwrap();
            let mut config = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
            assert_eq!(config.path(), path);
            config
                .set_field(ConnectionField::Address, "USB-serial")
                .unwrap();
            assert!(!dir.0.join("profiles").exists());
            let reloaded = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
            assert_eq!(reloaded.value(ConnectionField::Address), Some("USB-serial"));
            let saved = fs::read_to_string(path).unwrap();
            match extension {
                "toml" => {
                    let root: DocumentMut = saved.parse().unwrap();
                    assert_eq!(root["resource"]["path"].as_str(), Some("custom"));
                    assert_eq!(
                        root["instance_options"]["touch_mode"].as_str(),
                        Some("maatouch")
                    );
                }
                _ => {
                    let root: JsonValue = if extension == "json" {
                        serde_json::from_str(&saved).unwrap()
                    } else {
                        serde_yaml::from_str(&saved).unwrap()
                    };
                    assert_eq!(root["resource"]["path"], "custom");
                    assert_eq!(root["instance_options"]["touch_mode"], "maatouch");
                }
            }
        }
    }

    #[test]
    fn default_profile_takes_precedence_even_over_conflicting_invalid_legacy_files() {
        let dir = TempDir::new();
        let path = dir.profile("toml", "[connection]\naddress = 'default-device'\n");
        fs::write(dir.0.join("asst.toml"), "invalid[").unwrap();
        fs::write(dir.0.join("asst.json"), "null").unwrap();
        let mut config = ConnectionConfig::load_from_config_dir(&dir.0).unwrap();
        assert_eq!(config.path(), path);
        assert_eq!(
            config.value(ConnectionField::Address),
            Some("default-device")
        );
        config
            .set_field(ConnectionField::Address, "new-device")
            .unwrap();
        assert_eq!(
            fs::read_to_string(dir.0.join("asst.toml")).unwrap(),
            "invalid["
        );
        assert_eq!(fs::read_to_string(dir.0.join("asst.json")).unwrap(), "null");
    }

    #[test]
    fn rejects_conflicting_legacy_profiles_without_creating_default() {
        let dir = TempDir::new();
        fs::write(dir.0.join("asst.yaml"), "{}").unwrap();
        fs::write(dir.0.join("asst.yml"), "{}").unwrap();
        let error = ConnectionConfig::load_from_config_dir(&dir.0)
            .unwrap_err()
            .to_string();
        assert!(error.contains("asst.yaml") && error.contains("asst.yml"));
        assert!(!dir.0.join("profiles").exists());
    }
}
