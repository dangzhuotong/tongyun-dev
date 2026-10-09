# tongyun-dev PRD（MVP）

## 目标

让本机多个开发项目（先做 `td_work`）能一键启停、看状态、跟日志；输出对人和 AI 都友好。

## 非目标（MVP 不做）

- 断点调试、远程调试
- 依赖 IDEA Ultimate
- 改 82 / 线上安全组
- 多用户、云端托管

## 用户故事

1. 作为开发者，我能 `start td_work`，脚本同步资源并起 Tomcat，登录页就绪后成功退出。
2. 作为开发者，我能 `stop td_work`，Tomcat 停干净、8080 释放。
3. 作为开发者 / Agent，我能 `status td_work --json` 看到：运行中否、pid、端口、登录页探测码、日志路径。
4. 作为开发者 / Agent，我能 `logs td_work [--follow]` 读 catalina 日志。

## 验收（短）

见 [docs/development.md](development.md)「MVP 验收清单」。
