# 05 · 项目卡片快捷动作

KIND: feat  
参考：PRD「卡片上加：在 Cursor 打开目录、打开日志文件、复制启动命令」。

## 目标

网页控制台与桌面项目卡片（或详情行）提供三个快捷动作：

1. 在 Cursor 打开项目目录（`cursor` CLI 或 `cursor://file/...`；不可用则降级已有「打开目录」）。
2. 打开日志文件（桌面已有入口则对齐网页）。
3. 复制当前生效启动命令到剪贴板。

## 要做

1. 复用已有 `open-dir` / log path / assembled start；只加薄 UI 与必要 API。
2. 复制命令用组装后的实际 start（含 active profile），不要复制未解析模板。
3. 纯函数（拼 cursor URI、格式化命令）加 vitest / Rust 单测。
4. 无新原生权限需求则不做；Cursor 未安装时按钮可用但提示降级。

## 验收

- 三端至少 Web + Desktop 能完成「复制启动命令」；打开日志在已有能力上对齐。
- 点击不崩溃；无公司路径进仓。
- desktop vitest / 相关测试绿。

## 禁止

- 单次 ≤15m；不 push main；不改用户全局 IDE 配置。
