# 0001 · 先做 Rust CLI，Tauri 后补

- 背景：要尽快可用；Tauri 桌面壳脚手架与打包耗时长。
- 决定：先用 Rust CLI（`tongyun-dev` 命令）打通启停 / 状态 / 日志；JSON 状态给 AI；人用终端。Tauri 2 + React 桌面壳作为 M2。
- 后果：配置与项目模型放在 `tongyun-dev-core`，CLI 与桌面壳都调用它。

## 增补

- 桌面壳已补上（Tauri 2 + React），网页控制台（`tongyun-dev ui`，默认 :9099）也走同一个 core；`add` / `remove` / 编辑在 core 里写配置。
- 目录扫描（`scan.rs`）也放在 core：三个入口共用同一套识别规则和去重逻辑。
- 仓库只带虚构的示例配置；用户配置默认在 `~/.config/tongyun-dev/projects.toml`。
