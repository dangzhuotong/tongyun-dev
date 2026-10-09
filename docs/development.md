# 开发与验收

## 命令（建立后）

```bash
cargo build -p tongyun-dev-cli
cargo test -p tongyun-dev-cli
cargo run -p tongyun-dev-cli -- status td_work --json
```

## MVP 验收清单（给运维/产品）

在装有 td_work 的本机（默认 macOS）：

1. [ ] `config/projects.toml` 里 `td_work` 路径指向本机 `~/code/testdaily/tdprojects`
2. [ ] `tongyun-dev status td_work --json` 返回 JSON，含 `running`、`pid`、`port`、`login_probe`、`log_path`
3. [ ] `tongyun-dev stop td_work` 后 status.running=false，8080 无监听
4. [ ] `tongyun-dev start td_work` 后登录页 `http://127.0.0.1:8080/td_work/admin/login.html` 返回 200（或文档约定的探针成功）
5. [ ] `tongyun-dev logs td_work` 能打印 catalina 日志；`--follow` 能跟随
6. [ ] 全程终端不打印密钥；仓内无 secrets

## 本机约定（td_work）

- 项目根：`~/code/testdaily/tdprojects`
- 启动：`scripts/start-td-work.sh`（内含先 stop 再 start）
- 停止：`$TOMCAT_HOME/bin/catalina.sh stop`（默认 `~/opt/apache-tomcat-9.0.98`）
- 日志：`~/code/_index/logs/td_work_catalina.out`
- PID：`~/code/_index/logs/td_work_tomcat.pid`
- 端口：8080；探针：`/td_work/admin/login.html`
