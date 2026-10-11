---
name: tongyun-dev
description: >-
  Drive local tongyun-dev (CLI / :9099 console / desktop) to scan, start, stop,
  and inspect registered projects. Use when an agent must run or check a local
  app via tongyun-dev rather than ad-hoc shell scripts.
---

# tongyun-dev · Agent 驱动说明（项目级）

本 skill **只放在本仓库** `.agents/skills/tongyun-dev/`，不要拷进全局 skills。人用桌面 App / 网页控制台；Agent 优先用 **CLI + `--json`**，必要时用本机 HTTP `:9099`。

先读仓库根 [AGENTS.md](../../../AGENTS.md)、[README.md](../../../README.md)、[docs/development.md](../../../docs/development.md)。配置与日志只在用户本机：`~/.config/tongyun-dev/`、`~/.tongyun-dev/logs`（可用配置改 `log_dir`）。**仓库内只有虚构示例，禁止写入真实公司路径/项目名。**

## 1. 二进制与入口

```bash
# 开发构建产物示例（路径按本机实际）
./target/debug/tongyun-dev <命令> [--json]
tongyun-dev ui [--port 9099]   # 网页控制台，默认 http://127.0.0.1:9099
```

配置查找顺序：`--config` → `TONGYUN_DEV_CONFIG` → `~/.config/tongyun-dev/settings.json` 记住的路径 → `~/.config/tongyun-dev/projects.toml` → 向上找 `config/projects.toml`（仅调试）。

## 2. CLI 命令表（Agent 稳定面）

| 命令 | 作用 | Agent 注意 |
| --- | --- | --- |
| `list [--json]` | 已登记项目 | 先 list 再 start |
| `status [id] [--json]` | 单项目或全部生命周期 | **解析 `--json`** |
| `start <id> [--open]` | 启到端口通/探针成功；`--open` 就绪后开页 | 可能先 `installing` |
| `restart <id> [--open]` | 停止成功后重新启动并等待就绪 | 停止失败不会继续启动 |
| `stop <id>` | 停进程组 | 失败也要读 status |
| `logs <id> [--follow] [--lines N]` | 读日志 | 大日志用 lines |
| `open <id> [--dir]` | 开页面或目录 | |
| `install <id>` | 装前端依赖 | |
| `scan <dir> [--depth N] [--apply] [--json]` | 扫描识别项目；`--apply` 写入配置 | 先无 apply 预览 |
| `add` / `remove` | 登记 / 删登记 | 不动项目文件 |
| `ui` | 起控制台 | 默认 9099 |

## 3. `status --json` 稳定字段

单项目对象至少包含：

| 字段 | 含义 |
| --- | --- |
| `running` | 是否有进程 |
| `state` | `stopped` / `installing` / `starting` / `port_open` / `ready` / `failed` |
| `elapsed_secs` | 进入当前态时长 |
| `failure` | 失败原因（失败时） |
| `pid` / `port` | 进程与端口 |
| `login_probe` | 登录/健康探针（含 `http_code`、`ok` 等） |
| `log_path` | 日志路径 |
| `port_conflict` / `port_holder` | 端口冲突信息 |

Agent 驱动建议：`start` → 轮询 `status --json` 直到 `state` 为 `ready` 或 `failed`（设超时）→ 需要时 `logs` → 结束 `stop`。

## 4. HTTP 控制台（:9099）

本机 `tongyun-dev ui` 后：

| 方法 | 路径 | 作用 |
| --- | --- | --- |
| GET | `/api/projects` | 概览 |
| GET | `/api/projects/{id}` | 项目设置详情 |
| PATCH | `/api/projects/{id}` | 部分更新项目设置 |
| GET | `/api/projects/{id}/logs` | 日志 |
| POST | `/api/projects/{id}/start\|stop\|restart\|install\|open\|open-dir` | 动作 |
| POST | `/api/projects` | 新增登记 |
| DELETE | `/api/projects/{id}` | 删登记 |
| POST | `/api/scan` | 扫描预览 |
| POST | `/api/scan/import` | 扫描结果写入配置 |

响应多为 JSON；动作失败时仍可能 HTTP 200 且 `ok: false`，以 body 为准。

## 5. 禁区

- 不把真实项目路径、内网地址、密钥写进仓库或本 skill。
- 不 source sdkman/fnm 初始化脚本启项目；版本由 core 注入 `JAVA_HOME`/`PATH`。
- 本 skill 不是 MCP；外部 Agent 靠 CLI/`--json` 或本机 HTTP。
