# 开发与验收

## 命令

在仓库根目录（Rust 1.85+，`rustup` 的 stable）：

```bash
cargo build -p tongyun-dev-cli
cargo test -p tongyun-dev-cli
cargo run -p tongyun-dev-cli -- status legacy_app --json
cargo run -p tongyun-dev-cli -- start legacy_app
cargo run -p tongyun-dev-cli -- stop legacy_app
cargo run -p tongyun-dev-cli -- logs legacy_app --follow
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

`tauri dev` 打开窗口：左侧只列出 `enabled` 的项目（含 `legacy_app` 和各条产品线），按钮为启动 / 停止 / 刷新状态，状态区显示 `running`、`pid`、`port`、`login_probe`。下方日志区每 1.5 秒重读该项目的日志尾部。`enabled = false` 的项目不进列表；`tongyun-dev list` 仍会打印它们和 `reason`。配置查找与 CLI 相同：`TONGYUN_DEV_CONFIG`，否则从当前目录往上找 `config/projects.toml`。

## 行为（legacy_app）

- `~`、`$HOME`、`${HOME}` 会展开成绝对路径；`{{root}}`、`{{tomcat}}`、`{{log_dir}}` 用展开后的值替换。展开失败（没有 HOME）直接报错，不会把路径写成 `/code/...`。
- `start` 在项目根目录执行 `scripts/start-legacy.sh`，继承当前终端的环境，并设置 `CATALINA_HOME`、`CATALINA_BASE`、`CATALINA_PID`。不用 `bash -l`。脚本返回后最多再等 180 秒（`TONGYUN_DEV_START_TIMEOUT`），直到登录页为 200、301 或 302。
- `stop` 执行 `catalina.sh stop 15 -force`。已经停掉则成功退出。若 20 秒后端口或 Tomcat 仍在，只对 Java/Tomcat 进程发 TERM，再不行发 KILL。非 Java 占用 8080 时不杀，命令失败并写出 pid 与进程名。
- `status --json` 的 `running` 表示 pid 文件或监听端口上有存活的 Java/Tomcat。`log_path` 是绝对路径。端口没开时不做 HTTP 请求，`login_probe.http_code` 为 `null`。
- `logs` 打印 catalina 日志；`--follow` 在文件还没生成时会等待，再用 `tail -F` 跟随。

## 配置字段

`config/projects.toml` 里每个 `[projects.<id>]`：

| 字段 | 说明 |
| --- | --- |
| `name` / `root` / `port` | 显示名、项目根、端口。启用的项目端口不能为 0 |
| `enabled` | 缺省 `true`。`false` 时桌面列表隐藏，`start` 直接失败并带上 `reason` |
| `reason` | `enabled = false` 时必填 |
| `start` / `stop` | 命令数组。`{{root}}` `{{tomcat}}` `{{log_dir}}` 会替换；`~` 和 `$HOME` 会展开 |
| `background` | `true` 时把 `mvn` / `pnpm` 放到新进程组，stdout/stderr 追加到日志，pid 写入 pid 文件。Tomcat 的启动脚本会自己退出，保持 `false` |
| `tomcat` | 只给 Tomcat。`stop` 走 `catalina.sh`，杀进程时只动 Java/Tomcat，不杀占着 8080 的其他程序 |
| `login_url` | 可选。HTTP 200 / 301 / 302 才算启动完成 |
| `health_url` | 可选。没有 `login_url` 时用它做同样的探针，状态里仍放在 `login_probe` |
| 两者都空 | 端口能连上即算就绪。Spring 的 `/` 经常不是 200，所以后端默认不写探针 |
| `log_dir` / `log_file` / `pid_file` | 可省略。默认 `~/.tongyun-dev/logs/<id>.out` 和 `.pid` |

Spring 用 `bash -c` 拉起 sdkman 再 `mvn spring-boot:run -Dspring-boot.run.profiles=dev`。`demo_exam` 会在当前 Java 不是 8 时，从 `~/.sdkman/candidates/java/8.*` 里挑一个 `sdk use`。前端用 `fnm exec --using=22` 跑 `pnpm dev`。不用 login shell。

没有 `tomcat` 的项目，`stop` 先跑配置里的 stop（如果有），再对 pid 文件里的进程组发 TERM/KILL，并清掉该端口上的监听（node 或 java 都会停）。

## 已注册项目

| id | 端口 | 根目录 | 说明 |
| --- | --- | --- | --- |
| `legacy_app` | 8080 | `~/code/acme/legacy-app` | Tomcat，登录页探针 |
| `demo_exam` | 8034 | `~/code/acme/01-exam/demo_exam` | Java 8，`profiles=dev` |
| `demo_exam_vue` | 5777 | `~/code/acme/01-exam/demo-exam-vue` | pnpm / fnm 22 |
| `demo_exam_student_vue` | 5999 | `~/code/acme/01-exam/student-vue` | pnpm / fnm 22 |
| `ap_docs-site` | 8888 | `~/code/acme/02-ap/docs-site` | Spring |
| `ap_docs-site_web` | 7888 | `~/code/acme/02-ap/docs-site-next` | Next，pnpm / fnm 22 |
| `ap_admin_test_center` | 8110 | `~/code/acme/02-ap/admin-test-center` | Spring |
| `ap_admin_test_center_vue` | 8111 | `~/code/acme/02-ap/admin-test-center-vue` | Vue，pnpm / fnm 22 |
| `acme_system_ap_java_backend` | — | `~/code/acme/02-ap/acme-system-ap-java-backend` | `enabled=false`，私有 Maven 被挡住 |
| `ops_class_admin_manager` | 8147 | `~/code/acme/03-ops-class/ops_class_admin_manager` | Spring |
| `ops_class_admin_web` | 8146 | `~/code/acme/03-ops-class/ops-class-admin-web` | 管理端前端，pnpm / fnm 22 |
| `opsstudentmanageserver` | — | `~/code/acme/03-ops-class/opsstudentmanageserver` | `enabled=false`，`acme-job-sdk` 被挡住 |

`02-ap` 里 docs-site / docs-site-next / admin-test-center / admin-test-center-vue，以及 `ops-class-admin-web` 的文件夹名是按仓库名写的。本机目录不同时只改 `root`。

## MVP 验收清单（给运维/产品）

在装有 legacy_app 的本机（默认 macOS）：

1. [ ] `config/projects.toml` 里 `legacy_app` 路径指向本机 `~/code/acme/legacy-app`
2. [ ] `tongyun-dev status legacy_app --json` 返回 JSON，含 `running`、`pid`、`port`、`login_probe`、`log_path`
3. [ ] `tongyun-dev stop legacy_app` 后 status.running=false，8080 无监听
4. [ ] `tongyun-dev start legacy_app` 后登录页 `http://127.0.0.1:8080/legacy_app/admin/login.html` 返回 200（301 / 302 也算探针成功）
5. [ ] `tongyun-dev logs legacy_app` 能打印 catalina 日志；`--follow` 能跟随
6. [ ] 全程终端不打印密钥；仓内无 secrets

## 本机约定（legacy_app）

- 项目根：`~/code/acme/legacy-app`
- 启动：`scripts/start-legacy.sh`（内含先 stop 再 start）
- 停止：`$TOMCAT_HOME/bin/catalina.sh stop 15 -force`（默认 `~/opt/tomcat-9`）
- 日志：`~/.tongyun-dev/logs/legacy_app_catalina.out`
- PID：`~/.tongyun-dev/logs/legacy_app_tomcat.pid`
- 端口：8080；探针：`/legacy_app/admin/login.html`
