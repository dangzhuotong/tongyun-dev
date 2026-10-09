# tongyun-dev

本地开发机控制台：注册项目、启停、看状态、跟日志。对标 IDEA 运行面板，**人用终端，AI 读 JSON 状态和日志**。

- 仓库：私有
- CLI 与桌面壳共用 `tongyun-dev-core`。壳是 Tauri 2 + React，见 [docs/decisions/0001-cli-first.md](docs/decisions/0001-cli-first.md)
- 第一个项目：`legacy_app`

## Mac 快速开始

需要 macOS、Git、Rust 1.85+（用 [rustup](https://rustup.rs) 装 stable，提供 `cargo`）。`bash`、`curl`、`tail`、`lsof`、`ps` 用系统自带的即可。在已经能跑 `java -version` 的终端里执行，CLI 会继承当前环境里的 `PATH` / `JAVA_HOME`。

```bash
git clone git@github.com:dangzhuotong/tongyun-dev.git
cd tongyun-dev
cargo build -p tongyun-dev-cli
cargo test -p tongyun-dev-cli
```

`config/projects.toml` 里 `legacy_app` 的默认路径（只写路径，不要写密码、Token、数据库口令）：

| 项 | 路径 |
| --- | --- |
| 项目根 | `~/code/acme/legacy-app` |
| 启动脚本 | `~/code/acme/legacy-app/scripts/start-legacy.sh` |
| 停止 | `~/opt/tomcat-9/bin/catalina.sh stop 15 -force` |
| 日志 | `~/.tongyun-dev/logs/legacy_app_catalina.out` |
| PID | `~/.tongyun-dev/logs/legacy_app_tomcat.pid` |

```bash
cargo run -p tongyun-dev-cli -- status legacy_app --json
cargo run -p tongyun-dev-cli -- start legacy_app
cargo run -p tongyun-dev-cli -- logs legacy_app
cargo run -p tongyun-dev-cli -- logs legacy_app --follow
cargo run -p tongyun-dev-cli -- stop legacy_app
cargo run -p tongyun-dev-cli -- list
```

编译完成后也可以：

```bash
./target/debug/tongyun-dev status legacy_app --json
./target/debug/tongyun-dev start legacy_app
./target/debug/tongyun-dev stop legacy_app
./target/debug/tongyun-dev logs legacy_app --follow
```

不在仓库根目录时，指定配置（`~` 可用）：

```bash
export TONGYUN_DEV_CONFIG="$PWD/config/projects.toml"
./target/debug/tongyun-dev --config ~/path/to/tongyun-dev/config/projects.toml status legacy_app --json
```

`start` 会先跑启动脚本，再等到登录页 `http://127.0.0.1:8080/legacy_app/admin/login.html` 返回 200、301 或 302。默认最多再等 180 秒，可以用 `TONGYUN_DEV_START_TIMEOUT`（秒）改。`status --json` 稳定包含 `running`、`pid`、`port`、`login_probe`、`log_path`（展开后的绝对路径）。

验收清单见 [docs/development.md](docs/development.md)。

## 桌面窗口（Mac）

需要 Node.js 20+ 和 Xcode Command Line Tools。窗口调用同一套 Rust core：项目列表、启动、停止、刷新状态（`running` / `pid` / `port` / `login_probe`），并跟随 catalina 日志。

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
