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

## 代码审查规矩

1. **本地测试进 PR**：改代码必须在本地跑相关测试，把测试命令和输出贴进 PR 描述（或附件）。没跑过别声称通过。
2. **提 PR 前自查 diff**：提交前自己过一遍 diff，重点看正确性、兼容（旧接口/旧数据）、权限、生命周期（启停、清理、泄漏）。
3. **已知坑沉淀**：同类问题第二次出现，就写进本文件「已知坑」一节（没有就新建），避免反复踩。
4. **AI 改稿轮次上限**：AI 修改来回审超过 3 轮还没过，停下重想方案并上报（阿码/阿审），不要硬改硬怼。
5. **风险分级合入**：
   - **低风险**：文档、测试的小改动，CI 绿可直接合。
   - **功能改动**：要阿审审过再合。
   - **高风险（必须阿橦本人批）**：计费、密钥相关、CI/发布流程（`.github/workflows`、updater、签名）的改动。

不要把密钥、Token、密码写进本文件或仓库任何位置。

## 已知坑

- **不要用系统钥匙串存应用密钥。** macOS 上未签名、或每次更新后签名变了的桌面壳，用 `keyring` / Security.framework 密码项 / Stronghold 默认钥匙串时，读或写密钥都会弹出登录钥匙串密码，「始终允许」也留不住。密钥只走 `crates/tongyun-dev-core/src/secret_store.rs`：配置目录里 mode `0600` 的加密文件，密钥由本机随机种子经 HKDF 派生，不调用钥匙串。旧钥匙串项只在迁移时读一次，确认加密文件能读回之后才尝试删除；删不掉也保留文件，之后不再读钥匙串。本仓库的项目设置、`settings.json` 和主题本来就不写钥匙串。这个文件是单模块，tongyun-git / tongyun-api 按原样移植。
