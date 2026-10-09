# 0002 · CI 不启动 Tomcat，真机验收分开

- 背景：M1 要能在 GitHub Actions 和云端 agent 里验证，那些环境没有 `td_work` 也没有 Tomcat。登录页和 8080 又确实是 Mac 验收的一部分。把两者绑在一次 `cargo test` 里，CI 就会要么失败，要么偷偷去起服务器。
- 决定：
  - 自动化测试只用临时目录、假 `Probe`、测试进程自己的 pid，以及测试代码临时监听的端口。不绑定、不请求 8080，不执行 `start-td-work.sh`。
  - `Probe` 是库里的 trait。二进制使用系统实现；测试替换 pid、端口和 HTTP 码。
  - 真实脚本、catalina、8080、登录页放在 Mac 清单（[development.md](../development.md)），不作为 CI 绿灯的条件。
  - `status --json` 的键由单元测试锁定，这样 Agent 契约不依赖活的 Tomcat。
- 后果：CI 绿只说明 CLI 与配置契约成立。合并前仍要在使用的 Mac 上按 status → start → logs → stop 勾一遍。M2 界面复用同一个 trait 和同一份 JSON，不另写一套探测。
- 相关：[0001](0001-cli-first.md)、[验收用例](../testing/acceptance-tests.md)。
