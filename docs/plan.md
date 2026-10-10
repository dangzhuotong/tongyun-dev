# 排期

| 里程碑 | 内容 | 目标 | 状态 |
| --- | --- | --- | --- |
| M0 | 私有仓 + AGENTS + 配置 + Rust CLI 骨架 | 当天 | 已完成 |
| M1 | legacy_app start/stop/status/logs（含 --json）+ 浏览器直开 + 网页控制台 :9099 + add/remove | **明天可用** | 已落地 |
| M2 | Tauri 桌面壳（列表 / 启停 / 刷新 / 日志 / 停用灰显 / 端口冲突 / node·java 版本），与 CLI、网页控制台共用 `tongyun-dev-core` | 已接上 | 待 Mac 真机验收 |
| M3 | 是否公开仓 | 问维护者 | 待决策 |

决策：core 同时给 CLI、网页控制台和桌面壳用（[0001](decisions/0001-cli-first.md)）。
