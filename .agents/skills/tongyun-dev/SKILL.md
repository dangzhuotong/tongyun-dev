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
| `list [--json] [--with-groups]` | 已登记项目 | 先 list 再 start；搭配 `--with-groups` 返回带 groups 对象 |
| `status [id] [--json]` | 单项目或全部生命周期 | **解析 `--json`** |
| `start <id> [--timeout N] [--json] [--open]` | 启到端口通/探针成功；`--open` 就绪后开页 | 可能先 `installing` |
| `restart <id> [--timeout N] [--json] [--open]` | 停止成功后重新启动并等待就绪 | 停止失败不会继续启动 |
| `stop <id> [--json]` | 停进程组 | 失败也要读 status |
| `logs <id> [--follow] [--lines N] [--json]` | 读日志 | 大日志用 lines |
| `wait <id> --timeout N [--json]` | 观察已有服务直到就绪 | 不启动、不停止；超时返回非零 |
| `open <id> [--dir]` | 开页面或目录 | |
| `install <id> [--json]` | 装前端依赖 | |
| `input <id> [text] [--no-enter] [--key key]` | 向运行中交互脚本发送输入或按键 | 支持 `--json` |
| `scan <dir> [--depth N] [--apply] [--json]` | 扫描识别项目；`--apply` 写入配置 | 先无 apply 预览 |
| `add` / `remove` | 登记 / 删登记 | 不动项目文件 |
| `group <id> [key] [--clear]` | 设置或清空项目分组 | `--clear` 清空 |
| `group-add <key> [--label 名]` | 新建分组 | 写入 `[group.<key>]` |
| `ui` | 起控制台 | 默认 9099 |

## 3. `status --json` 稳定字段

单项目对象至少包含：

| 字段 | 含义 |
| --- | --- |
| `running` | 仅 `ready` / `port_open` 为 true；进程是否存活看 `pid` |
| `state` | `stopped` / `installing` / `starting` / `port_open` / `ready` / `failed` |
| `elapsed_secs` | 进入当前态时长 |
| `failure` | 失败原因（失败时） |
| `pid` / `port` | 进程与端口 |
| `login_probe` | 登录/健康探针（含 `http_code`、`ok` 等） |
| `log_path` | 日志路径 |
| `port_conflict` / `port_holder` | 端口冲突信息 |
| `hints` | 诊断建议（如 monorepo 脚本误用 `--port` 等，可选） |

Agent 驱动建议：`start <id> --timeout 60 --json` → 检查退出码和 `ok` → 需要时 `logs <id> --json`；超时后读 `status <id> --json`，可再 `wait <id> --timeout 30 --json` 或显式 `stop <id> --json`。不要把等待超时当成进程已停止。

启停 / 重启 / 安装 / 等待 / 日志的 JSON stdout 是单个 `{ok, action, id, status, logs, error}`。失败也尽量带当前状态；配置或项目不存在时 `status: null`。日志尚未生成时 `ok: true, logs.missing: true`。成功退出 0、操作失败 1、参数错误 2。参数错误只写 stderr。`--follow` / `--open` 不能和 `--json` 同用。`list` / `status` 的成功 JSON 结构不变（`list --json` 为项目数组，`list --json --with-groups` 为 `{"projects":[...],"groups":[...]}`）。

`start` / `restart` 的等待预算从启动命令返回后计算，不限制自定义脚本本身；慢探针可能额外约 5 秒。自动安装阶段等待失败会取消安装；只读 `wait` 不取消任何进程。

## 4. HTTP 控制台（:9099）

本机 `tongyun-dev ui` 后：

| 方法 | 路径 | 作用 |
| --- | --- | --- |
| GET | `/api/projects` | 概览 |
| GET | `/api/projects/{id}` | 项目设置详情 |
| PATCH | `/api/projects/{id}` | 部分更新项目设置 |
| GET | `/api/projects/{id}/logs` | 日志 |
| POST | `/api/projects/{id}/input` | 发送输入/按键（非运行中返回 409） |
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
