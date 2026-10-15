# 02 · 扫描推断 Spring profile / env 预设

KIND: feat  
参考：PRD「Spring profile / 环境预设」；扫描现生成裸 `mvn spring-boot:run`。

## 目标

扫描 Spring Boot 项目时，从 `application-*.yml` / `.properties` 存在性推断候选 profile，写入候选的 `profiles` / `default_profile`（或等价引导字段），避免用户手改 toml 才能 `-Dspring-boot.run.profiles=dev`。

## 要做

1. `scan.rs`：列出 `src/main/resources/application-<name>.*` 的 name 作为候选 profile。
2. 若存在 `dev`，默认 `default_profile = "dev"`；生成的 start 在选中 profile 时应带对应 JVM/Maven 参数（与现有 `profiles.<name>.start` 机制对齐，不要另起一套）。
3. 无 profile 文件时行为与现在一致（裸 `spring-boot:run`）。
4. 临时目录夹具测试覆盖：有 `application-dev.yml` / 无 profile / 仅 `application.yml`。

## 验收

- 扫描产出的 Candidate（或 add 请求）含可落地的 profile 预设，用户确认后无需手敲 profiles。
- 老配置无 profiles 字段仍能读、默认行为不变。
- 相关 `scan.rs` 单测绿；无公司路径。

## 禁止

- 不写真实业务 profile 名进示例以外的断言；单次 ≤15m；不 push main。
