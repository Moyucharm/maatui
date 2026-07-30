# MaaTUI

本地 [maa-cli](https://github.com/MaaAssistantArknights/maa-cli) 的轻量 TUI 控制壳。

只负责**调用 `maa`、展示日志、控制启停**，不复刻、不嵌入 MaaCore，也不解析任务配置。

## 功能（当前）

1. **每日任务** — 执行 `maa run daily -v`，运行中可手动停止  
2. **退出**

## 前置条件

- 已安装并配置好 `maa-cli`，`maa` 在 `PATH` 中
- 存在任务 `daily`（`maa list` 可确认）
- Linux 终端（使用进程组信号停止子进程）

## 构建与运行

```bash
cargo build --release
./target/release/maatui
# 或
cargo run --release
```

## 快捷键

| 按键 | 作用 |
|------|------|
| `↑` / `k` · `↓` / `j` | 空闲时切换菜单 |
| `Enter` | 确认当前菜单项 |
| `s` | 运行中停止任务 |
| `q` / `Esc` | 退出（运行中会先停止任务） |
| `PgUp` / `PgDn` | 滚动日志 |
| `Ctrl+u` / `Ctrl+d` | 滚动日志 |
| `Home` / `End` | 日志顶部 / 贴底自动滚动 |
| 鼠标滚轮 | 滚动日志 |

## 设计说明

- **壳与内核分离**：任务逻辑完全由 `maa` 负责
- **进程组停止**：`SIGTERM` → 超时后 `SIGKILL`，尽量清理子进程树
- **轻量**：同步主循环 + 日志读线程 + 环形缓冲（约 3000 行），无重型异步运行时

## License

按仓库后续约定；当前为个人/本地用途脚手架。
