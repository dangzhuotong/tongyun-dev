# 持久化操作审计与配置恢复

2026-10-16 补充：在共享 core 中记录 start/stop/restart、依赖安装、交互输入、项目和分组配置修改，CLI、网页与桌面复用同一实现。状态轮询和普通日志输出仍按原接口工作，不宣称每次后台轮询都写审计。

## 调用

```sh
tongyun-dev --task task-demo --actor agent:demo start demo_api --json
tongyun-dev operations
```

actor/task 为全局 CLI 参数，只用于关联，不能视作认证身份。单项目 JSON 操作增加 schemaVersion、operationId、taskId，保留原 ok/action/id/status/logs/error。JSON 操作的外层记录关联核心内的细分动作。批量流程内部的各项目调用会记录，不额外改变批量输出格式。

网页 `GET /api/operations` 返回 `{ok,events}`，沿用当前 Host/Origin 检查；它不是对多用户远程开放的授权服务。CLI 端以调用方标签记录，网页标为 web:local，未显式传上下文的桌面核心调用标为 local:unspecified。

## 文件与语义

审计目录为当前配置文件同目录的 audit/；默认在用户配置目录，若显式指定源码目录内的配置，则审计也会保存在该配置旁。事件使用 JSON v1 字段 schemaVersion、operationId、taskId、actor、tool、action、target、phase、timestampMs、outcome，与 Git/API 语义一致。每项操作独立保存 started、finished，不用一个会被并发覆盖的大 JSON 数组。

- 开始持久化不可用时禁止操作；独立事件文件使用 create_new + sync_all。
- started 没有 finished 表示中断/待核查，不能推断失败或自动重试。
- outcome=completed 表示这次函数调用返回，不代表 detached 进程已经健康；就绪以现有 status/wait 为准。failed_or_unknown 同样需要结合进程状态核查。
- 公共事件不保存命令参数、环境变量值、stdin 内容、运行日志或配置全文。
- macOS/Linux 目录 0700、文件 0600；Windows 使用用户配置目录继承 ACL，尚未完成 Windows 真机权限验收。

## 配置备份

配置写入前校验新内容，再持久化 configuration.write 开始记录和 `<operationId>.before.toml` 私有备份，然后以唯一临时文件替换配置；结束写记录。备份保存真实旧配置，可能包含本机私有环境，禁止进入仓库或作为公共日志输出。

恢复时先核对当前配置与后续修改，停止正在进行的配置写入，在本机将选定 before.toml 备份恢复到原配置位置并重新读取；本次没有添加自动覆盖恢复按钮，也没有承诺重启/安装操作能恢复外部业务数据。原有服务进程/端口状态校验继续有效。

审计与备份目录已忽略；目前未实现跨进程配置事务锁、备份自动清理、统一跨工具总控 UI 或自动补偿。不要将操作追溯等同于对所有外部副作用可回滚。

新增仅用于隔离测试的 tempfile（Apache-2.0 OR MIT）管理临时审计目录；运行逻辑仍使用现有 Rust/Serde/文件系统。

执行证据见 [2026-10-16 验收](testing/runs/2026-10-16-agent-safety.md)。跨工具联调：`python3 scripts/cross-tool-smoke.py --workspace <三个仓库的父目录>`，需先构建两个 Rust CLI。
