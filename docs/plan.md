# 排期

| 里程碑 | 内容 | 状态 |
| --- | --- | --- |
| M0 | AGENTS + 配置 + Rust CLI 骨架 | 已完成 |
| M1 | start/stop/status/logs（含 --json）+ 浏览器直开 + 网页控制台 :9099 + add/remove | 已完成 |
| M2 | Tauri 桌面壳（列表 / 启停 / 分阶段状态 / 实时日志 / 项目设置 / Java·Node 版本），与 CLI、网页控制台共用 `tongyun-dev-core` | 已完成，待各平台真机验收 |
| M3 | 目录扫描（Spring Boot / Tomcat WAR / Vite / Vue CLI / Next / Nuxt / Python）+ 首次使用空状态；仓库只带示例配置 | 已完成 |
| M4 | 开源发布 | 待决策 |

决策：core 同时给 CLI、网页控制台和桌面壳用（[0001](decisions/0001-cli-first.md)）。
