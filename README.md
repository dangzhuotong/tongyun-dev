# tongyun-dev

本地开发机控制台：注册项目、启停、看状态、跟日志。对标 IDEA 运行面板，**人用终端，AI 读 JSON 状态和日志**。

- 仓库：私有
- CLI 与桌面壳共用 `tongyun-dev-core`。壳是 Tauri 2 + React，见 [docs/decisions/0001-cli-first.md](docs/decisions/0001-cli-first.md)
- 已注册本机 testdaily 产品线：`td_work`、新托福、AP、领启课管。私有 Maven 和 `td-job-sdk` 被挡住的两项在桌面列表里变灰，并带上原因。

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
| `lj_class_admin_manager` | 8147 | 是 |
| `lj_class_admin_web` | 8146 | 是 |
| `ap_admin_test_center_vue` | — | 否，本机没有独立目录 |
| `testdaily_system_ap_java_backend` | — | 否，私有 Maven 被挡住 |
| `ljstudentmanageserver` | — | 否，`td-job-sdk` 被挡住 |

`04-papers`、`05-wechat`、`06-admin-misc` 里其余仓库也已登记，目前停用，原因是启动命令还没确认。

路径和启动命令见 [config/projects.toml](config/projects.toml)，字段说明见 [docs/development.md](docs/development.md)。Spring / pnpm 在后台跑，日志默认在 `~/code/_index/logs/<id>.out`。

### 工具链版本（node / java）

在 `config/projects.toml` 中，每个项目可声明可选的 `node` 与 `java` 字段：

| 字段 | 示例 | 行为说明 |
| --- | --- | --- |
| `node` | `"20"` | CLI start 时在启动命令前加 `fnm exec --using=<v> --` 切换；未安装 `fnm` 时给出可读报错 |
| `java` | `"17"` | CLI start 时经 `bash -lc` 运行 `sdk use java <v>` 切换；未安装 `sdkman` 时给出可读报错 |

桌面 App 的项目列表会在各条目中展示声明的版本（如 `Node 20`、`Java 17`；未声明则不显示）。

验收清单见 [docs/development.md](docs/development.md)。

## 桌面窗口（Mac）

需要 Node.js 20+ 和 Xcode Command Line Tools。窗口调用同一套 Rust core：项目列表（停用项变灰并显示原因）、启动、停止、刷新状态（`running` / `pid` / `port` / `login_probe`），并跟随该项目日志。启动和停止结束前按钮保持加载状态。端口被其他进程占用时会警告。可以一键打开日志文件和登录页（或 health URL）。

开发调试：

```bash
cd apps/desktop
npm install
npm run tauri dev
```

打包并安装 `.app`（在 `apps/desktop` 目录）：

```bash
npm install
npm run tauri build
```

`npm run tauri build` 会先编前端，再编 release。产物：

| 产物 | 路径 |
| --- | --- |
| 应用 | `apps/desktop/src-tauri/target/release/bundle/macos/tongyun-dev.app` |
| 磁盘镜像 | `apps/desktop/src-tauri/target/release/bundle/dmg/` 里的 `.dmg`（文件名带版本和架构） |

装到「应用程序」后打开：

```bash
cp -R apps/desktop/src-tauri/target/release/bundle/macos/tongyun-dev.app /Applications/
open /Applications/tongyun-dev.app
```

也可以打开 `.dmg`，把 `tongyun-dev` 拖进「应用程序」。若 Gatekeeper 拦住：系统设置 → 隐私与安全性 → 仍要打开。

开发时在仓库里运行，窗口会从当前目录往上找到 `config/projects.toml`。也可以先 `export TONGYUN_DEV_CONFIG=/absolute/path/to/config/projects.toml`。不要把密钥写进仓库。

## 文档

- [AGENTS.md](AGENTS.md) — AI 开发规则
- [docs/prd.md](docs/prd.md) — 产品范围
- [docs/plan.md](docs/plan.md) — 排期
- [docs/development.md](docs/development.md) — 命令与验收
