# tongyun-dev PRD（MVP）

## 目标

让本机多个开发项目能一键启停、看状态、跟日志；输出对人和 AI 都友好。注册表覆盖 acme 在用的产品线；依赖被挡住的仓库保持停用。

## 非目标（MVP 不做）

- 断点调试、远程调试
- 依赖 IDEA Ultimate
- 改服务器 / 线上安全组
- 多用户、云端托管

## 用户故事

1. 作为开发者，我能 `start legacy_app`，脚本同步资源并起 Tomcat，登录页就绪后成功退出。
2. 作为开发者，我能 `stop legacy_app`，Tomcat 停干净、8080 释放。
3. 作为开发者 / Agent，我能 `status legacy_app --json` 看到：运行中否、pid、端口、登录页探测码、日志路径。
4. 作为开发者 / Agent，我能 `logs legacy_app [--follow]` 读 catalina 日志。
5. 作为开发者，我能打开桌面窗口，看到全部项目。停用项变灰并显示原因，不能启动。启动 / 停止会停在加载状态直到结束。可以一键打开日志和登录页（或 health URL）。端口被其他进程占用时有警告。

## 验收（短）

见 [docs/development.md](development.md)「MVP 验收清单」。
