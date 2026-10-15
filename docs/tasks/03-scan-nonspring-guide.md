# 03 · 非 Spring Boot Maven：认不出就引导

KIND: feat  
参考：PRD「认不出就引导」；现状 `detect_maven` 对非 boot 直接 `return None`。

## 目标

扫到 `pom.xml` 但不是 spring-boot 时，不要静默跳过：给出可编辑候选（mainClass、`application*.yml` 端口、`<packaging>`），让用户选完能 `add`，目标是减少「disabled / 未配置」数量。

## 要做

1. 非 boot 路径返回 `Candidate`（confidence Low/Medium），`kind` 如 `maven-plain`，`enabled` 建议 false 并写清 `why`。
2. 收集：`<packaging>`、`<mainClass>`（含 plugin 配置）、resources 下端口候选；填进 Candidate 扩展字段或 `why`/`hints`（与现有结构对齐，优先复用字段）。
3. Web / CLI 扫描结果展示这些候选，支持用户改 start 后加入。
4. 测试：纯 jar 库、有 mainClass 的可执行 jar、packaging=pom 多模块（多模块仍可只作 container）。

## 验收

- 非 boot 单模块不再被 scan 完全忽略；至少出现带说明的候选。
- 用户按引导填 start 后能 status/start（虚构夹具）。
- 不把真实公司模块名写进测试。

## 禁止

- 单次 ≤15m；不 push main；不启真实 Tomcat/业务包。
