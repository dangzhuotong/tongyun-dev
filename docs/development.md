# 开发与验收

`tongyun-dev` 等于 `cargo run -p tongyun-dev-cli --`，也等于编出来的 `target/debug/tongyun-dev`。

## 环境

- Rust stable（以仓库根的 `rust-toolchain.toml` 为准），含 rustfmt 与 clippy。
- 本机命令：`bash`、`tail`、`curl`、`kill`。登录探测依赖 curl。
- 跑测试和 CI **不需要** Tomcat，也不需要 `td_work` 检出。
- 真机验收才需要 macOS 上的 `td_work` 与 Tomcat。云端 agent 环境没有这两样，不能代替真机清单。

## 常用命令

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets

cargo run -p tongyun-dev-cli -- list
cargo run -p tongyun-dev-cli -- status td_work --json
cargo run -p tongyun-dev-cli -- start td_work
cargo run -p tongyun-dev-cli -- logs td_work --follow
cargo run -p tongyun-dev-cli -- stop td_work
```

配置查找：`--config`，否则 `TONGYUN_DEV_CONFIG`，否则从当前目录向上找 `config/projects.toml`。

路径和默认值不同时，在 shell 里覆盖，不要改提交的 toml，也不要写入密钥：

```bash
export TONGYUN_DEV_TD_WORK_ROOT="$HOME/code/testdaily/tdprojects"
export TONGYUN_DEV_TD_WORK_TOMCAT="$HOME/opt/apache-tomcat-9.0.98"
export TONGYUN_DEV_TD_WORK_LOG_DIR="$HOME/code/_index/logs"
```

空值会被忽略。变量名规则见 [prd.md](prd.md)。

## 测试

| 层 | 位置 | 覆盖 |
| --- | --- | --- |
| 单元 | `crates/tongyun-dev-cli/src/{config,probe,ops}.rs` | TOML、`~`、模板、环境变量、pid 文件、假探针、JSON 键、停止超时、错误不带参数 |
| 集成 | `crates/tongyun-dev-cli/tests/cli.rs` | 二进制 list/status/logs/start/stop；临时目录；一次性本地 HTTP |
| 真机 | 下面的清单 | 真实脚本、8080、登录页、catalina |

`cargo test` 不得绑定或请求 8080，不得执行 `start-td-work.sh`。

## CI

`.github/workflows/ci.yml` 在 `ubuntu-latest` 上跑：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

`RUSTFLAGS=-D warnings`。不安装 Tomcat，不启动 8080 上的服务。

## JSON

`status --json` 与 `list --json` 的键见 [prd.md](prd.md)。解析 JSON，不要按空格切人读的那一行。pretty-print 的缩进不是契约。

## 本机约定（td_work）

- 项目根：`~/code/testdaily/tdprojects`
- 启动：`scripts/start-td-work.sh`（脚本内部会先 stop 再 start）
- 停止：`$TOMCAT_HOME/bin/catalina.sh stop 15 -force`（默认 `~/opt/apache-tomcat-9.0.98`）
- 日志：`~/code/_index/logs/td_work_catalina.out`
- PID：`~/code/_index/logs/td_work_tomcat.pid`
- 端口：8080
- 探针：`http://127.0.0.1:8080/td_work/admin/login.html`

`stop` 最长等 20 秒。到时 pid 仍在或 8080 仍开着，命令以非 0 退出。

## MVP 验收清单（Mac 真机）

在装有 `td_work` 与 Tomcat 的 macOS 上按这个顺序勾。CI 不跑 Tomcat。下文 `tongyun-dev` 即 `cargo run -p tongyun-dev-cli --`。

1. [ ] `config/projects.toml` 里 `td_work` 指向本机 `~/code/testdaily/tdprojects`（路径不同时用 `TONGYUN_DEV_TD_WORK_ROOT` / `_TOMCAT` / `_LOG_DIR` 覆盖，不要把密钥写进仓库）
2. [ ] `tongyun-dev status td_work --json` 退出码 0，JSON 含 `running`、`pid`、`port`、`port_open`、`login_probe`（`url` / `http_code` / `ok`）、`log_path`、`pid_path`、`root`
3. [ ] `tongyun-dev start td_work` 成功返回后，`http://127.0.0.1:8080/td_work/admin/login.html` 为 200，或再次 `status --json` 时 `login_probe.ok` 为 true
4. [ ] `tongyun-dev logs td_work` 能打印 catalina 日志；`tongyun-dev logs td_work --follow` 能跟随新行（Ctrl-C 结束）
5. [ ] `tongyun-dev stop td_work` 成功后，再次 `status td_work --json`：`running` 为 false，`port_open` 为 false（8080 无监听）
6. [ ] 全程终端不打印密钥、Token、数据库密码；仓库内无 secrets

用例编号和自动化对应关系见 [testing/acceptance-tests.md](testing/acceptance-tests.md)。
