# 开发与验收

## 命令

在仓库根目录（Rust 1.85+，`rustup` 的 stable）：

```bash
cargo build -p tongyun-dev-cli
cargo test -p tongyun-dev-cli

# 启动网页控制台并自动打开浏览器（无参数默认行为）
cargo run -p tongyun-dev-cli
cargo run -p tongyun-dev-cli -- ui

# 扫描目录
cargo run -p tongyun-dev-cli -- scan ~/code
cargo run -p tongyun-dev-cli -- scan ~/code --depth 3 --apply

# CLI 与 Agent 命令（示例使用 demo_api / demo_web / demo_legacy）
cargo run -p tongyun-dev-cli -- list --json
cargo run -p tongyun-dev-cli -- status demo_api --json
cargo run -p tongyun-dev-cli -- start demo_api --open
cargo run -p tongyun-dev-cli -- open demo_api
cargo run -p tongyun-dev-cli -- stop demo_api
cargo run -p tongyun-dev-cli -- logs demo_api --follow
cargo run -p tongyun-dev-cli -- add demo_web --name "Demo Web" --root "~/code/demo/demo-web" --start "pnpm dev" --port 5173 --node 20
cargo run -p tongyun-dev-cli -- remove demo_web
```

二进制名是 `tongyun-dev`（`./target/debug/tongyun-dev`）。逻辑在 `tongyun-dev-core`，CLI、网页控制台（`crates/tongyun-dev-cli/src/server.rs`，只做 HTTP 路由和页面）和桌面壳共用。`add` / `remove` 也在 core 里（`Loaded::add_project` / `remove_project`，用 toml_edit 保留注释）。

常用 CLI 命令格式：
- `tongyun-dev scan <dir> [--depth N] [--apply] [--json]`：扫描目录，识别项目并可选写入配置
- `tongyun-dev list [--json]`：列出所有已登记项目
- `tongyun-dev status [id] [--json]`：查看项目状态（不带 id 为全部项目）
- `tongyun-dev start <id> [--open]`：启动项目并等待端口连通/探针就绪；`--open` 就绪后用浏览器打开
- `tongyun-dev stop <id>`：停止项目
- `tongyun-dev logs <id> [--follow] [--lines N]`：查看或跟随日志输出
- `tongyun-dev open <id> [--dir]`：用浏览器打开项目页面，或 `--dir` 打开项目根目录
- `tongyun-dev install <id>`：手动触发依赖安装（按锁文件选用 pnpm / yarn / npm）
- `tongyun-dev add <id> ...` / `tongyun-dev remove <id>`：添加或移除项目登记
- `tongyun-dev ui [--port 9099] [--no-open]`：启动网页控制台

```bash
cargo test -p tongyun-dev-core
cargo test -p tongyun-dev-cli   # 含网页控制台的 HTTP 集成测试
```

## 桌面壳

需要 Node.js 20+。Mac 上还要 Xcode Command Line Tools。桌面壳依赖 Rust 1.88+。在 `apps/desktop`：

```bash
cd apps/desktop
npm install
npm run typecheck
npm run tauri dev
npm run tauri build
```

`tauri dev` 打开窗口：左侧列出全部项目。`enabled = false` 的项变灰，并显示 `reason`，启动按钮不可用。启动 / 停止进行中按钮保持「启动中…」「停止中…」，列表锁定，直到命令结束。状态区显示 `running`、`state`、`pid`、`port`、`login_probe`。端口被别的进程或其他项目占用时出现警告（`port_conflict` / `port_holder`）。「打开页面」用系统浏览器打开 `login_url` 或 `health_url`（都没有时打开 `http://127.0.0.1:<port>/`）；「打开目录」在文件管理器中打开项目根路径。「打开日志」用系统默认程序打开日志文件。下方日志区每 1.5 秒重读该项目的日志尾部。

配置查找顺序与 CLI 相同：1) 显式 `--config` 参数 → 2) 环境变量 `TONGYUN_DEV_CONFIG` → 3) `~/.config/tongyun-dev/settings.json` 记住的路径 → 4) 默认配置 `~/.config/tongyun-dev/projects.toml` → 5) 从当前目录及上级目录找 `config/projects.toml`。未找到配置时桌面端主区域会展示空状态，支持「扫描一个目录 / 手动新增」，或选择已有文件 / 一键创建默认配置。

Mac 上 `npm run tauri build` 的应用在 `apps/desktop/src-tauri/target/release/bundle/macos/tongyun-dev.app`。安装步骤见 [README](../README.md)。

## 扫描目录

`tongyun-dev scan <dir> [--depth N] [--apply] [--json]` 用于自动发现本地开发项目。桌面 App 和网页控制台均提供「扫描目录…」按钮（桌面端支持系统对话框选目录）；初次未配置时的空状态也支持「扫描一个目录 / 手动新增」。

- **扫描遍历规则**：默认向下遍历 3 层（`--depth`，最大支持 8 层）。跳过以 `.` 开头的隐藏目录（如 `.git`）以及依赖/产物目录（`node_modules`、`target`、`dist`、`build`、`vendor`、`__pycache__`、`venv`、`logs` 等）。在已识别的项目内部，自动跳过源码和资源目录（`src`、`public`、`static` 等）。Maven 父工程（带 `<modules>`）、npm workspaces 根目录和 Gradle 多项目根目录被视为容器，继续向下查找子项目。
- **识别类型与推断规则**：
  - **Spring Boot（Maven）**：`pom.xml` 包含 `spring-boot`。启动命令 `mvn spring-boot:run`（存在 `mvnw` 时优先使用 `./mvnw`）。端口优先读取 `src/main/resources/application*.yml/properties` 中的 `server.port`，缺省 8080。Java 版本读取自 `.sdkmanrc` / `.java-version` / `pom.xml`（`java.version` / `maven.compiler.*`）。
  - **Spring Boot（Gradle）**：`build.gradle(.kts)` 使用 `org.springframework.boot`。启动命令 `./gradlew bootRun`。端口同上；Java 版本来自 toolchain / `sourceCompatibility`。
  - **Tomcat WAR**：`pom.xml` 中 `<packaging>war</packaging>`。启动命令 `mvn package` 并部署运行。端口默认 8080。由于需要指定 Tomcat 路径，扫描添加时默认置为停用（`enabled = false`），需填入 `tomcat` 安装目录后启用。
  - **Vite（Vue / React）**：依赖中包含 `vite`。启动命令按锁文件自动选择 `pnpm dev` / `yarn dev` / `npm run dev`。端口从脚本参数 `--port` / `-p` 或 `vite.config.*` 读取，缺省 5173。Node 版本读取自 `.nvmrc` / `.node-version` / `package.json`（`engines.node`）。
  - **Vue CLI**：依赖中包含 `@vue/cli-service`。启动命令 `npm run serve` 等。端口从脚本或 `vue.config.js` 的 `devServer.port` 读取，缺省 8080。
  - **Next.js / Nuxt**：依赖中包含 `next` 或 `nuxt`。启动命令 `pnpm dev` 等。端口从脚本参数或配置文件读取，缺省 3000。
  - **React（CRA）/ 普通 Node**：依赖中包含 `react-scripts` 或仅有 `start` 脚本。启动命令 `npm start` 等，端口从脚本中推断或需手动指定。
  - **Django / FastAPI / Flask**：根据 `manage.py` 或依赖中的 `fastapi` / `uvicorn` / `flask` 识别。启动命令使用 `python manage.py runserver` / `uvicorn` / `flask run`（存在 `.venv` 时优先使用其中的 Python 环境）。默认端口分别为 8000 / 8000 / 5000。
- **去重与导入机制**：
  - 扫描报告输出每项把握程度（`high` / `medium` / `low`）、推断依据说明（`why`），以及建议的 id、名称、命令、端口与环境版本。
  - 去重保护：已登记的项目（按项目根路径 `root` 判定）会被标为「已注册」，使用 `--apply` 或导入时直接跳过，**不会重复添加，也不会覆盖已有项目配置**。
  - 建议的 id 自动避开配置中已有的 id，防止冲突。
  - 写入配置时使用 `toml_edit`，保留 `~/.config/tongyun-dev/projects.toml` 中原有的注释与排版。

## 运行行为与启停机制

- **路径与变量展开**：
  - `~`、`$HOME`、`${HOME}` 会展开成绝对路径；`{{root}}`、`{{tomcat}}`、`{{log_dir}}` 用展开后的值替换。若无法展开（如缺失 HOME 环境变量）直接报错，不会拼接异常路径。
- **后台运行项目（`background = true`）**：
  - 如 Spring Boot 项目（`demo_api`）或前端项目（`demo_web`）。命令在独立的新进程组中执行，stdout/stderr 追加写入日志，PID 记录至 pid 文件。
  - **前端依赖自动安装**：若项目根目录有 `package.json` 但缺失 `node_modules`，且配置 `auto_install = true`（缺省值），启动前会先按锁文件（pnpm / yarn / npm）自动执行依赖安装，状态置为 `installing`，装好后接着启动。
  - **端口参数规范**：pnpm 直接传递端口参数（如 `pnpm dev --port 5173`），不要写 `--`；npm 才需要 `npm run dev -- --port 5173`。
- **Tomcat 项目**：
  - 针对传统 WAR 项目（`demo_legacy`），配置 `tomcat` 字段。`stop` 命令优先执行配置脚本（如 `catalina.sh stop 15 -force`）。
  - 若停止超时后仍需发信号终止，仅针对 Java/Tomcat 进程，不杀占用端口的其他无关程序。
- **通用项目停止机制**：
  - 未配置 `tomcat` 的项目，`stop` 会先执行配置的停止命令（若有），随后向 pid 记录的进程组发送 TERM 信号；若超过宽限期仍存活则发送 KILL 强制终止，并清理端口监听。
- **Java / Node 工具链解析**：
  - 项目配置 `java = "17"` 或 `node = "20"` 时，core 在拉起进程前自动在系统常见路径中查找匹配的版本目录，并直接注入 `JAVA_HOME` 与设置 `PATH`。
  - 不需要、也不会在 shell 中执行 `source` sdkman / fnm 的初始化脚本。
  - 若本机缺少对应版本，启动时直接报错并给出可读原因（例如找不到指定版本），杜绝使用错误版本运行。
  - 也可通过 `java_home` / `node_home` 显式指定安装目录。
- **日志输出与跟踪**：
  - `logs` 查看日志尾部；`--follow` 在文件尚未生成时会自动等待，并在生成后持续跟随。

## 启动状态

`tongyun-dev` 采用核心启停状态机（`LifeState`），Status JSON 中的 `state` 包含六个状态：

| 状态 | 说明 |
| --- | --- |
| `stopped` | 已停止。服务未启动或已完全停止，无存活进程且未监听端口 |
| `installing` | 安装依赖中。检测到前端项目缺少 node_modules，启动前正在自动执行依赖安装 |
| `starting` | 启动中。进程已拉起或正在启动，端口尚未连通；无启动标记但检测到进程存活且端口未通也归为此状态（`running = false`） |
| `port_open` | 端口已通。服务端口已可连接，但配置了 `login_url` / `health_url` 探针且探针尚未返回就绪状态（200 / 301 / 302） |
| `ready` | 就绪。端口已通且探针成功（或未配置探针时端口连通即就绪）。此时前端/控制台可打开页面 |
| `failed` | 失败。启动命令报错、启动标记记录超时仍未就绪、或启动后进程过早退出。包含 `failure` 失败原因说明 |

**状态规则与兼容性：**
- `running` 语义：仅当 `state` 为 `port_open` 或 `ready` 时才为 `true`。进程存活但端口未通时为 `starting`，`running = false`。
- 端口被其他程序占用（`port_conflict = true`）不算本项目 `ready` / `port_open`；`port_holder` 记录占用进程的 PID 与名称。

**超时与宽限配置：**
- **启动超时**（就绪等待超时）：优先级为 项目字段 `start_timeout_secs` > 环境变量 `TONGYUN_DEV_START_TIMEOUT` > 默认值 `180` 秒。
- **停止宽限**（`stop_grace_secs`）：优先级为 项目字段 `stop_grace_secs` > 默认值 `8` 秒。停止时先向 pid 对应的进程组发送 TERM 信号，超过宽限期仍存活则向进程组发送 KILL 强制终止。

## 配置字段

`~/.config/tongyun-dev/projects.toml` 里每个 `[projects.<id>]`：

| 字段 | 说明 |
| --- | --- |
| `name` / `root` / `port` | 显示名、项目根、端口。启用的项目端口不能为 0 |
| `enabled` | 缺省 `true`。`false` 时桌面列表变灰并显示 `reason`，`start` 直接失败 |
| `reason` | `enabled = false` 时必填说明原因 |
| `start` / `stop` | 命令数组。`{{root}}` `{{tomcat}}` `{{log_dir}}` 会替换；`~` 和 `$HOME` 会展开 |
| `background` | `true` 时把命令放到后台新进程组，stdout/stderr 追加到日志，PID 写入 pid 文件。缺省 `false` |
| `tomcat` | Tomcat 安装路径（仅 Tomcat 项目需要）。`stop` 走 `catalina.sh`，终止进程时只针对 Java/Tomcat |
| `login_url` | 可选。HTTP 200 / 301 / 302 才算启动完成 |
| `health_url` | 可选。没有 `login_url` 时用它做探针，状态里仍放在 `login_probe` |
| 两者都空 | 端口能连上即算就绪 |
| `node` | 可选（如 `"20"`）。启动前自动找到对应版本并注入 PATH，不 source sdkman/fnm；缺失时给出可读报错 |
| `java` | 可选（如 `"17"`）。启动前自动找到对应版本并注入 JAVA_HOME / PATH，不 source sdkman/fnm；缺失时给出可读报错 |
| `java_home` / `node_home` | 可选。显式指定安装目录（Windows 或特殊环境自定义安装路径时使用） |
| `auto_install` | 可选，缺省 `true`。前端缺少 node_modules 时启动前自动装依赖；`false` 时仅提示依赖未安装 |
| `shell` | 可选。单条命令使用的 shell（macOS/Linux 默认 `"bash"`，可选 `"sh"`；Windows 默认 `"cmd"`，可选 `"powershell"`） |
| `start_windows` / `stop_windows` | 可选。Windows 环境下优先使用的启动 / 停止命令数组 |
| `start_timeout_secs` | 可选（如 `30`）。启动就绪等待超时秒数，优先级高于环境变量 `TONGYUN_DEV_START_TIMEOUT` 与默认 180 秒 |
| `stop_grace_secs` | 可选（如 `5`）。停止等待 TERM 宽限秒数，超时后发送 KILL，默认 8 秒 |
| `log_dir` / `log_file` / `pid_file` | 可省略。默认日志与 PID 路径为 `~/.tongyun-dev/logs/<id>.out` 和 `.pid` |

## 验收清单

用于验证核心功能的通用验收清单（可用示例项目或你自己的一个项目进行验证）：

1. [ ] **扫描目录添加项目**：执行 `tongyun-dev scan ~/code` 或在界面中点击「扫描目录…」，能正确识别出 Spring Boot、Vite 前端等项目类型、端口与推荐工具链版本；带 `--apply` 保存时，已注册过的目录（相同 root）会被跳过且不覆盖已有配置。
2. [ ] **状态查询与 JSON 输出**：执行 `tongyun-dev status demo_api --json`（或你自己的一个项目），能返回标准 JSON，包含 `state`、`running`、`pid`、`port`、`log_path` 与探针状态；未启动时 `state = "stopped"`，`running = false`。
3. [ ] **分阶段启停与就绪探针**：执行 `tongyun-dev start demo_api --open`，经历 `starting` -> `port_open` 直到就绪探针（或端口连通）成功变为 `ready`，并自动打开浏览器；执行 `tongyun-dev stop demo_api` 后进程退出且端口释放，状态回归 `stopped`。
4. [ ] **端口冲突检测**：端口被非本项目的外部程序占用时，`status --json` 能正确标记 `port_conflict = true`，并在 `port_holder` 中显示占用程序的 PID 与进程名，不误判为本项目就绪。
5. [ ] **日志查看与跟随**：执行 `tongyun-dev logs demo_api` 能正确输出日志内容；带 `--follow` 参数能在文件生成及追加内容时持续跟随输出。
6. [ ] **网页控制台与桌面 App 表现一致**：启动 `tongyun-dev`（默认 `http://127.0.0.1:9099`）与桌面应用，卡片/列表中的项目状态、启停操作及日志展示完全一致；停用项（`enabled = false`）置灰且展示禁用原因。
7. [ ] **快捷打开页面与目录**：执行 `tongyun-dev open demo_api` 能在浏览器中打开对应服务页面；执行 `tongyun-dev open demo_api --dir` 能在系统文件管理器中打开项目根目录。
8. [ ] **Java / Node 版本切换与缺失报错**：配置了 `java = "17"` 或 `node = "20"` 的项目，启动前能自动注入对应工具链环境；若本机未安装声明的版本，启动时给出明确可读的错误提示，而不是使用错误版本运行。
9. [ ] **项目增删与配置保留**：通过 CLI（`tongyun-dev add` / `remove`）或界面操作增删项目，配置正确写入 `~/.config/tongyun-dev/projects.toml`，且原有文件注释与结构不受影响。
10. [ ] **安全与敏感信息隔离**：全程终端与日志中不打印数据库密码、Token 等敏感信息；项目配置文件只存在于本地，不进入 Git 仓库。
