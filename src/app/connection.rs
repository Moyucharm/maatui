//! maa-cli 默认连接档案的加载与字段编辑。

use super::*;

impl App {
    pub(crate) fn reload_connection(&mut self) {
        match ConnectionConfig::load_default() {
            Ok(config) => {
                self.connection_config = Some(config);
                self.connection_error = None;
                self.last_failed = false;
                self.status_text = "已重载连接配置".to_string();
            }
            Err(error) => {
                self.connection_config = None;
                self.connection_error = Some(format!("{error:#}"));
                self.last_failed = true;
                self.status_text = "连接配置加载失败；修复文件后按 r 重载".to_string();
            }
        }
    }

    pub(crate) fn handle_connection_key(&mut self, key: KeyEvent) {
        match action_for(ShortcutContext::Connection, key) {
            Some(Action::Back) => self.screen = Screen::Main,
            Some(Action::Previous) => {
                self.connection_idx = self.connection_idx.saturating_sub(1);
            }
            Some(Action::Next) => {
                self.connection_idx = (self.connection_idx + 1).min(ConnectionField::ALL.len() - 1);
            }
            Some(Action::Edit) => self.edit_connection_field(),
            Some(Action::Reload) => self.reload_connection(),
            _ => {}
        }
    }

    fn edit_connection_field(&mut self) {
        if self.phase != TaskPhase::Idle {
            return;
        }
        let Some(config) = self.connection_config.as_ref() else {
            self.status_text = "连接配置不可用；修复文件后按 r 重载".to_string();
            return;
        };
        let field = ConnectionField::ALL[self.connection_idx.min(ConnectionField::ALL.len() - 1)];
        let current = config.value(field).unwrap_or_default().to_string();
        let title = format!("编辑 {}", field.label());
        let target = InputTarget::ConnectionField(field);
        match field {
            ConnectionField::AdbPath | ConnectionField::Address => {
                self.open_input(title, current, target);
            }
            ConnectionField::Config | ConnectionField::Preset => {
                let options = connection_options(field, &current);
                self.open_select(title, FieldValue::String(current), options, true, target);
            }
        }
    }

    pub(super) fn commit_connection_input(&mut self, field: ConnectionField, input: &str) {
        if self.phase != TaskPhase::Idle {
            self.status_text = "任务运行中，不能修改连接配置".to_string();
            return;
        }
        let result = self
            .connection_config
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("连接配置不可用；修复文件后按 r 重载"))
            .and_then(|config| config.set_field(field, input));
        match result {
            Ok(()) => {
                self.connection_error = None;
                self.last_failed = false;
                self.status_text = if input.trim().is_empty() {
                    format!("{}已恢复默认；下次任务生效", field.label())
                } else {
                    format!("{}已保存；下次任务生效", field.label())
                };
            }
            Err(error) => {
                self.connection_error = Some(format!("{error:#}"));
                self.last_failed = true;
                self.status_text = "连接配置保存失败；按 Enter/e 重试编辑".to_string();
            }
        }
    }
}

fn connection_options(field: ConnectionField, current: &str) -> Vec<SelectOption> {
    let definitions: &[(&str, &str, &str)] = match field {
        ConnectionField::Config => &[
            ("CompatPOSIXShell", "Linux 通用", "POSIX Shell 兼容连接配置"),
            ("Waydroid", "Waydroid", "Linux Waydroid 专用连接配置"),
            ("General", "通用", "MaaCore 通用连接配置"),
            ("CompatMac", "macOS 通用", "macOS 兼容连接配置"),
            ("BlueStacks", "蓝叠", "BlueStacks 模拟器"),
            ("MuMuEmulator12", "MuMu 12", "MuMu 模拟器 12"),
            ("LDPlayer", "雷电", "雷电模拟器"),
            ("Nox", "夜神", "夜神模拟器"),
            ("XYAZ", "逍遥", "逍遥模拟器"),
            ("WSA", "Windows 安卓子系统", "Windows Subsystem for Android"),
            ("Androws", "腾讯 Androws", "腾讯 Androws 模拟器"),
            ("AVD", "Android 虚拟设备", "Android Virtual Device"),
            ("CapWithShell", "Shell 截图", "使用 shell 命令截图"),
            ("Compatible", "兼容模式", "MaaCore 兼容连接配置"),
            ("SecondResolution", "非主显示屏", "使用非主显示屏的分辨率"),
        ],
        ConnectionField::Preset => &[
            ("ADB", "ADB", "常规 ADB 连接"),
            ("Waydroid", "Waydroid", "Linux，需 ADB"),
            ("MuMuPro", "MuMu Pro", "macOS 模拟器"),
            ("PlayCover", "PlayCover", "macOS，使用 PlayTools"),
            ("Androws", "腾讯 Androws", "需新版 maa-cli"),
        ],
        _ => unreachable!("仅连接配置与预设使用选择器"),
    };
    let mut options = Vec::with_capacity(definitions.len() + 2);
    options.push(SelectOption {
        value: FieldValue::String(String::new()),
        label: "默认 / 自动".to_string(),
        description: "使用 maa-cli 默认值".to_string(),
    });
    let mut matched = current.is_empty();
    for &(value, label, description) in definitions {
        // 预设接受不同大小写；打开选择器不应改写已有值。
        let is_current = current == value
            || (field == ConnectionField::Preset && current.eq_ignore_ascii_case(value));
        matched |= is_current;
        options.push(SelectOption {
            value: FieldValue::String(if is_current { current } else { value }.to_string()),
            label: label.to_string(),
            description: description.to_string(),
        });
    }
    if !matched {
        options.push(SelectOption {
            value: FieldValue::String(current.to_string()),
            label: format!("当前自定义值：{current}"),
            description: "保留当前值".to_string(),
        });
    }
    options
}
