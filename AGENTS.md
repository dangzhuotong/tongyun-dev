# tongyun-dev · AI 开发规则

所有在本仓库工作的 agent 先读本文件。

## 项目与修改入口

- 用途：本机开发服务启停 / 状态 / 日志；服务 AI 与人。范围见 [docs/prd.md](docs/prd.md)。
- 技术栈：Rust CLI 优先；Tauri 2 + React 桌面壳后补。
- 密钥、数据库密码、Token：**不进仓、不打印**。项目路径写配置文件，可用环境变量覆盖。
- 不碰 82 安全组；不依赖 IDEA Ultimate。

| 要改什么 | 先读 |
| --- | --- |
| 需求、验收 | [docs/prd.md](docs/prd.md)、[docs/testing/acceptance-tests.md](docs/testing/acceptance-tests.md) |
| 用户流程 | [docs/user-flows.md](docs/user-flows.md) |
| 排期 | [docs/plan.md](docs/plan.md) |
| 架构 | [docs/architecture.md](docs/architecture.md) |
| 命令、CI | [docs/development.md](docs/development.md) |
| 项目注册格式 | [config/projects.toml](config/projects.toml)、[docs/decisions/0001-cli-first.md](docs/decisions/0001-cli-first.md) |
| 测试与真机分界 | [docs/decisions/0002-ci-without-tomcat.md](docs/decisions/0002-ci-without-tomcat.md) |

## 开发与验证

- 改启停逻辑：CI 用临时目录和假探针，不启动 Tomcat。合入前仍要在真实 Mac 上对 `td_work` 按 development.md 的顺序跑 `status` → `start` → `logs` → `stop`，核对进程、8080、登录页。
- 改 JSON 输出：保证 `--json` 稳定字段，供 Agent 解析。键名以 prd 为准，单测锁住。
- 交接写清：改了什么、CI 实际跑了哪些命令、结果、真机未跑项。

## 配套技能

| 任务 | 技能 |
| --- | --- |
| 需求/验收 | tong-product-pro |
| 测试 | tong-test-pro |
| PR 审查 | tong-code-review-pro |
| 项目规范 | tong-project-pro |
