# tongyun-dev

本地开发机控制台：注册项目、启停、看状态、跟日志。对标 IDEA 运行面板，**人用终端，AI 读 JSON 状态和日志**。

- 仓库：私有
- MVP 只有 **Rust CLI**（`tongyun-dev`）。Tauri 2 + React 桌面壳不在这次范围，见 [docs/decisions/0001-cli-first.md](docs/decisions/0001-cli-first.md)
- 第一个项目：`td_work`

## Mac 快速开始

需要 macOS、Git、Rust 1.85+（用 [rustup](https://rustup.rs) 装 stable，提供 `cargo`）。`bash`、`curl`、`tail`、`lsof`、`ps` 用系统自带的即可。在已经能跑 `java -version` 的终端里执行，CLI 会继承当前环境里的 `PATH` / `JAVA_HOME`。

```bash
git clone git@github.com:dangzhuotong/tongyun-dev.git
cd tongyun-dev
cargo build -p tongyun-dev-cli
cargo test -p tongyun-dev-cli
```

`config/projects.toml` 里 `td_work` 的默认路径（只写路径，不要写密码、Token、数据库口令）：

| 项 | 路径 |
| --- | --- |
| 项目根 | `~/code/testdaily/tdprojects` |
| 启动脚本 | `~/code/testdaily/tdprojects/scripts/start-td-work.sh` |
| 停止 | `~/opt/apache-tomcat-9.0.98/bin/catalina.sh stop 15 -force` |
| 日志 | `~/code/_index/logs/td_work_catalina.out` |
| PID | `~/code/_index/logs/td_work_tomcat.pid` |

```bash
cargo run -p tongyun-dev-cli -- status td_work --json
cargo run -p tongyun-dev-cli -- start td_work
cargo run -p tongyun-dev-cli -- logs td_work
cargo run -p tongyun-dev-cli -- logs td_work --follow
cargo run -p tongyun-dev-cli -- stop td_work
cargo run -p tongyun-dev-cli -- list
```

编译完成后也可以：

```bash
./target/debug/tongyun-dev status td_work --json
./target/debug/tongyun-dev start td_work
./target/debug/tongyun-dev stop td_work
./target/debug/tongyun-dev logs td_work --follow
```

不在仓库根目录时，指定配置（`~` 可用）：

```bash
export TONGYUN_DEV_CONFIG="$PWD/config/projects.toml"
./target/debug/tongyun-dev --config ~/path/to/tongyun-dev/config/projects.toml status td_work --json
```

`start` 会先跑启动脚本，再等到登录页 `http://127.0.0.1:8080/td_work/admin/login.html` 返回 200、301 或 302。默认最多再等 180 秒，可以用 `TONGYUN_DEV_START_TIMEOUT`（秒）改。`status --json` 稳定包含 `running`、`pid`、`port`、`login_probe`、`log_path`（展开后的绝对路径）。

验收清单见 [docs/development.md](docs/development.md)。

## 文档

- [AGENTS.md](AGENTS.md) — AI 开发规则
- [docs/prd.md](docs/prd.md) — 产品范围
- [docs/plan.md](docs/plan.md) — 排期
- [docs/development.md](docs/development.md) — 命令与验收
