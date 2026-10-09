# 架构（提案）

状态：M1 的 CLI 按本文实现。Tauri 桌面壳只定复用边界，本里程碑不写界面。

本文同时是提案和当前实现说明。和代码不一致时，以代码和测试为准，并回写本文。

## 上下文

```text
人 / Agent
    |
    |  argv、--json、日志字节
    v
tongyun-dev（二进制）
    |
    v
tongyun-dev-cli（库）
    |-- 读 config/projects.toml，应用环境变量
    |-- 展开 ~ 与 {{root}} {{tomcat}} {{log_dir}}
    |-- Probe：pid、127.0.0.1 端口、登录页 HTTP
    |-- 执行 start / stop argv，tail 日志
    v
本机进程与文件（td_work 的脚本、Tomcat、catalina、pid）
```

CI 和单元测试停在库这一层：临时目录、假 `Probe`、或本机的 `TcpListener`。它们不调用 `td_work` 的启动脚本。

## 模块

| 模块 | 职责 |
| --- | --- |
| `main.rs` | clap：`list` `start` `stop` `status` `logs`。人读文本走 stdout，进度走 stderr |
| `config` | 解析 TOML、查找配置文件、环境变量覆盖、路径展开 |
| `probe` | `Probe` trait。二进制用 `SystemProbe`（`kill -0`、TCP、`curl`）。测试用假实现 |
| `ops` | pid 文件、`StatusJson`、启动、停止等待、`tail` 参数 |

库是以后 UI 的复用点。界面应调用同一套 `resolve_project` 和 `status_of`，而不是再解析一遍 TOML。

## 配置解析顺序

1. 选定文件：`--config`，否则 `TONGYUN_DEV_CONFIG`，否则从 cwd 向上找 `config/projects.toml`。
2. 解析 `[projects.<id>]`。项目按 id 放进有序表，`list` 直接按字典序输出。
3. 应用 `TONGYUN_DEV_<ID>_ROOT` / `_TOMCAT` / `_LOG_DIR`。空值跳过。这里不打印取值。
4. 展开路径。`root`、`tomcat` 只处理开头的 `~`。`log_dir` 先代入 `{{root}}` 和 `{{tomcat}}` 再处理 `~`。`start`、`stop`、`log_file`、`pid_file` 代入三个占位符；若整段参数以 `~` 开头，再展开一次。
5. 占位符只替换一次。写进替换值里的 `{{...}}` 保持字面量，避免循环。

`start` / `stop` 在项目根目录存在时以该目录为 cwd。参数原样交给操作系统，不经过 shell，除非配置自己把程序写成 `bash`。默认的 `td_work` stop 就是 `bash -lc` 加一段已替换好的脚本。替换是字面量，路径里不要放双引号。

## 状态怎么来

`status_of` 不启动也不停止任何东西。

1. 读 pid 文件第一行。`0`、空、非数字 → 没有 pid。
2. `Probe::pid_alive`。系统实现是 `kill -0`，并且拒绝 pid `0`（`kill -0 0` 会打到进程组）。
3. `running` = 第 2 步为真。`pid` 在为假时是 null。
4. `port_open` = 在 400ms 内连上 `127.0.0.1:port`。这只说明有套接字在听，不证明是我们的 pid。
5. `login_probe` = `curl -sS -o /dev/null -w %{http_code}`，连接超时 2 秒，总超时 5 秒。`000` 记为 null。`ok` 仅 200 / 301 / 302。curl 的 stderr 丢弃。

人读输出不是稳定契约。Agent 只解析 `--json`。

## 启停

`start` 执行 `start` argv。非 0 则失败。成功后打一次人读状态，供眼睛看；Agent 要机器可读结果时再跑 `status --json`。`start` 不额外等待登录页，因为 `td_work` 的等待在启动脚本里，那个脚本不在本仓。

`stop`：

1. 执行 `stop` argv。失败时若 pid 已死且端口已关，记一条 stderr 后继续；否则返回该错误（不含参数）。
2. 轮询 pid 与端口，默认最多 20 秒、间隔 500ms。轮询不打登录页，避免每次 curl 把停止拖慢。
3. 两者都落下则成功。超时则非 0，消息里带 `pid_alive`、`port_open`、端口号。

`logs` 在文件不存在时直接失败，不创建文件。`--follow` 使用 `tail -F`。跟随被中断时不把 tail 的非 0 当成失败。

## 安全边界

- 配置和错误文本不包含密钥。调用方若把密钥放进 argv，CLI 也不回显参数；仍不要这么做。
- 探测只访问配置里的登录 URL，默认是 `127.0.0.1`。
- 不读取、不修改 82 安全组，不连接生产主机。

## 测试边界

见 [0002](decisions/0002-ci-without-tomcat.md)。

- 纯函数：TOML、`~`、模板、HTTP 码、JSON 键。
- 临时目录：pid 文件、缺失日志、假探针下的 stop 超时。
- 二进制：`--config` 指向临时 TOML；活 pid 用测试进程自己的 pid；打开的端口用 `TcpListener`；登录 200 用测试里的一次性 HTTP，而不是 Tomcat。
- 不在测试里绑定或请求 8080。

## M2 界面（仍是提案）

Tauri 2 + React，与 tongyun-git 同系。进程模型建议：

- Rust 侧引用本库，向前端提供 `list` 和 `status` 的同一 JSON。
- 日志跟随放在独立任务里，界面只订阅追加的文本。
- 不在 WebView 里拼 shell。启动和停止仍走配置里的 argv。

M2 之前不新增第二份项目注册格式。
