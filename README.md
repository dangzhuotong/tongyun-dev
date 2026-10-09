# tongyun-dev

本地开发机控制台：注册项目、启停、看状态、跟日志。对标 IDEA 运行面板，**人能看面板，AI 能读 JSON 状态和日志流**。

- 仓库：私有，MVP 做完再问是否公开
- 技术栈：先 **Rust CLI**（明天可用）；桌面壳（Tauri 2 + React）后补，同系 tongyun-git
- 第一个内置项目：`td_work`（TestDaily 工作平台）

## 快速用（MVP）

```bash
cargo run -p tongyun-dev-cli -- status td_work
cargo run -p tongyun-dev-cli -- start td_work
cargo run -p tongyun-dev-cli -- stop td_work
cargo run -p tongyun-dev-cli -- logs td_work --follow
cargo run -p tongyun-dev-cli -- status td_work --json
```

配置见 `config/projects.toml`（路径可按机器改，勿把密钥写进仓）。

## 文档

入口是 [AGENTS.md](AGENTS.md)。

- [docs/prd.md](docs/prd.md) — 产品范围与 JSON / 配置契约
- [docs/plan.md](docs/plan.md) — 排期
- [docs/architecture.md](docs/architecture.md) — 架构提案（M1 CLI 已按此实现）
- [docs/development.md](docs/development.md) — 命令、CI、Mac 验收清单
- [docs/user-flows.md](docs/user-flows.md) — 用户流程
- [docs/testing/acceptance-tests.md](docs/testing/acceptance-tests.md) — 自动化与真机用例
- [docs/decisions/](docs/decisions/) — ADR

CI 只跑 `cargo fmt`、`clippy`、`cargo test`，不启动 Tomcat。Mac 上的 status → start → logs → stop 见 development.md。
