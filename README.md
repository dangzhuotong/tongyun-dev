# tongyun-dev

本地开发机控制台：注册项目、启停、看状态、跟日志。对标 IDEA 运行面板，**人用终端，AI 读 JSON 状态和日志**。

- 仓库：私有
- CLI 与桌面壳共用 `tongyun-dev-core`。壳是 Tauri 2 + React，见 [docs/decisions/0001-cli-first.md](docs/decisions/0001-cli-first.md)
- 已注册本机 testdaily 产品线（桌面列表只显示启用项）：`td_work`、新托福、AP、领启课管。私有 Maven 和 `td-job-sdk` 被挡住的两项 `enabled=false`

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

`td_work` 的 `start` 会先跑启动脚本，再等到登录页 `http://127.0.0.1:8080/td_work/admin/login.html` 返回 200、301 或 302。默认最多再等 180 秒，可以用 `TONGYUN_DEV_START_TIMEOUT`（秒）改。`status --json` 稳定包含 `running`、`pid`、`port`、`login_probe`、`log_path`（展开后的绝对路径）。

`list` 打印全部项目。`list --json` 每项仍有 `id`、`name`、`root`，并带 `enabled`、`port`、`reason`。停用项在文本模式多两列：`disabled` 和原因。

| id | 端口 | 启用 |
| --- | --- | --- |
| `td_work` | 8080 | 是 |
| `td_new_toefl` | 8034 | 是 |
| `td_new_toefl_vue` | 5777 | 是 |
| `td_new_toefl_student_vue` | 5999 | 是 |
| `ap_whitepaper` | 8888 | 是 |
| `ap_whitepaper_web` | 7888 | 是 |
| `ap_admin_test_center` | 8110 | 是 |
| `ap_admin_test_center_vue` | 8111 | 是 |
| `testdaily_system_ap_java_backend` | — | 否，私有 Maven 被挡住 |
| `lj_class_admin_manager` | 8147 | 是 |
| `lj_class_admin_web` | 8146 | 是 |
| `ljstudentmanageserver` | — | 否，`td-job-sdk` 被挡住 |

路径和启动命令见 [config/projects.toml](config/projects.toml)，字段说明见 [docs/development.md](docs/development.md)。Spring / pnpm 在后台跑，日志默认在 `~/code/_index/logs/<id>.out`。

验收清单见 [docs/development.md](docs/development.md)。

## 桌面窗口（Mac）

需要 Node.js 20+ 和 Xcode Command Line Tools。窗口调用同一套 Rust core：启用中的项目列表、启动、停止、刷新状态（`running` / `pid` / `port` / `login_probe`），并跟随该项目日志。停用项不显示。

```bash
cd apps/desktop
npm install
npm run tauri dev
```

打包：

```bash
cd apps/desktop
npm run tauri build
```

开发时在仓库里运行，窗口会从当前目录往上找到 `config/projects.toml`。也可以先 `export TONGYUN_DEV_CONFIG=/absolute/path/to/config/projects.toml`。不要把密钥写进仓库。

## 文档

- [AGENTS.md](AGENTS.md) — AI 开发规则
- [docs/prd.md](docs/prd.md) — 产品范围
- [docs/plan.md](docs/plan.md) — 排期
- [docs/development.md](docs/development.md) — 命令与验收
