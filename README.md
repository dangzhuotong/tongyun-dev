# tongyun-dev

本地开发机控制台：把电脑上的各个开发项目（Spring Boot、Tomcat WAR、Vite / Vue / React、Next、Nuxt、Python……）登记到一处，一键启停、看状态、跟日志。对标 IDEA 的运行面板，**人用桌面 App 或网页，AI 读 JSON 状态和日志**。

- CLI、网页控制台（:9099）与桌面 App（Tauri 2 + React）共用同一个 Rust core：`crates/tongyun-dev-core`
- 项目不用手写：**扫描一个目录**，自动认出里面的项目、端口、启动命令和 Java / Node 版本
- 你的项目清单只存在本机 `~/.config/tongyun-dev/projects.toml`，不进仓库

## 三个入口，一个 core

| 入口 | 怎么开 | 适合 |
| --- | --- | --- |
| 桌面 App | `apps/desktop`（Tauri 2），见下文「桌面 App」 | 常驻窗口，跟日志 |
| 网页控制台 | `tongyun-dev`（无参数）或 `tongyun-dev ui`，默认 `http://127.0.0.1:9099` | 浏览器里点 |
| CLI | `tongyun-dev <命令>`，`--json` 给 Agent | 终端、脚本、AI |

## 快速开始

需要 Git 和 Rust 1.85+（用 [rustup](https://rustup.rs) 装 stable）。macOS / Linux 自带的 `bash`、`curl`、`tail`、`lsof`、`ps` 即可。

```bash
git clone <this repo> tongyun-dev && cd tongyun-dev
cargo build -p tongyun-dev-cli
./target/debug/tongyun-dev scan ~/code          # 先看看能找到哪些项目（只读）
./target/debug/tongyun-dev scan ~/code --apply  # 写进 ~/.config/tongyun-dev/projects.toml
./target/debug/tongyun-dev                      # 打开网页控制台
```

第一次运行还没有配置时，`scan --apply`、桌面 App 的「扫描一个目录」都会先在 `~/.config/tongyun-dev/projects.toml` 新建一份空配置。

## 扫描目录

`tongyun-dev scan <目录> [--depth N] [--apply] [--json]`，桌面 App 和网页控制台里是「扫描目录…」按钮（App 可以用系统对话框选目录）。

- 默认往下看 3 层（`--depth`，最多 8）；跳过 `node_modules`、`.git` 等隐藏目录、`target`、`dist`、`build`、`vendor` 等。
- 每个结果带类型、把握（high / medium / low）、建议的 id / 名称 / 启动命令 / 端口 / Java / Node 版本，以及「为什么这么猜」。
- 已经登记过的目录（按 root 判断）标成「已注册」，不会重复添加，也不会改动已有项目；建议的 id 不会和已有 id 冲突。
- 写配置用 toml_edit，原有注释和格式都保留。

| 类型 | 怎么认 | 启动命令 | 端口 / 版本从哪来 |
| --- | --- | --- | --- |
| Spring Boot（Maven） | `pom.xml` 里有 spring-boot | `mvn spring-boot:run`（有 `mvnw` 用 `./mvnw`） | `src/main/resources/application*.yml/properties` 的 `server.port`，默认 8080；Java 来自 `.sdkmanrc` / `.java-version` / `java.version` / `maven.compiler.*` |
| Spring Boot（Gradle） | `build.gradle(.kts)` 用了 `org.springframework.boot` | `./gradlew bootRun` | 同上；Java 来自 toolchain / `sourceCompatibility` |
| Tomcat WAR | `pom.xml` `<packaging>war</packaging>` | `mvn package` → 拷 war 到 `{{tomcat}}/webapps` → `catalina.sh run` | 默认 8080；需要填 Tomcat 目录，不填就先加成停用 |
| Vite（Vue / React） | 依赖里有 `vite` | `pnpm dev` / `yarn dev` / `npm run dev`（按锁文件） | 脚本里的 `--port` / `-p`，或 `vite.config.*` 的 `server.port`，默认 5173；Node 来自 `.nvmrc` / `.node-version` / `engines.node` |
| Vue CLI | `@vue/cli-service` | `… serve` | 脚本或 `vue.config.js` 的 `devServer.port`，默认 8080 |
| Next.js / Nuxt | `next` / `nuxt` | `… dev` | `-p` / `--port` 或 `nuxt.config`，默认 3000 |
| React（CRA）、普通 Node | `react-scripts` / 只有 `start` 脚本 | `npm start` 等 | 脚本里的端口；找不到要手动填 |
| Django / FastAPI / Flask | `manage.py`、依赖里的 fastapi / uvicorn / flask | `runserver` / `uvicorn` / `flask run`（有 `.venv` 就用它） | 8000 / 8000 / 5000 |

Maven 父工程（有 `<modules>`）、npm workspaces 根、Gradle 多项目根只当容器，继续往下找子项目。

## CLI

```bash
tongyun-dev list [--json]
tongyun-dev status [id] [--json]       # 不带 id：全部项目
tongyun-dev start <id> [--open]        # 等到端口通 / 探针成功；--open 就绪后打开页面
tongyun-dev stop <id>
tongyun-dev logs <id> [--follow] [--lines N]
tongyun-dev open <id> [--dir]          # 打开页面（login_url / health_url / http://127.0.0.1:<port>/）或目录
tongyun-dev install <id>               # 装前端依赖（按锁文件选 pnpm / yarn / npm）
tongyun-dev scan <dir> [--depth N] [--apply] [--json]
tongyun-dev add my_web --root ~/code/my-web --start "pnpm dev" --port 5173 --node 20
tongyun-dev add my_api --root ~/code/my-api --start "mvn spring-boot:run" --port 8081 --java 17
tongyun-dev remove my_web              # 只删登记，不动项目文件
tongyun-dev ui [--port 9099] [--no-open]
```

`status --json` 稳定包含 `running`、`state`（`stopped` / `installing` / `starting` / `port_open` / `ready` / `failed`）、`elapsed_secs`、`failure`、`pid`、`port`、`login_probe`、`log_path`、`port_conflict`、`port_holder`。

配置查找顺序：1) `--config`；2) 环境变量 `TONGYUN_DEV_CONFIG`；3) `~/.config/tongyun-dev/settings.json` 记住的路径（App 里「换一个配置文件」）；4) `~/.config/tongyun-dev/projects.toml`；5) 从当前目录往上找 `config/projects.toml`（只给开发调试用，已被 `.gitignore`）。

配置格式见 [config/projects.example.toml](config/projects.example.toml)（虚构示例）和 [docs/development.md](docs/development.md)。

**前端依赖**：根目录有 `package.json` 但没有 `node_modules` 时，`start` 会先按锁文件装依赖（状态「安装依赖中」），装好接着启动。不想自动装就写 `auto_install = false`。

**端口参数**：pnpm 直接带参数（`pnpm dev --port 5173`）；不要写 `pnpm dev -- --port`，旧配置里这样写的启动时会自动去掉 `--`。npm 才需要 `npm run dev -- --port 5173`。

## Java / Node 版本

项目里写 `java = "17"` / `node = "20"`，启动前由 core 直接找到对应 JDK / Node，注入 `JAVA_HOME` 和 `PATH`，不 source sdkman / fnm 的初始化脚本：

- Java：`java_home` 显式路径优先；macOS / Linux 找 `~/.sdkman/candidates/java/<版本>*`（`1.8` 视同 `8`）和 `/Library/Java/JavaVirtualMachines`；Windows 找 `Program Files` 下的 Zulu / Eclipse Adoptium / Java。
- Node：`node_home` 优先；macOS / Linux 找 fnm（`FNM_DIR`、`~/.local/share/fnm`、`~/Library/Application Support/fnm`）和 nvm；Windows 找 fnm 和 nvm-windows。
- 找不到就直接提示「缺 Java 8」这类原因，不会拿错的版本硬跑。
- GUI App 的 PATH 很短，core 会补上常见目录（`/opt/homebrew/bin`、`/usr/local/bin`、sdkman 的 maven / gradle、`~/.local/bin` 等）。
- 项目自带的启动脚本不要 `source sdkman-init.sh`（在 `set -u` 下会报 `SDKMAN_CANDIDATES_API: unbound variable`），直接调用 `mvn` / `java` 即可。

## 日志与 PID

- 每个项目默认把输出写到 `~/.tongyun-dev/logs/<id>.out`，PID 记在 `~/.tongyun-dev/logs/<id>.pid`。
- 想换位置：在配置文件顶层写 `log_dir = "~/somewhere"`，所有没单独设置的项目都用它。某个项目可以再写 `log_dir = "~/somewhere"`、`log_dir = "{{root}}/logs"`，或相对项目根的 `log_dir = "logs"`（相对路径不能含 `..`，`/` 和 `\` 都算分隔符）来覆盖顶层。桌面「项目设置」、网页控制台和 `tongyun-dev add --log-dir` 写的是项目自己的值。也可以单独指定 `log_file` / `pid_file`，里面能用 `{{log_dir}}`、`{{root}}`。
- 看日志：`tongyun-dev logs <id> [--follow]`，或者桌面 App / 网页控制台的日志面板。

**从旧版升级**：默认日志目录已经改成 `~/.tongyun-dev/logs`。升级后第一次打开，App 可能认不出用旧位置启动、还在跑的进程（显示为未运行）。手动停掉旧进程再从 App 启动即可；想沿用旧目录，就在对应项目里设 `log_dir`。

## 桌面 App

需要 Node.js 20+；macOS 还要 Xcode Command Line Tools；桌面壳需要 Rust 1.88+（Tauri 依赖要求）。

```bash
cd apps/desktop
npm install
npm run tauri dev      # 开发
npm run tauri build    # 打包
```

产物在仓库根目录的 `target/release/bundle/`（macOS 通用包在 `target/universal-apple-darwin/release/bundle/`；`.app` 名为 `tongyun-dev.app`）。安装包没有 Apple 签名，第一次打开如被拦：系统设置 → 隐私与安全性 → 仍要打开，或执行 `xattr -dr com.apple.quarantine /Applications/tongyun-dev.app`。

### 检查更新

日常更新用桌面 App 里的 **检查更新**（顶栏或侧栏的设置，以及关于窗口），或等启动大约 3 秒后的提示。已经是最新会提示「已是最新版本」。不要找人在本机用开发 tip 重装。

新版本来自 tag `vX.Y.Z`，并且和 `apps/desktop/package.json`、`apps/desktop/src-tauri/tauri.conf.json`、仓库 `Cargo.toml` 的版本一致。GitHub Actions 会发布 Release，并带上更新器用的 `latest.json`。合进 main 不会自动发版。步骤见 [docs/RELEASE.md](docs/RELEASE.md)。

安装包在 CI 里用 **minisign** 签过更新包（`TAURI_SIGNING_*`）。**尚未做 Apple 公证**：首次安装仍可能要按上面的办法解除隔离。已经装上的 App 用检查更新换包。发版流水线挂了，或用户本机环境坏了，才需要人工介入。

## 平台

工具链解析和环境注入支持 macOS、Linux、Windows。进程停止与状态查询目前只支持 Unix（依赖 `kill`、`ps`、`lsof`、`tail`）。

## 文档

- [AGENTS.md](AGENTS.md) — AI 开发规则
- [.agents/skills/tongyun-dev/SKILL.md](.agents/skills/tongyun-dev/SKILL.md) — Agent 如何用 CLI / :9099 驱动本工具
- [docs/prd.md](docs/prd.md) — 产品范围
- [docs/plan.md](docs/plan.md) — 排期
- [docs/development.md](docs/development.md) — 命令、配置字段与验收
- [docs/RELEASE.md](docs/RELEASE.md) — 桌面 App 发版与检查更新

## License

MIT

## 重启与网页项目设置

- `tongyun-dev restart <id> [--open]`：停止成功后重新启动并等待就绪。桌面和网页的重启使用相同 core；停止未成功时返回错误，不继续启动。
- 网页卡片的「设置」支持名称、目录、命令、端口、探针、日志、Java/Node 版本、启用/停用和超时。数组形式的复杂命令保持只读，避免保存时改变参数含义。清空版本或探针会移除该配置字段；原有注释保留。
- HTTP：`GET /api/projects/<id>` 读取详情，`PATCH /api/projects/<id>` 提交部分更新，`POST /api/projects/<id>/restart` 重启。修改接口沿用 Host / Origin 检查。
- 重复启动不会替换仍存活的已追踪进程；停止未成功时保留 PID 与启动标记，便于重试和排查。

自动检查见 `.github/workflows/check.yml`：macOS / Linux 执行 Rust 测试；macOS 执行桌面 typecheck、测试、前端构建和 Tauri Rust 检查。它不代替图形界面真机验收。

## AI 调用与人工接管

AI 用 CLI / JSON 操作本机服务，人用桌面 / 网页查看状态和接管；需求判断、代码分析、测试选择交给 AI。

```bash
tongyun-dev start demo_api --timeout 60 --json
tongyun-dev wait demo_api --timeout 30 --json
tongyun-dev logs demo_api --lines 80 --json
tongyun-dev stop demo_api --json
```

启停 / 重启 / 安装 / 等待 / 日志的 JSON 返回 `{ok, action, id, status, logs, error}`，操作失败退出码为 1；`status --json` 与 `list --json` 保持原有结构。等待超时后已启动的服务仍可查询和停止，自动依赖安装则会取消；`wait` 本身不会启停服务。完整协议和超时范围见 [开发文档](docs/development.md#ai-操作协议与等待上限)。
