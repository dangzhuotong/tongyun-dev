# tongyun-dev · AI 开发规则

所有在本仓库工作的 agent 先读本文件。

## 项目与修改入口

- 用途：本机开发服务登记 / 扫描 / 启停 / 状态 / 日志；服务 AI 与人。范围见 [docs/prd.md](docs/prd.md)。
- 技术栈：Rust `tongyun-dev-core` 放全部项目逻辑（启停、状态、日志、扫描、add/remove/edit、打开页面/目录）；CLI、网页控制台（`tongyun-dev ui`，:9099，`crates/tongyun-dev-cli/src/server.rs`）和 Tauri 2 + React 桌面壳都只调用它，不要在入口层重复实现。
- 这是要开源的通用工具：**仓库里不许出现任何真实的公司 / 个人项目名、本机路径、内网地址或端口表**。用户的项目清单只在 `~/.config/tongyun-dev/projects.toml`。示例、测试、文档一律用虚构名字（`demo_api`、`acme-web`、`~/code/demo/...`）。
- 密钥、数据库密码、Token：**不进仓、不打印**。

| 要改什么 | 先读 |
| --- | --- |
| 需求、验收 | [docs/prd.md](docs/prd.md) |
| 排期 | [docs/plan.md](docs/plan.md) |
| 命令、CI、配置字段 | [docs/development.md](docs/development.md) |
| 配置格式 | [config/projects.example.toml](config/projects.example.toml)、[config/projects.template.toml](config/projects.template.toml) |
| 扫描规则 | `crates/tongyun-dev-core/src/scan.rs` |
| 架构决定 | [docs/decisions/0001-cli-first.md](docs/decisions/0001-cli-first.md) |
| Agent 驱动（CLI / :9099） | [.agents/skills/tongyun-dev/SKILL.md](.agents/skills/tongyun-dev/SKILL.md) |

## 开发与验证

- `cargo test -p tongyun-dev-core -p tongyun-dev-cli`；`apps/desktop` 里 `npm run typecheck && npm test && npm run build`；`cargo +stable check -p tongyun-dev-desktop`。
- 改扫描：在 `scan.rs` 的测试里用临时目录造假项目覆盖新规则。
- 改启停逻辑：在真实本机用自己的一个项目跑 `start` / `stop` / `status`，核对进程、端口、探针。
- 改 JSON 输出：保证 `--json` 稳定字段，供 Agent 解析。
- 交接写清：改了什么、实际跑了哪些命令、结果、未跑项。
