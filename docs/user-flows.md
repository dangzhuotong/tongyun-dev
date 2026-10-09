# 用户流程

命令细节与退出码见 [prd.md](prd.md)。真机勾选顺序见 [development.md](development.md)。

角色：

- 开发者：在终端看人读输出。
- Agent：只解析 `list --json` 和 `status --json`，日志用 `logs` 的 stdout。

## 主路径：status → start → logs → stop

这是 Mac 验收的顺序。`td_work` 的启动脚本自己会先停再起，所以从 `start` 开始是安全的，即使上次没关干净。

1. **看当前状态。** `tongyun-dev status td_work --json`。Agent 读 `running`、`port_open`、`login_probe.ok`。人可以去掉 `--json`。
2. **启动。** `tongyun-dev start td_work`。stderr 出现 `starting td_work`。stdout 是人读状态，不是 JSON。要给 Agent 用，再跑一次 `status --json`，确认 `login_probe.ok` 或用浏览器打开登录 URL。
3. **看日志。** `tongyun-dev logs td_work` 打出 catalina 末尾。要盯启动过程，用 `tongyun-dev logs td_work --follow`，看完 Ctrl-C。
4. **停止。** `tongyun-dev stop td_work`。成功后再 `status td_work --json`：`running` 为 false，`port_open` 为 false。

## 列出项目

`tongyun-dev list` 或 `list --json`。根目录是展开 `~` 并应用环境变量之后的路径。输出里没有启动命令，也没有登录 URL。

## Agent 读状态

```bash
tongyun-dev status td_work --json
```

只读 stdout 的 JSON。stderr 不保证为空（例如停止时的提示）。字段缺了或改名都算契约破坏，单测锁着键名。

判断建议：

- 页面可以打开：`login_probe.ok == true`。
- 进程表里有我们记录的 pid：`running == true`。
- 端口被占用：`port_open == true`。它和 `running` 可以不一致，不要只看其中一个。

## 换一台机器的路径

默认路径在 `config/projects.toml`。这台机器目录不同时：

```bash
export TONGYUN_DEV_TD_WORK_ROOT="$HOME/code/testdaily/tdprojects"
export TONGYUN_DEV_TD_WORK_TOMCAT="$HOME/opt/apache-tomcat-9.0.98"
export TONGYUN_DEV_TD_WORK_LOG_DIR="$HOME/code/_index/logs"
```

然后从任意目录执行，或设置 `TONGYUN_DEV_CONFIG` / `--config`。不要把本机绝对路径和密码提交进仓库。

## 注册另一个项目

M1 没有交互式注册。在 `projects.toml` 增加 `[projects.<id>]`，字段与 `td_work` 相同，路径继续用 `~` 和占位符。`tongyun-dev list` 能看到新 id 即注册成功。M2 的界面仍读这个文件，不另起格式。

## 异常

| 现象 | 用户看到的 | 接下来 |
| --- | --- | --- |
| id 写错 | stderr：`unknown project id`，提示 `list` | 用 `list` 核对 id |
| 找不到配置 | stderr：`config file not found` 或提示设置 `--config` | 指到 toml，或 cd 到仓库里 |
| 日志文件还没有 | `logs` 非 0，`log file missing` | 先 `start`，或确认 `log_file` 展开对了 |
| 停止命令失败但已经停了 | stderr 说明已经 down，退出码仍为 0 | 再 `status --json` 确认 |
| 停不掉 | 约 20 秒后非 0，带 `pid_alive` 和 `port_open` | 看 catalina；不要在 CLI 里加 `kill -9` 的隐藏步骤 |
| 端口开着但 `running` 为 false | JSON 同时给出两个事实 | 可能 pid 文件过期或不是我们起的进程；先看 `pid_path` 和 `port` |
| 登录不是 200/301/302 | `login_probe.ok` 为 false，`http_code` 为码或 null | 看 `logs`。`start` 本身不会因为探测失败而失败 |
