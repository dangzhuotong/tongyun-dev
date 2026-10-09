# tongyun-dev · AI 开发规则

所有在本仓库工作的 agent 先读本文件。

## 项目与修改入口

- 用途：本机开发服务启停 / 状态 / 日志；服务 AI 与人。范围见 [docs/prd.md](docs/prd.md)。
- 技术栈：Rust CLI 优先；Tauri 2 + React 桌面壳后补。
- 密钥、数据库密码、Token：**不进仓、不打印**。项目路径写配置文件，可用环境变量覆盖。
- 不碰 服务器安全组；不依赖 IDEA Ultimate。

| 要改什么 | 先读 |
| --- | --- |
| 需求、验收 | [docs/prd.md](docs/prd.md) |
| 排期 | [docs/plan.md](docs/plan.md) |
| 命令、CI | [docs/development.md](docs/development.md) |
| 项目注册格式 | [config/projects.toml](config/projects.toml)、[docs/decisions/0001-cli-first.md](docs/decisions/0001-cli-first.md) |

## 开发与验证

- 改启停逻辑：在真实本机对 `legacy_app` 跑 `start` / `stop` / `status`（见 development.md），核对进程、8080、登录页。
- 改 JSON 输出：保证 `--json` 稳定字段，供 Agent 解析。
- 交接写清：改了什么、本机实际跑了哪些命令、结果、未跑项。

## 配套技能

| 任务 | 技能 |
| --- | --- |
| 需求/验收 | tong-product-pro |
| 测试 | tong-test-pro |
| PR 审查 | tong-code-review-pro |
| 项目规范 | tong-project-pro |
