# 开发与验收

## 命令

在仓库根目录（Rust 1.85+，`rustup` 的 stable）：

```bash
cargo build -p tongyun-dev-cli
cargo test -p tongyun-dev-cli
cargo run -p tongyun-dev-cli -- status td_work --json
cargo run -p tongyun-dev-cli -- start td_work
cargo run -p tongyun-dev-cli -- stop td_work
cargo run -p tongyun-dev-cli -- logs td_work --follow
```

二进制名是 `tongyun-dev`（`./target/debug/tongyun-dev`）。逻辑在 `tongyun-dev-core`，CLI 和桌面壳共用。

```bash
cargo test -p tongyun-dev-core
```

## 桌面壳

需要 Node.js 20+。Mac 上还要 Xcode Command Line Tools。在 `apps/desktop`：

```bash
cd apps/desktop
npm install
npm run tauri dev
npm run tauri build
```

`tauri dev` 打开窗口：左侧项目列表（含 `td_work`），按钮为启动 / 停止 / 刷新状态，状态区显示 `running`、`pid`、`port`、`login_probe`。下方日志区每 1.5 秒重读 catalina 日志尾部（与 CLI 同一条 log 路径）。配置查找与 CLI 相同：`TONGYUN_DEV_CONFIG`，否则从当前目录往上找 `config/projects.toml`。

## 行为（td_work）

- `~`、`$HOME`、`${HOME}` 会展开成绝对路径；`{{root}}`、`{{tomcat}}`、`{{log_dir}}` 用展开后的值替换。展开失败（没有 HOME）直接报错，不会把路径写成 `/code/...`。
- `start` 在项目根目录执行 `scripts/start-td-work.sh`，继承当前终端的环境，并设置 `CATALINA_HOME`、`CATALINA_BASE`、`CATALINA_PID`。不用 `bash -l`。脚本返回后最多再等 180 秒（`TONGYUN_DEV_START_TIMEOUT`），直到登录页为 200、301 或 302。
- `stop` 执行 `catalina.sh stop 15 -force`。已经停掉则成功退出。若 20 秒后端口或 Tomcat 仍在，只对 Java/Tomcat 进程发 TERM，再不行发 KILL。非 Java 占用 8080 时不杀，命令失败并写出 pid 与进程名。
- `status --json` 的 `running` 表示 pid 文件或监听端口上有存活的 Java/Tomcat。`log_path` 是绝对路径。端口没开时不做 HTTP 请求，`login_probe.http_code` 为 `null`。
- `logs` 打印 catalina 日志；`--follow` 在文件还没生成时会等待，再用 `tail -F` 跟随。

## MVP 验收清单（给运维/产品）

在装有 td_work 的本机（默认 macOS）：

1. [ ] `config/projects.toml` 里 `td_work` 路径指向本机 `~/code/testdaily/tdprojects`
2. [ ] `tongyun-dev status td_work --json` 返回 JSON，含 `running`、`pid`、`port`、`login_probe`、`log_path`
3. [ ] `tongyun-dev stop td_work` 后 status.running=false，8080 无监听
4. [ ] `tongyun-dev start td_work` 后登录页 `http://127.0.0.1:8080/td_work/admin/login.html` 返回 200（301 / 302 也算探针成功）
5. [ ] `tongyun-dev logs td_work` 能打印 catalina 日志；`--follow` 能跟随
6. [ ] 全程终端不打印密钥；仓内无 secrets

## 本机约定（td_work）

- 项目根：`~/code/testdaily/tdprojects`
- 启动：`scripts/start-td-work.sh`（内含先 stop 再 start）
- 停止：`$TOMCAT_HOME/bin/catalina.sh stop 15 -force`（默认 `~/opt/apache-tomcat-9.0.98`）
- 日志：`~/code/_index/logs/td_work_catalina.out`
- PID：`~/code/_index/logs/td_work_tomcat.pid`
- 端口：8080；探针：`/td_work/admin/login.html`
