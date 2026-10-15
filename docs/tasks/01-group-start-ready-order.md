# 01 · 分组启动依赖顺序：ready 后再起下一项

KIND: feat  
参考：总览 PRD 第 3 节第 1 项「项目分组与依赖顺序」；`docs/plan.md` 已有 `depends_on` DAG 与组级启停。

## 目标

Web / 桌面点「启动本组」时，按 `depends_on` 拓扑顺序启动，**每一项等到 Ready（或明确失败）再启下一项**。  
现状：CLI `start_batch` 会 `start_with_profile` 等待；GUI 走的 `start_batch_detached` 只按序 `start_detached`，前端可能在后端未就绪时已起。

## 要做

1. 让组级 / 批量 detached 路径也串行等待就绪（可后台线程里顺序 wait，立刻返回「已开始」状态给 UI）。
2. 依赖失败则跳过下游，并在 batch 结果 / status 里写清原因。
3. 网页组卡片、桌面组级按钮行为与 CLI `--group` 一致。
4. 补集成测试：虚构两项目 A→B，A 未 ready 前 B 不得进入 Starting。

## 验收

- Given 组内 `demo_api` depends_on 空、`demo_web` depends_on=`["demo_api"]`  
  When 调用组 start（detached 或 Web API）  
  Then `demo_web` 仅在 `demo_api` 为 Ready（或失败并跳过）后才启动。
- 环路仍被检测并拒绝；失败一句话出现在 batch item `error`。
- `cargo test -p tongyun-dev-core -p tongyun-dev-cli` 绿。

## 禁止

- 公司路径 / 真实项目名不进仓；单次 ≤15m；不 push main；不改用户 `projects.toml`。
