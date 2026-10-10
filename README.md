# tongyun-dev

本地开发机控制台：注册项目、启停、看状态、跟日志。对标 IDEA 运行面板，**人用终端，AI 读 JSON 状态和日志**。

- 仓库：私有
- CLI、网页控制台（:9099）与桌面壳共用 `tongyun-dev-core`。桌面壳是 Tauri 2 + React，见 [docs/decisions/0001-cli-first.md](docs/decisions/0001-cli-first.md)
- 已注册本机 acme 产品线：`legacy_app`、示例考试、AP、示例课管。私有 Maven 和 `acme-job-sdk` 被挡住的两项在桌面列表里变灰，并带上原因。

## 三个入口，一个 core

所有启停 / 状态 / 日志 / 注册逻辑都在 `crates/tongyun-dev-core`，三种入口调同一套代码、读同一份 `config/projects.toml`：

| 入口 | 怎么开 | 适合 |
| --- | --- | --- |
| 网页控制台 | `cargo run -p tongyun-dev-cli`（无参数）或 `-- ui`，默认 `http://127.0.0.1:9099` | 浏览器里点，支持注册/移除项目 |
| 桌面窗口 | `apps/desktop`（Tauri 2），见下文「桌面窗口（Mac）」 | 常驻窗口，跟日志 |
| CLI | `tongyun-dev <命令>`，`--json` 给 Agent | 终端、脚本、AI |

### 网页控制台（:9099）

```bash
cargo run -p tongyun-dev-cli                      # 无参数 = ui，自动打开浏览器
cargo run -p tongyun-dev-cli -- ui --port 9100 --no-open
bash scripts/run.sh                                # 同上，包了一层 cargo run
```

只监听 `127.0.0.1`；9099 被占时自动往后找（最多 +20）。卡片每 3 秒刷新：运行状态、pid、端口、探针、根目录；停用项变灰并显示原因、启动按钮不可点；端口被别的进程占用时显示占用者；声明了 `node` / `java` 的项目显示版本。按钮：启动 / 停止（进行中显示「启动中…」并锁定）、打开页面、打开目录、看日志、移除注册。「＋ 注册项目」可填 Node / Java 版本。每个请求都重新读配置，所以和 CLI、桌面窗口的改动互相可见。只接受本机 Host 和同源请求，别的网页不能借浏览器调它。

## Mac 快速开始

需要 macOS、Git、Rust 1.85+（用 [rustup](https://rustup.rs) 装 stable，提供 `cargo`）。`bash`、`curl`、`tail`、`lsof`、`ps` 用系统自带的即可。在已经能跑 `java -version` 的终端里执行，CLI 会继承当前环境里的 `PATH` / `JAVA_HOME`。

```bash
git clone git@github.com:dangzhuotong/tongyun-dev.git
cd tongyun-dev
cargo build -p tongyun-dev-cli
cargo test -p tongyun-dev-core -p tongyun-dev-cli
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

`legacy_app` 的 `start` 会先跑启动脚本，再等到登录页 `http://127.0.0.1:8080/legacy_app/admin/login.html` 返回 200、301 或 302。默认最多再等 180 秒，可以用 `TONGYUN_DEV_START_TIMEOUT`（秒）改。`status --json` 稳定包含 `running`、`pid`、`port`、`login_probe`、`log_path`（展开后的绝对路径）。

其他命令：

```bash
tongyun-dev status                    # 不带 id：全部项目
tongyun-dev start legacy_app --open      # 就绪后用浏览器打开登录页
tongyun-dev open legacy_app              # 打开页面（login_url / health_url / http://127.0.0.1:<port>/）
tongyun-dev open legacy_app --dir        # 打开项目目录
tongyun-dev add my_web --root ~/code/my_web --start "pnpm dev" --port 5173 --node 22
tongyun-dev add my_api --root ~/code/my_api --start "mvn spring-boot:run" --port 8090 --java 17.0.10-tem
tongyun-dev remove my_web             # 只删注册，不动项目文件
```

`add` 写入 `config/projects.toml` 时保留原有注释和其他项目的写法；id 已存在会拒绝（先 `remove`）。新加的项目默认后台运行（`background = true`，适合 `pnpm dev` / `mvn spring-boot:run` 这类不退出的命令；只有会自己退出的脚本才加 `--foreground`）。启动命令存成 `["bash", "-lc", "<命令>"]`；声明了 `--node` / `--java` 时存成 `["bash", "-c", ...]`，避免 login shell 重新初始化 fnm / sdkman 把版本切回去。`--login-url` 可选：填了就要求 HTTP 200/301/302 才算启动完成，不填则端口能连上即可。

`list` 打印全部项目。`list --json` 每项仍有 `id`、`name`、`root`，并带 `enabled`、`port`、`reason`。停用项在文本模式多两列：`disabled` 和原因。

| id | 端口 | 启用 |
| --- | --- | --- |
| `legacy_app` | 8080 | 是 |
| `demo_exam` | 8034 | 是 |
| `demo_exam_vue` | 5777 | 是 |
| `demo_exam_student_vue` | 5999 | 是 |
| `ap_docs-site` | 8888 | 是 |
| `ap_docs-site_web` | 7888 | 是 |
| `ap_admin_test_center` | 8110 | 是 |
| `ops_class_admin_manager` | 8147 | 是 |
| `ops_class_admin_web` | 8146 | 是 |
| `ap_admin_test_center_vue` | — | 否，本机没有独立目录 |
| `acme_system_ap_java_backend` | — | 否，私有 Maven 被挡住 |
| `opsstudentmanageserver` | — | 否，`acme-job-sdk` 被挡住 |

`04-papers`、`05-wechat`、`06-admin-misc` 里其余仓库也已登记，目前停用，原因是启动命令还没确认。

路径和启动命令见 [config/projects.toml](config/projects.toml)，字段说明见 [docs/development.md](docs/development.md)。Spring / pnpm 在后台跑，日志默认在 `~/.tongyun-dev/logs/<id>.out`。

### 工具链版本（node / java）

在 `config/projects.toml` 中，每个项目可声明可选的 `node` 与 `java` 字段：

| 字段 | 示例 | 行为说明 |
| --- | --- | --- |
| `node` | `"20"` | CLI start 时在启动命令前加 `fnm exec --using=<v> --` 切换；未安装 `fnm` 时给出可读报错 |
| `java` | `"17"` | CLI start 时经 `bash -lc` 运行 `sdk use java <v>` 切换；未安装 `sdkman` 时给出可读报错 |

`add` 用 `--node` / `--java`（网页控制台表单同样有这两栏）写入这两个字段。桌面 App 和网页控制台的项目列表会展示声明的版本（如 `Node 20`、`Java 17`；未声明则不显示）。

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

桌面壳需要 Rust 1.88+（Tauri 依赖要求），CLI / core 1.85 即可。

开发时在仓库里运行，窗口会从当前目录往上找到 `config/projects.toml`。也可以先 `export TONGYUN_DEV_CONFIG=/absolute/path/to/config/projects.toml`。不要把密钥写进仓库。

## 平台

目前只支持 macOS / Linux：core 用 `kill`、`ps`、`lsof`、`tail` 管进程。`scripts/run.ps1` 保留着，但 Windows 上启停和状态还不能用。

## 文档

- [AGENTS.md](AGENTS.md) — AI 开发规则
- [docs/prd.md](docs/prd.md) — 产品范围
- [docs/plan.md](docs/plan.md) — 排期
- [docs/development.md](docs/development.md) — 命令与验收
