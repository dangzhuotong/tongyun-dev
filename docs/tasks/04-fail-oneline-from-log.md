# 04 · 启动失败一句话原因（从日志抓）

KIND: feat  
参考：PRD「失败一句话原因」；已有 `StatusJson.failure` / `hints`，但对日志尾部常见异常覆盖不足。

## 目标

进程已退出或 Failed 时，从日志尾部提炼一行可读原因（优先：最后一个异常类名+消息、`Port already in use`、`Could not resolve dependencies`、Maven 私服 401/403 类固定卡点），展示在 CLI status、网页、桌面卡片，而不是让人翻整个 log。

## 要做

1. core 增加纯函数：输入 log tail 文本 → `Option<String>` 一行原因（可另附 hints）。
2. 在 derive status / Failed 路径调用；已有 failure 时优先保留更具体的日志提炼，或合并不重复。
3. 单测覆盖上述三类固定句式 + 无匹配时回退原 failure。
4. UI 已显示 `failure` 则只需确认绑定；缺则补一行。

## 验收

- 虚构日志含 `Port already in use` 时 status.failure（或 hints）出现对应中文/英文一句。
- `Could not resolve dependencies` 同理。
- `cargo test -p tongyun-dev-core` 相关用例绿。

## 禁止

- 不上传真实用户日志；单次 ≤15m；不 push main。
