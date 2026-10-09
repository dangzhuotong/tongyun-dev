# 0001 · 先做 Rust CLI，Tauri 后补

- 背景：明天就要能用；Tauri 桌面壳脚手架与打包耗时长。
- 决定：MVP 用 Rust CLI（`tongyun-dev` 命令）打通 legacy_app；JSON 状态给 AI；人用终端。Tauri 2 + React 与 tongyun-git 同系，作为 M2。
- 后果：配置与项目模型放在 `tongyun-dev-core`，CLI 与桌面壳都调用它。

## 增补

桌面壳已补上（Tauri 2 + React）。窗口只做项目列表、启动、停止、刷新状态、catalina 日志尾部。CLI 仍然可用。
