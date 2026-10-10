# 开发与验收

## 命令

在仓库根目录（Rust 1.85+，`rustup` 的 stable）：

```bash
cargo build -p tongyun-dev-cli
cargo test -p tongyun-dev-cli

# 启动网页控制台并自动打开浏览器（无参数默认行为）
cargo run -p tongyun-dev-cli
cargo run -p tongyun-dev-cli -- ui

# CLI 与 Agent 命令
cargo run -p tongyun-dev-cli -- status legacy_app --json
cargo run -p tongyun-dev-cli -- start legacy_app --open
cargo run -p tongyun-dev-cli -- open legacy_app
cargo run -p tongyun-dev-cli -- stop legacy_app
cargo run -p tongyun-dev-cli -- logs legacy_app --follow
cargo run -p tongyun-dev-cli -- add my_app --name "我的应用" --root "~/code/my_app" --start "pnpm dev" --port 3000 --node 22
cargo run -p tongyun-dev-cli -- remove my_app
```

二进制名是 `tongyun-dev`（`./target/debug/tongyun-dev`）。逻辑在 `tongyun-dev-core`，CLI、网页控制台（`crates/tongyun-dev-cli/src/server.rs`，只做 HTTP 路由和页面）和桌面壳共用。`add` / `remove` 也在 core 里（`Loaded::add_project` / `remove_project`，用 toml_edit 保留注释）。

```bash
cargo test -p tongyun-dev-core
cargo test -p tongyun-dev-cli   # 含网页控制台的 HTTP 集成测试
```

## 桌面壳

需要 Node.js 20+。Mac 上还要 Xcode Command Line Tools。在 `apps/desktop`：

```bash
cd apps/desktop
npm install
npm run typecheck
npm run tauri dev
npm run tauri build
```

`tauri dev` 打开窗口：左侧列出全部项目。`enabled = false` 的项变灰，并显示 `reason`，启动按钮不可用。启动 / 停止进行中按钮保持「启动中…」「停止中…」，列表锁定，直到命令结束。状态区显示 `running`、`pid`、`port`、`login_probe`。端口被别的进程或其他项目占用时出现警告（`port_conflict` / `port_holder`）。「打开页面」用系统浏览器打开 `login_url` 或 `health_url`（都没有时打开 `http://127.0.0.1:<port>/`）。「打开日志」用系统默认程序打开日志文件。下方日志区每 1.5 秒重读该项目的日志尾部。配置查找顺序与 CLI 相同：1) 显式 `--config` 参数 → 2) 环境变量 `TONGYUN_DEV_CONFIG` → 3) `~/.config/tongyun-dev/settings.json` 记住的路径 → 4) 默认配置 `~/.config/tongyun-dev/projects.toml` → 5) 从当前目录及上级目录找 `config/projects.toml`。未找到配置时桌面端主区域会展示空状态，支持选择文件或一键创建默认配置。

Mac 上 `npm run tauri build` 的应用在 `apps/desktop/src-tauri/target/release/bundle/macos/tongyun-dev.app`。安装步骤见 [README](../README.md)。

## 行为（legacy_app）

- `~`、`$HOME`、`${HOME}` 会展开成绝对路径；`{{root}}`、`{{tomcat}}`、`{{log_dir}}` 用展开后的值替换。展开失败（没有 HOME）直接报错，不会把路径写成 `/code/...`。
- `start` 在项目根目录执行 `scripts/start-legacy.sh`，继承当前终端的环境，并设置 `CATALINA_HOME`、`CATALINA_BASE`、`CATALINA_PID`。不用 `bash -l`。脚本返回后最多再等 180 秒（`TONGYUN_DEV_START_TIMEOUT`），直到登录页为 200、301 或 302。
- `stop` 执行 `catalina.sh stop 15 -force`。已经停掉则成功退出。若 20 秒后端口或 Tomcat 仍在，只对 Java/Tomcat 进程发 TERM，再不行发 KILL。非 Java 占用 8080 时不杀，命令失败并写出 pid 与进程名。
- `status --json` 的 `running` 表示 pid 文件或监听端口上有存活的 Java/Tomcat。`log_path` 是绝对路径。端口没开时不做 HTTP 请求，`login_probe.http_code` 为 `null`。`port_conflict` 为真表示端口上的进程不是本项目的服务；`port_holder` 是 `pid` 和进程名，能对上其他项目的 pid 文件时带上那个 id。Spring / pnpm 只有 pid 文件（或其子进程）才算 `running`，别人占着端口算冲突，不算已启动。
- `logs` 打印 catalina 日志；`--follow` 在文件还没生成时会等待，再用 `tail -F` 跟随。

## 启动状态

`tongyun-dev` 采用核心启停状态机（`LifeState`），Status JSON 中的 `state` 包含五个状态：

| 状态 | 说明 |
| --- | --- |
| `stopped` | 已停止。服务未启动或已完全停止，无存活进程且未监听端口 |
| `starting` | 启动中。进程已拉起或正在启动，端口尚未连通；无启动标记但检测到进程存活且端口未通也归为此状态（`running = false`） |
| `port_open` | 端口已通。服务端口已可连接，但配置了 `login_url` / `health_url` 探针且探针尚未返回就绪状态（200 / 301 / 302） |
| `ready` | 就绪。端口已通且探针成功（或未配置探针时端口连通即就绪）。此时前端/控制台可打开页面 |
| `failed` | 失败。启动命令报错、启动标记记录超时仍未就绪、或启动后进程过早退出。包含 `failure` 失败原因说明 |

**状态规则与兼容性：**
- `running` 语义：仅当 `state` 为 `port_open` 或 `ready` 时才为 `true`。进程存活但端口未通时为 `starting`，`running = false`。
- 端口被其他程序占用（`port_conflict = true`）不算本项目 `ready` / `port_open`。

**超时与宽限配置：**
- **启动超时**（就绪等待超时）：优先级为 项目字段 `start_timeout_secs` > 环境变量 `TONGYUN_DEV_START_TIMEOUT` > 默认值 `180` 秒。
- **停止宽限**（`stop_grace_secs`）：优先级为 项目字段 `stop_grace_secs` > 默认值 `8` 秒。停止时先向 pid 对应的进程组发送 TERM 信号，超过宽限期仍存活则向进程组发送 KILL 强制终止。

## 配置字段

`config/projects.toml` 里每个 `[projects.<id>]`：

| 字段 | 说明 |
| --- | --- |
| `name` / `root` / `port` | 显示名、项目根、端口。启用的项目端口不能为 0 |
| `enabled` | 缺省 `true`。`false` 时桌面列表变灰并显示 `reason`，`start` 直接失败 |
| `reason` | `enabled = false` 时必填 |
| `start` / `stop` | 命令数组。`{{root}}` `{{tomcat}}` `{{log_dir}}` 会替换；`~` 和 `$HOME` 会展开 |
| `background` | `true` 时把 `mvn` / `pnpm` 放到新进程组，stdout/stderr 追加到日志，pid 写入 pid 文件。Tomcat 的启动脚本会自己退出，保持 `false` |
| `tomcat` | 只给 Tomcat。`stop` 走 `catalina.sh`，杀进程时只动 Java/Tomcat，不杀占着 8080 的其他程序 |
| `login_url` | 可选。HTTP 200 / 301 / 302 才算启动完成 |
| `health_url` | 可选。没有 `login_url` 时用它做同样的探针，状态里仍放在 `login_probe` |
| 两者都空 | 端口能连上即算就绪。Spring 的 `/` 经常不是 200，所以后端默认不写探针 |
| `node` | 可选（如 `"20"`）。CLI 启动时自动加 `fnm exec --using=<v> --`，缺失时给出可读报错 |
| `java` | 可选（如 `"17"`）。CLI 启动时经 `bash -lc` 执行 `sdk use java <v>` 切换，缺失时给出可读报错 |
| `start_timeout_secs` | 可选（如 `30`）。启动就绪等待超时秒数，优先级高于环境变量 `TONGYUN_DEV_START_TIMEOUT` 与默认 180 秒 |
| `stop_grace_secs` | 可选（如 `5`）。停止等待 TERM 宽限秒数，超时后发送 KILL，默认 8 秒 |
| `log_dir` / `log_file` / `pid_file` | 可省略。默认 `~/.tongyun-dev/logs/<id>.out` 和 `.pid` |

Spring 用 `bash -c` 拉起 sdkman 再 `mvn spring-boot:run -Dspring-boot.run.profiles=dev`。`demo_exam` 会在当前 Java 不是 8 时，从 `~/.sdkman/candidates/java/8.*` 里挑一个 `sdk use`。前端用 `fnm exec --using=22` 跑 `pnpm dev`。不用 login shell。

没有 `tomcat` 的项目，`stop` 先跑配置里的 stop（如果有），再对 pid 文件里的进程组发 TERM/KILL，并清掉该端口上的监听（node 或 java 都会停）。

## 已注册项目

| id | 端口 | 根目录 | 说明 |
| --- | --- | --- | --- |
| `legacy_app` | 8080 | `~/code/acme/legacy-app` | Tomcat，登录页探针 |
| `demo_exam` | 8034 | `~/code/acme/01-exam/demo_exam` | Java 8，`profiles=dev` |
| `demo_exam_vue` | 5777 | `~/code/acme/01-exam/demo-exam-vue` | pnpm / fnm 22 |
| `demo_exam_student_vue` | 5999 | `~/code/acme/01-exam/demo-exam-student-vue` | pnpm / fnm 22 |
| `ap_docs-site` | 8888 | `~/code/acme/02-ap/acme-ap-docs-site-backend` | Spring |
| `ap_docs-site_web` | 7888 | `~/code/acme/02-ap/acme-ap-docs-site-react` | pnpm / fnm 22 |
| `ap_admin_test_center` | 8110 | `~/code/acme/02-ap/acme-ap-admin-test-center` | Spring |
| `ap_admin_test_center_vue` | — | `~/code/acme/02-ap/admin-test-center-vue` | `enabled=false`，本机没有这个目录 |
| `acme_system_ap_java_backend` | — | `~/code/acme/02-ap/acme-system-ap-java-backend` | `enabled=false`，私有 Maven 被挡住 |
| `ops_class_admin_manager` | 8147 | `~/code/acme/03-ops-class/ops-class-admin/ops_class_admin_manager` | Spring |
| `ops_class_admin_web` | 8146 | `~/code/acme/03-ops-class/ops-class-admin/ops_class_admin_web` | 管理端前端，pnpm / fnm 22 |
| `opsstudentmanageserver` | — | `~/code/acme/03-ops-class/opsstudentmanageserver` | `enabled=false`，`acme-job-sdk` 被挡住 |

`04-papers`、`05-wechat`、`06-admin-misc` 以及 `ops_student_manage_vue` 已登记，`enabled=false`，原因是启动命令还没确认。路径以 `config/projects.toml` 为准。

## MVP 验收清单（给运维/产品）

在装有 legacy_app 的本机（默认 macOS）：

1. [ ] `config/projects.toml` 里 `legacy_app` 路径指向本机 `~/code/acme/legacy-app`
2. [ ] `tongyun-dev status legacy_app --json` 返回 JSON，含 `running`、`pid`、`port`、`login_probe`、`log_path`
3. [ ] `tongyun-dev stop legacy_app` 后 status.running=false，8080 无监听
4. [ ] `tongyun-dev start legacy_app` 后登录页 `http://127.0.0.1:8080/legacy_app/admin/login.html` 返回 200（301 / 302 也算探针成功）
5. [ ] `tongyun-dev logs legacy_app` 能打印 catalina 日志；`--follow` 能跟随
6. [ ] 全程终端不打印密钥；仓内无 secrets
7. [ ] `tongyun-dev`（无参数）打开 `http://127.0.0.1:9099`：卡片和桌面窗口列出的项目一致，停用项变灰带原因；在网页里启停 `legacy_app` 与 CLI `status` 结果一致
8. [ ] 网页「＋ 注册项目」填 Node 22 加一个前端项目，`config/projects.toml` 出现该项且原有注释还在；桌面窗口刷新后能看到它和 `Node 22`；网页里移除后文件里消失
9. [ ] 声明了 `java` 的项目在装好 sdkman 的 Mac 上能按版本启动；没装对应版本时报可读错误而不是用错版本硬跑

## 本机约定（legacy_app）

- 项目根：`~/code/acme/legacy-app`
- 启动：`scripts/start-legacy.sh`（内含先 stop 再 start）
- 停止：`$TOMCAT_HOME/bin/catalina.sh stop 15 -force`（默认 `~/opt/tomcat-9`）
- 日志：`~/.tongyun-dev/logs/legacy_app_catalina.out`
- PID：`~/.tongyun-dev/logs/legacy_app_tomcat.pid`
- 端口：8080；探针：`/legacy_app/admin/login.html`
