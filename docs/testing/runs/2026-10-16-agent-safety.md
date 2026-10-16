# 共享核心审计与配置备份验收 · 2026-10-16

基线 ffb8391，分支 codex/agent-safety-trace。本机 macOS，测试只使用临时配置、临时服务、回环端口。

- `cargo test -p tongyun-dev-core -p tongyun-dev-cli`：168 个 Rust 测试全部通过，0 失败/忽略。
- apps/desktop 下 `npm run typecheck && npm test && npm run build`：150 个前端测试及类型、构建通过。
- `cargo +stable check -p tongyun-dev-desktop`：通过。
- 实际共享核心修改分组、持久化记录、重新加载后读取成功；配置备份与真实旧内容逐字一致，Unix 文件权限 0600。
- 缺失项目操作记录失败/待核查；不可用审计阻止配置写入，配置内容不变。
- 通过真实 CLI JSON 返回 operationId/taskId，并由第二个 CLI 进程读取对应 finished 事件。
- 实际 Web API /api/operations 返回记录，不输出备份全文。
- `python3 scripts/cross-tool-smoke.py`：真实 Dev 进程启停、API Runner 本机请求、Git 临时仓库提交；同 taskId 的 8 项操作都具备 started/finished，服务已停止。

## 首次失败

原生命周期夹具先 free_port() 再绑定监听器，在并行测试中出现 AddrInUse；改为直接 bind(127.0.0.1:0) 并保留监听器，完整回归通过。跨工具夹具首次填了非法 port_mode=fixed，产品正确拒绝；更正为 none 后通过，未放宽产品校验。

## 边界

本轮没有运行真实业务服务，没有改用户常用项目配置，没有完成 Windows/Linux/macOS 安装包的 GUI 人工验收或签名升级。配置备份只验证字节级恢复材料，没有提供自动恢复按钮，也没有承诺恢复依赖安装或外部业务副作用。批量和桌面默认上下文的关联范围见 docs/agent-safety.md。未提交推送。
