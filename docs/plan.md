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

- 已补：三端统一重启、网页项目设置、重复启动保护、停止结果校验、Windows 进程管理与跨平台兼容性（Toolhelp32 快照进程树与 netstat 端口监听检测、taskkill 树终止、cmd/PowerShell 解包、TOML/JSON 路径规范化）、多环境预设（`--profile`、动态重写端口与探针、active_profile 反映）、快捷网页链接与全开（`links`、`open --all`、多链接折叠菜单）、系统原生终端拉起（`terminal` 命令与跨平台命令转义）、旧日志/PID 目录迁移引导与停止清理、项目分组（`members` 与 `group` 字段、分组管理）、批量启停（CLI `--all` / `--group`、Web API 与控制台本组/全部启停、桌面端全启/全停与组级启停）、依赖启动拓扑排序（`depends_on` DAG 排序与逆向停止、环路检测、依赖失败跳过）、CLI add/edit 参数对齐（`--link`, `--default-profile`, `--depends-on` 及清除参数）、Web 控制台与桌面端设置表单完整对齐（pty, env, auto_answers, port_mode, depends_on）、子进程输出与自动应答前缀解耦（`› [`）、macOS/Linux CI。
- 已补：分组按 `[group.<name>].members` 顺序启动并等前一个就绪（`tongyun-dev start <组名>`，网页和桌面分组卡片「全部启动」）；扫描按已有 `application-*.yml` 建议 profile / env；非 Spring Boot 的 jar 给出 mainClass、端口、packaging 候选且不默认停用，非 Spring WAR 仍先停用并在未填 `tomcat` 时拒绝把 `{{tomcat}}` 空替换成 `/bin/catalina.sh`；启动失败从日志抽出一行原因；项目卡片可打开 Cursor、打开日志文件、复制启动命令。
- 待完成：各平台 GUI 真机验收、签名发布。
- 交互输入后续（#8 合入后非阻塞备注，2026-10-12）：
  - pty=true 时 Unix 走 openpty relay（macOS 与 Linux 同一路径）：输入仍写 FIFO，由 relay 转发到 PTY。管道满时仍可能返回「子进程没有读取输入（管道已满）」；大输入/子进程不读 stdin 的表现需单独观察。
  - openpty relay；Mac 验收：安装后用一个虚构项目 `pty = true` 实跑 start/stop，确认没有 tcgetattr / Operation not supported、isatty 为真、input 可达、stop 后进程组与 FIFO 清理。

## AI 与人工共用运行面板

- 已补：启停/重启/安装/日志 JSON 操作结果、明确退出码、只观察服务的限时 wait、启动就绪等待预算、自动安装超时取消、端口冲突启动保护、失败进程仍存活提示。
- 边界：不扩展为通用测试或部署平台，选择测试和分析错误由 AI 完成。
