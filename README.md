# MaaTUI

本地 [maa-cli](https://github.com/MaaAssistantArknights/maa-cli) 的轻量 TUI 控制壳。

MaaTUI 负责调用 `maa`、编辑 `daily` 配置、展示分级日志和控制任务启停；不嵌入 MaaCore。

## 功能

### 每日任务

- 通过独立运行页执行 `maa run daily -v`
- 主菜单和配置编辑页不再挤占日志区域
- 运行中显示明确的停止操作
- `SIGTERM` 后超时使用 `SIGKILL` 清理整个进程组

### daily 配置管理

自动定位以下唯一一个配置文件：

- `~/.config/maa/tasks/daily.toml`
- `daily.yaml` / `daily.yml`
- `daily.json`

支持：

- 开启、关闭任务
- 上移、下移任务
- 新增和删除常用任务
- 编辑任务名称、基础参数和高级参数
- 客户端、连战次数、基建模式、无人机用途、临期理智药等使用带说明的选项
- 招募星级、设施顺序支持多选；普通文本仍保留自定义输入
- 关卡选择器支持常驻关卡、活动热更新目录、按接口时区计算的开放状态和掉落说明，也可输入自定义关卡
- 选择“当前/上次作战”会保存空 `stage`，由 MaaCore 识别游戏当前或最近一次作战入口
- 在任务编辑的关卡字段按 `v`，可创建 `OnSideStory` 活动变体；直接选择关卡仍保存为固定 `stage`
- 编辑、移动和删除条件变体；UI 可编辑基础条件，已有 `And` / `Or` / `Not` 组合条件会原样保留但暂不支持可视化修改
- 每个完成的操作自动原子保存

TOML 使用 `toml_edit` 修改，尽可能保留注释、布局和未知字段；YAML/JSON 会保留配置语义和未知字段，但保存后可能规范化排版。

### 自动战斗

执行 `maa copilot ... -v`，支持输入：

- 纯数字作业代码，例如 `123456`
- `maa://123456`
- `prts://123456`（由 MaaTUI 兼容转换为 `maa://`）
- 本地 JSON 路径或 `file://` URI

可配置：

- 普通、突袭、普通后突袭
- 自动编队和编队编号
- 使用理智药
- 补充低信赖干员
- 忽略练度要求
- 助战模式和指定助战干员
- 循环次数

`maa://<code>s` 作业集可以被识别，但当前版本只显示兼容提示，暂不执行。

### 分级日志

MaaTUI 强制使用 `MAA_LOG_PREFIX=Always` 获取可解析的日志等级，并分别显示：

- 普通：白色
- Info / 系统：青色
- Success：绿色
- Warn：黄色
- Error：红色
- Debug / Trace：灰色

不会再把 maa-cli 写入 `stderr` 的所有普通日志一律显示成黄色 `!`。

## 前置条件

- 已安装并配置 `maa-cli`
- `maa` 在 `PATH` 中
- 存在 `daily` 任务（`maa list` 可确认）
- Linux 终端（当前停止实现使用 Unix 进程组信号）

## 构建与运行

```bash
cargo build --release
./target/release/maatui
# 或
cargo run --release
```

## 快捷键

### 通用

| 按键 | 作用 |
|---|---|
| `↑/↓` 或 `j/k` | 移动选择 |
| `Enter` | 确认、编辑或执行 |
| `Esc` | 返回 |
| `q` | 返回或退出 |
| `PgUp/PgDn` | 滚动日志 |
| 鼠标滚轮 | 滚动日志 |

### 配置管理

| 按键 | 作用 |
|---|---|
| `Space` | 开启/关闭任务并自动保存 |
| `Enter` / `e` | 编辑任务 |
| `a` | 新增任务或变体 |
| `d` | 删除，需确认 |
| `Shift+↑/↓` | 移动任务或变体 |
| `←/→` 或 `h/l` | 切换基础、高级、条件菜单 |
| `r` | 从磁盘重新加载 daily 配置 |
| `v` | 在关卡字段创建 OnSideStory 活动变体 |

选择器中使用 `↑/↓` 移动、`Enter` 确认；多选使用 `Space` 勾选，允许自定义的字段使用 `c` 输入。关卡选择器可使用 `r` 刷新热更新目录。

### 任务运行

| 按键 | 作用 |
|---|---|
| `Enter` / `s` | 停止当前任务 |
| `q` / `Esc` | 停止任务并退出 |

## 验证

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

## 设计原则

- **壳与内核分离**：实际游戏自动化由 maa-cli / MaaCore 负责
- **安全写入**：配置先写同目录临时文件，再原子替换
- **未知字段保留**：避免 MaaCore 或 maa-cli 新字段被编辑器误删
- **轻量运行**：同步事件循环、日志读取线程和有界日志缓冲，无异步运行时
