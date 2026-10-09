# 开发与验收

## 命令

在仓库根目录（Rust 1.85+，`rustup` 的 stable）：

```bash
cargo build -p tongyun-dev-cli
cargo test -p tongyun-dev-cli
cargo run -p tongyun-dev-cli -- status td_work --json
cargo run -p tongyun-dev-cli -- start td_work
cargo run -p tongyun-dev-cli -- stop td_work
cargo run -p tongyun-dev-cli -- logs td_work --follow
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

`tauri dev` 打开窗口：左侧列出全部项目。`enabled = false` 的项变灰，并显示 `reason`，启动按钮不可用。启动 / 停止进行中按钮保持「启动中…」「停止中…」，列表锁定，直到命令结束。状态区显示 `running`、`pid`、`port`、`login_probe`。端口被别的进程或其他项目占用时出现警告（`port_conflict` / `port_holder`）。「打开页面」用系统浏览器打开 `login_url` 或 `health_url`（都没有时打开 `http://127.0.0.1:<port>/`）。「打开日志」用系统默认程序打开日志文件。下方日志区每 1.5 秒重读该项目的日志尾部。配置查找与 CLI 相同：`TONGYUN_DEV_CONFIG`，否则从当前目录往上找 `config/projects.toml`。

Mac 上 `npm run tauri build` 的应用在 `apps/desktop/src-tauri/target/release/bundle/macos/tongyun-dev.app`。安装步骤见 [README](../README.md)。

## 行为（td_work）

- `~`、`$HOME`、`${HOME}` 会展开成绝对路径；`{{root}}`、`{{tomcat}}`、`{{log_dir}}` 用展开后的值替换。展开失败（没有 HOME）直接报错，不会把路径写成 `/code/...`。
- `start` 在项目根目录执行 `scripts/start-td-work.sh`，继承当前终端的环境，并设置 `CATALINA_HOME`、`CATALINA_BASE`、`CATALINA_PID`。不用 `bash -l`。脚本返回后最多再等 180 秒（`TONGYUN_DEV_START_TIMEOUT`），直到登录页为 200、301 或 302。
- `stop` 执行 `catalina.sh stop 15 -force`。已经停掉则成功退出。若 20 秒后端口或 Tomcat 仍在，只对 Java/Tomcat 进程发 TERM，再不行发 KILL。非 Java 占用 8080 时不杀，命令失败并写出 pid 与进程名。
- `status --json` 的 `running` 表示 pid 文件或监听端口上有存活的 Java/Tomcat。`log_path` 是绝对路径。端口没开时不做 HTTP 请求，`login_probe.http_code` 为 `null`。`port_conflict` 为真表示端口上的进程不是本项目的服务；`port_holder` 是 `pid` 和进程名，能对上其他项目的 pid 文件时带上那个 id。Spring / pnpm 只有 pid 文件（或其子进程）才算 `running`，别人占着端口算冲突，不算已启动。
- `logs` 打印 catalina 日志；`--follow` 在文件还没生成时会等待，再用 `tail -F` 跟随。

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
| `log_dir` / `log_file` / `pid_file` | 可省略。默认 `~/code/_index/logs/<id>.out` 和 `.pid` |

Spring 用 `bash -c` 拉起 sdkman 再 `mvn spring-boot:run -Dspring-boot.run.profiles=dev`。`td_new_toefl` 会在当前 Java 不是 8 时，从 `~/.sdkman/candidates/java/8.*` 里挑一个 `sdk use`。前端用 `fnm exec --using=22` 跑 `pnpm dev`。不用 login shell。

没有 `tomcat` 的项目，`stop` 先跑配置里的 stop（如果有），再对 pid 文件里的进程组发 TERM/KILL，并清掉该端口上的监听（node 或 java 都会停）。

## 已注册项目

| id | 端口 | 根目录 | 说明 |
| --- | --- | --- | --- |
| `td_work` | 8080 | `~/code/testdaily/tdprojects` | Tomcat，登录页探针 |
| `td_new_toefl` | 8034 | `~/code/testdaily/01-new-toefl/td_new_toefl` | Java 8，`profiles=dev` |
| `td_new_toefl_vue` | 5777 | `~/code/testdaily/01-new-toefl/td-new-toefl-vue` | pnpm / fnm 22 |
| `td_new_toefl_student_vue` | 5999 | `~/code/testdaily/01-new-toefl/td-new-toefl-student-vue` | pnpm / fnm 22 |
| `ap_whitepaper` | 8888 | `~/code/testdaily/02-ap/td-ap-whitepaper-backend` | Spring |
| `ap_whitepaper_web` | 7888 | `~/code/testdaily/02-ap/td-ap-whitepaper-react` | pnpm / fnm 22 |
| `ap_admin_test_center` | 8110 | `~/code/testdaily/02-ap/td-ap-admin-test-center` | Spring |
| `ap_admin_test_center_vue` | — | `~/code/testdaily/02-ap/admin-test-center-vue` | `enabled=false`，本机没有这个目录 |
| `testdaily_system_ap_java_backend` | — | `~/code/testdaily/02-ap/testdaily-system-ap-java-backend` | `enabled=false`，私有 Maven 被挡住 |
| `lj_class_admin_manager` | 8147 | `~/code/testdaily/03-lj-class/lj-class-admin/lj_class_admin_manager` | Spring |
| `lj_class_admin_web` | 8146 | `~/code/testdaily/03-lj-class/lj-class-admin/lj_class_admin_web` | 管理端前端，pnpm / fnm 22 |
| `ljstudentmanageserver` | — | `~/code/testdaily/03-lj-class/ljstudentmanageserver` | `enabled=false`，`td-job-sdk` 被挡住 |

`04-papers`、`05-wechat`、`06-admin-misc` 以及 `lj_student_manage_vue` 已登记，`enabled=false`，原因是启动命令还没确认。路径以 `config/projects.toml` 为准。

## MVP 验收清单（给运维/产品）

在装有 td_work 的本机（默认 macOS）：

1. [ ] `config/projects.toml` 里 `td_work` 路径指向本机 `~/code/testdaily/tdprojects`
2. [ ] `tongyun-dev status td_work --json` 返回 JSON，含 `running`、`pid`、`port`、`login_probe`、`log_path`
3. [ ] `tongyun-dev stop td_work` 后 status.running=false，8080 无监听
4. [ ] `tongyun-dev start td_work` 后登录页 `http://127.0.0.1:8080/td_work/admin/login.html` 返回 200（301 / 302 也算探针成功）
5. [ ] `tongyun-dev logs td_work` 能打印 catalina 日志；`--follow` 能跟随
6. [ ] 全程终端不打印密钥；仓内无 secrets

## 本机约定（td_work）

- 项目根：`~/code/testdaily/tdprojects`
- 启动：`scripts/start-td-work.sh`（内含先 stop 再 start）
- 停止：`$TOMCAT_HOME/bin/catalina.sh stop 15 -force`（默认 `~/opt/apache-tomcat-9.0.98`）
- 日志：`~/code/_index/logs/td_work_catalina.out`
- PID：`~/code/_index/logs/td_work_tomcat.pid`
- 端口：8080；探针：`/td_work/admin/login.html`
