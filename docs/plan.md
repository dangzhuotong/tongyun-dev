# 排期

| 里程碑 | 内容 | 状态 |
| --- | --- | --- |
| M0 | AGENTS + 配置 + Rust CLI 骨架 | 已完成 |
| M1 | start/stop/status/logs（含 --json）+ 浏览器直开 + 网页控制台 :9099 + add/remove | 已完成 |
| M2 | Tauri 桌面壳（列表 / 启停 / 分阶段状态 / 实时日志 / 项目设置 / Java·Node 版本），与 CLI、网页控制台共用 `tongyun-dev-core` | 已完成，待各平台真机验收 |
| M3 | 目录扫描（Spring Boot / Tomcat WAR / Vite / Vue CLI / Next / Nuxt / Python）+ 首次使用空状态；仓库只带示例配置 | 已完成 |
| M4 | 开源发布 | 待决策 |

决策：core 同时给 CLI、网页控制台和桌面壳用（[0001](decisions/0001-cli-first.md)）。

## 后续完善

- 已补：三端统一重启、网页项目设置、重复启动保护、停止结果校验、Windows 进程管理与跨平台兼容性（tasklist/taskkill 树终止、cmd/PowerShell 解包、TOML/JSON 路径规范化）、macOS/Linux CI。
- 待完成：旧日志/PID 目录迁移引导、各平台 GUI 真机验收、签名发布。
- 后续产品功能：项目分组、批量启停、依赖启动顺序。
- 交互输入后续（#8 合入后非阻塞备注，2026-10-12）：
  - 子进程自身输出若以「› 」开头，会被 auto-answer 扫描跳过（当前约定回显占位符也用该前缀）；若项目日志里确实会出现这种行，需改跳过规则或换回显前缀。
  - pty=true 时走 `script` 包一层，管道满时不会走到 FIFO 的 WouldBlock 提示；大输入/子进程不读 stdin 的表现需单独观察。
  - macOS 的 `script -q /dev/null <argv...>` 分支尚未在真机跑过：阿装装机验收时用一个 `pty = true` 的虚构项目实跑一次（确认 isatty、input 可达、stop 后进程与 FIFO 清理）。

## AI 与人工共用运行面板

- 已补：启停/重启/安装/日志 JSON 操作结果、明确退出码、只观察服务的限时 wait、启动就绪等待预算、自动安装超时取消、端口冲突启动保护、失败进程仍存活提示。
- 边界：不扩展为通用测试或部署平台，选择测试和分析错误由 AI 完成。
