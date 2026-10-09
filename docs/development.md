# 开发与验收

## 命令（建立后）

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
cargo run -p tongyun-dev-cli -- add my_app --name "我的应用" --root "~/code/my_app" --start "npm run dev" --port 3000
cargo run -p tongyun-dev-cli -- remove my_app
```

## MVP 验收清单（给运维/产品）

在装有 legacy_app 的本机（默认 macOS）：

1. [ ] `config/projects.toml` 里 `legacy_app` 路径指向本机 `~/code/acme/legacy-app`
2. [ ] `tongyun-dev status legacy_app --json` 返回 JSON，含 `running`、`pid`、`port`、`login_probe`、`log_path`
3. [ ] `tongyun-dev stop legacy_app` 后 status.running=false，8080 无监听
4. [ ] `tongyun-dev start legacy_app` 后登录页 `http://127.0.0.1:8080/legacy_app/admin/login.html` 返回 200（或文档约定的探针成功）
5. [ ] `tongyun-dev logs legacy_app` 能打印 catalina 日志；`--follow` 能跟随
6. [ ] 全程终端不打印密钥；仓内无 secrets

## 本机约定（legacy_app）

- 项目根：`~/code/acme/legacy-app`
- 启动：`scripts/start-legacy.sh`（内含先 stop 再 start）
- 停止：`$TOMCAT_HOME/bin/catalina.sh stop`（默认 `~/opt/tomcat-9`）
- 日志：`~/.tongyun-dev/logs/legacy_app_catalina.out`
- PID：`~/.tongyun-dev/logs/legacy_app_tomcat.pid`
- 端口：8080；探针：`/legacy_app/admin/login.html`
