# tongyun-dev PRD（MVP）

## 背景

本机同时跑多个开发项目。第一个要管的是 `td_work`：启动脚本同步资源并拉起 Tomcat，日志在 catalina 文件，进程号在 pid 文件，页面在 `127.0.0.1:8080`。人和 Agent 都要能启停、看状态、读日志，而且不依赖 IDEA Ultimate，也不改 82 安全组。

## 目标用户

- 在 macOS 上开发 `td_work` 的人。
- 本机上的 Agent：读 `--json`，不刮终端表格。

## 目标

- 一条命令列出已注册项目。
- `start` / `stop` / `status` / `logs` 覆盖 `td_work`。
- `status --json` 的字段名稳定，供 Agent 解析。
- 配置里写路径，支持 `~` 与 `{{root}}` / `{{tomcat}}` / `{{log_dir}}`，也可用环境变量按机器覆盖。
- 仓库和终端输出里不出现密钥、Token、数据库密码。

## 非目标（MVP 不做）

- 断点调试、远程调试。
- 依赖 IDEA Ultimate。
- 改 82 / 线上安全组。
- 多用户、云端托管、在 CI 里启动 Tomcat。
- Tauri 桌面壳（M2，见 [架构提案](architecture.md)）。

## 用户故事

1. 作为开发者，我能 `list` 看到已注册项目和展开后的根目录。
2. 作为开发者，我能 `start td_work`。配置的启动脚本同步资源并起 Tomcat；脚本返回后我能看到状态。登录页是否就绪以脚本和随后的 `status` 为准。
3. 作为开发者，我能 `stop td_work`。Tomcat 停干净、8080 不再监听时命令成功；超时仍占用则命令失败并说明 pid 与端口。
4. 作为开发者 / Agent，我能 `status td_work --json` 看到：是否在跑、pid、端口、端口是否打开、登录页探测、日志路径、pid 路径、项目根。
5. 作为开发者 / Agent，我能 `logs td_work` 读 catalina 末尾，并用 `--follow` 跟着新行。

## 功能需求

### 命令

| 命令 | 行为 | 成功时退出码 |
| --- | --- | --- |
| `list` | 按 id 字典序打印 `id`、名称、展开后的根目录 | 0 |
| `list --json` | JSON 数组，元素含 `id`、`name`、`root` | 0 |
| `start <id>` | 在项目根目录执行配置的 `start`；参数不回显 | 0 |
| `stop <id>` | 执行配置的 `stop`，再等到 pid 不在且端口关闭，最长 20 秒 | 0 |
| `status <id>` | 人读的一行摘要，外加 log / pidfile / root | 0 |
| `status <id> --json` | 稳定 JSON，见下表 | 0 |
| `logs <id>` | `tail -n <lines>`，默认 80 行 | 0 |
| `logs <id> --follow` | `tail -n <lines> -F`；Ctrl-C 结束 | 0 |

未知项目、配置缺失、启动或停止命令失败、停止超时、日志文件不存在：退出码非 0。错误写到 stderr。`--json` 只出现在 stdout。

`stop` 的命令本身失败时：若 pid 已不在且端口已关，视为已经停掉，命令仍成功。若进程或端口还在，把失败返回，且 **不把命令参数写进错误**。

### `status --json` 字段

空白与换行不稳定；键名和类型稳定。

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `id` | string | 配置里的项目 id |
| `name` | string | 显示名 |
| `running` | bool | pid 文件里的进程仍活着。只看 pid，不看端口 |
| `pid` | number 或 null | 活着的 pid；没有或已退出则为 null |
| `port` | number | 配置的端口 |
| `port_open` | bool | `127.0.0.1:port` 能连上。与 `running` 独立 |
| `login_probe.url` | string | 探测 URL |
| `login_probe.http_code` | number 或 null | HTTP 状态码；连不上为 null |
| `login_probe.ok` | bool | 状态码是 200、301 或 302 |
| `log_path` | string | 展开后的日志文件 |
| `pid_path` | string | 展开后的 pid 文件 |
| `root` | string | 展开后的项目根 |

pid 文件取第一行。空文件、非数字、`0` 都当作没有 pid。`running=false` 且 `port_open=true` 表示有东西占着端口，但 pid 文件对不上活进程。

### 配置

文件：`config/projects.toml`。查找顺序：

1. `--config`
2. 环境变量 `TONGYUN_DEV_CONFIG`
3. 从当前目录向上找 `config/projects.toml`

每个 `[projects.<id>]` 必填：`name`、`root`、`start`、`stop`、`tomcat`、`log_dir`、`log_file`、`pid_file`、`port`、`login_url`。`start` / `stop` 是非空字符串数组。未知标量字段忽略，便于以后加注释性字段。

路径展开：

- 只展开开头的 `~` 和 `~/`。`~other` 保持原样。
- `{{root}}`、`{{tomcat}}`、`{{log_dir}}` 各替换一次。替换进去的文本不再扫描，避免套娃。
- `root` 和 `tomcat` 只做 `~` 展开。
- `log_dir` 可以使用 `{{root}}` 和 `{{tomcat}}`，不要引用自己。
- 其余字段可以使用这三个占位符。

环境变量覆盖（空字符串忽略，且不打印取值）：

| 变量 | 覆盖 |
| --- | --- |
| `TONGYUN_DEV_<ID>_ROOT` | `root` |
| `TONGYUN_DEV_<ID>_TOMCAT` | `tomcat` |
| `TONGYUN_DEV_<ID>_LOG_DIR` | `log_dir` |

`<ID>` 是项目 id：`-` 换成 `_`，再变成大写。`td_work` → `TONGYUN_DEV_TD_WORK_ROOT`。

### `td_work` 默认值

| 项 | 值 |
| --- | --- |
| 根目录 | `~/code/testdaily/tdprojects` |
| 启动 | `bash {{root}}/scripts/start-td-work.sh`（脚本内部会先停再起） |
| 停止 | `catalina.sh stop 15 -force`，Tomcat 默认 `~/opt/apache-tomcat-9.0.98` |
| 日志 | `~/code/_index/logs/td_work_catalina.out` |
| pid | `~/code/_index/logs/td_work_tomcat.pid` |
| 端口 | 8080 |
| 登录页 | `http://127.0.0.1:8080/td_work/admin/login.html` |

## 安全

- 密钥、数据库密码、Token 不进仓库，不写进配置，不在命令参数里放。
- CLI 不打印环境变量，也不把 `start` / `stop` 的参数打到 stdout 或 stderr。
- 登录探测丢弃 curl 的 stderr。
- 不碰 82 安全组，不连线上机器。

## 成功标准

- CI（Ubuntu）上 `cargo fmt`、`clippy -D warnings`、`cargo test` 通过，且不启动 Tomcat。
- 装有 `td_work` 的 Mac 按 [development.md](development.md) 的顺序勾完：status → start → logs → stop。

自动化用例与真机用例分列在 [testing/acceptance-tests.md](testing/acceptance-tests.md)。
