# 验收用例

两类分开跑。自动化在 CI 里，不启动 Tomcat，不碰端口 8080。真机只在装有 `td_work` 的 Mac 上做，顺序与 [development.md](../development.md) 的清单一致。

## 自动化（CI）

实现在 `crates/tongyun-dev-cli` 的单元测试和 `tests/cli.rs`。

| ID | 步骤 | 期望 |
| --- | --- | --- |
| AT-AUTO-01 | 解析仓库里的 `config/projects.toml` | 存在 `td_work`；根、启动脚本、Tomcat、catalina、pid、8080、登录 URL 与 PRD 默认值一致；文件不含 password / token / secret 等字样 |
| AT-AUTO-02 | 用假 home `/home/dev` 展开 `td_work` | `~` 与 `{{root}}` `{{tomcat}}` `{{log_dir}}` 变成绝对路径；stop 脚本里不再残留 `~` 或 `{{` |
| AT-AUTO-03 | `~/`、`~`、带空格的 `~/`、绝对路径、相对路径、`~other` | 只有开头的 `~` / `~/` 展开；`~other` 保持原样 |
| AT-AUTO-04 | `root` 的值里含有字面量 `{{tomcat}}` | 替换只发生一次，不会把嵌进去的占位符再展开 |
| AT-AUTO-05 | 设置 `TONGYUN_DEV_TD_WORK_ROOT` / `_LOG_DIR`，`_TOMCAT` 设成空白 | 根和日志目录被覆盖；空白的 Tomcat 覆盖不生效；`{{root}}` 用的是新根 |
| AT-AUTO-06 | 非法 TOML、缺字段、空的 start | 解析失败 |
| AT-AUTO-07 | `--config`、`TONGYUN_DEV_CONFIG`、从子目录向上找 | 优先级是 flag、环境变量、再向上查找 |
| AT-AUTO-08 | pid 文件缺失、空、`0`、非数字、` 42 ` | 只有正整数 pid 被读出 |
| AT-AUTO-09 | 假探针：pid 活着 / 过期 / 只有端口开着；HTTP 200 与 503 | `running` 只跟活 pid 有关；`port_open` 独立；`ok` 仅 200/301/302；JSON 键集合与 PRD 一致 |
| AT-AUTO-10 | `SystemProbe` 对本进程 `kill -0`，对 pid 0 和已退出的子进程 | 本进程为活；0 与已回收 pid 为死。不把 `u32::MAX` 当成测试 pid（`kill` 会把它看成 -1）。不调用 Tomcat |
| AT-AUTO-11 | `SystemProbe` 对 `TcpListener` 端口 0 | 监听中为开；关掉监听后为关。端口不是 8080 |
| AT-AUTO-12 | stop 命令为 `false` 且假探针已经 down | 命令成功，不等待满 20 秒 |
| AT-AUTO-13 | stop 命令成功但端口一直开着，超时 0 | 非 0，消息含 `timed out` 与 `port_open=true` |
| AT-AUTO-14 | stop 参数里放一个假 token，且 pid 仍活着 | 失败信息含退出状态，不含该参数 |
| AT-AUTO-15 | `tail` 参数 | 默认不含 `-F`；`--follow` 含 `-n` 与 `-F` |
| AT-AUTO-16 | 日志文件不存在 | 失败信息为 `log file missing`，不创建文件 |
| AT-AUTO-17 | 二进制 `--help` | 含 list、start、stop、status、logs |
| AT-AUTO-18 | 在临时仓库的子目录执行 `list`，`HOME` 指向临时目录 | 按 id 排序；`~` 按该 HOME 展开；输出不含启动命令 |
| AT-AUTO-19 | `--config` 指向好文件，同时 `TONGYUN_DEV_CONFIG` 指向不存在的文件 | 使用 `--config`。未知 id 非 0 |
| AT-AUTO-20 | pid 文件写测试进程自己的 pid；`TONGYUN_DEV_DEMO_ROOT` 覆盖根 | `running` 为 true，`pid` 等于测试进程，`port_open` 为 false，`root` 是覆盖值 |
| AT-AUTO-21 | 临时 HTTP 返回 200，端口来自 `TcpListener` | `port_open` 为 true，`login_probe.http_code` 为 200，`ok` 为 true |
| AT-AUTO-22 | 日志有 5 行，`--lines 1`；再 `--follow` 并追加一行 | 只看到最后一行；follow 能读到新行。随后杀掉 tail |
| AT-AUTO-23 | `start` 在项目根 `touch` 标记文件，再 `stop` | 标记文件出现；stop 成功。另一次 stop 的参数含假 token 且进程仍在：stderr 不含该 token |
| AT-AUTO-24 | 缺少日志文件、`--config` 指向不存在的路径 | 均非 0，stderr 说明原因 |

## 真机（Mac，`td_work`）

与 development.md 清单同一顺序。云端和 CI 不跑这张表。

| ID | 对应清单 | 步骤 | 期望 |
| --- | --- | --- | --- |
| AT-MAC-01 | 第 1 步 | 确认 toml 或 `TONGYUN_DEV_TD_WORK_*` | 根目录是这台 Mac 上的 `td_work`，仓库里没有密钥 |
| AT-MAC-02 | 第 2 步 | `tongyun-dev status td_work --json` | 退出码 0，PRD 列出的键都在 |
| AT-MAC-03 | 第 3 步 | `tongyun-dev start td_work`，再看登录页或 `login_probe.ok` | 启动成功；`http://127.0.0.1:8080/td_work/admin/login.html` 为 200，或 `login_probe.ok` 为 true |
| AT-MAC-04 | 第 4 步 | `logs td_work`，然后 `logs td_work --follow` | 能看到 catalina；follow 能跟上新行；Ctrl-C 结束 |
| AT-MAC-05 | 第 5 步 | `tongyun-dev stop td_work`，再 `status --json` | stop 成功；`running` 为 false；`port_open` 为 false |
| AT-MAC-06 | 第 6 步 | 回看整段终端输出和 git 状态 | 没有密钥、Token、数据库密码 |

## 不在 CI 里的事

- 执行 `scripts/start-td-work.sh`。
- 对真实 Tomcat 发 `catalina.sh stop`。
- 请求真实的 `/td_work/admin/login.html`。
- 证明 8080 在那台 Mac 上被释放。这是 AT-MAC-05。
