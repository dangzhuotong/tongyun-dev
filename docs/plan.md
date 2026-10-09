# 排期

决策：先 CLI，明天能用。见 [0001](decisions/0001-cli-first.md)。CI 不启动 Tomcat，真机验收分开，见 [0002](decisions/0002-ci-without-tomcat.md)。

## 原则

- M1 只交付本机 CLI 和稳定 JSON。桌面壳不挡这条路径。
- 配置和状态模型放在 `tongyun-dev-cli` 库里，M2 的界面复用，不另写一套启停。
- 能在临时目录和假探针上验证的行为进 CI。进程、8080、登录页留在 Mac 真机清单。

## 里程碑

| 里程碑 | 交付 | 完成定义 |
| --- | --- | --- |
| M0 | 私有仓、AGENTS.md、`config/projects.toml`、CLI 骨架 | 已完成 |
| M1 | `list` / `start` / `stop` / `status --json` / `logs --follow`；路径展开；单测与集成测试；GitHub Actions | 代码与 CI 绿；Mac 清单由使用的人勾 |
| M2 | 多项目注册体验、Tauri 2 + React 面板 | 复用同一库的状态 JSON，不在 M1 里做 |
| M3 | 是否把仓库公开 | 问阿橦，现在保持私有 |

## M1 本轮范围

纳入：

- 文档补齐：PRD、排期、架构提案、开发与 CI、用户流程、验收用例、ADR。
- CLI 行为与 [prd.md](prd.md) 的命令、JSON、配置一致。
- 测试不依赖真实 Tomcat、不占用 8080。

不纳入：

- 在云端或 CI 安装 / 启动 Tomcat，或克隆 `td_work`。
- Tauri 工程、安装包、托盘图标。
- 改 `td_work` 仓库里的 `scripts/start-td-work.sh`。那个脚本不在本仓。

## M1 剩余

Mac 上对真实 `td_work` 跑 [development.md](development.md) 的验收清单。CI 绿不能代替这一步：启动脚本、catalina 停干净、登录页 200 都只在那台机器上发生。

## 风险

| 风险 | 影响 | 处理 |
| --- | --- | --- |
| 启动脚本的就绪时机不在本仓 | `start` 返回时页面可能还没 200 | `start` 不假装等待登录页；用随后的 `status --json` 看 `login_probe` |
| pid 文件和实际监听进程不一致 | `running` 与 `port_open` 一个真一个假 | 两个字段都返回，不合并成一个布尔 |
| 停止超时仍占着 8080 | 命令失败 | 20 秒后非 0，stderr 写明 `pid_alive` 和 `port_open` |
| 本机路径和默认 `~` 不同 | 提交的 toml 不能每人一份 | 用 `TONGYUN_DEV_TD_WORK_*` 覆盖，不把差异提交进去 |
