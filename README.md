# tongyun-dev

本地开发机控制台：注册项目、启停、看状态、跟日志。对标 IDEA 运行面板，**人能看面板，AI 能读 JSON 状态和日志流**。

- 仓库：私有，MVP 做完再问是否公开
- 技术栈：先 **Rust CLI**（明天可用）；桌面壳（Tauri 2 + React）后补，同系 tongyun-git
- 第一个内置项目：`legacy_app`（acme 工作平台）

## 快速用

### 1. 可视化控制台（双击 / 无参数直接启动并自动打开浏览器）
```bash
cargo run -p tongyun-dev-cli
# 或指定 ui 子命令：
cargo run -p tongyun-dev-cli -- ui
```
启动后自动唤起默认浏览器控制台（`http://127.0.0.1:9099`），支持卡片化状态轮询、一键启停、查看实时日志、动态添加新项目。

### 2. 命令行与 AI Agent 常用命令
```bash
cargo run -p tongyun-dev-cli -- status legacy_app         # 查看状态
cargo run -p tongyun-dev-cli -- status legacy_app --json  # 稳定 JSON 状态（供 Agent 解析）
cargo run -p tongyun-dev-cli -- start legacy_app --open   # 启动并在就绪后自动拉起浏览器
cargo run -p tongyun-dev-cli -- open legacy_app           # 一键在浏览器打开项目入口
cargo run -p tongyun-dev-cli -- stop legacy_app           # 优雅停止并等待端口释放
cargo run -p tongyun-dev-cli -- logs legacy_app --follow  # 跟随实时日志
cargo run -p tongyun-dev-cli -- add <id> --name <名称> --root <路径> --start <启动命令> # 动态注册项目
cargo run -p tongyun-dev-cli -- remove <id>            # 移除已注册项目
```

配置文件位于 `config/projects.toml`（亦支持通过命令或界面动态增删，路径可用环境变量 `TONGYUN_DEV_CONFIG` 覆盖，勿把密钥写进仓）。

## 文档

- [AGENTS.md](AGENTS.md) — AI 开发规则
- [docs/prd.md](docs/prd.md) — 产品范围
- [docs/plan.md](docs/plan.md) — 排期
- [docs/development.md](docs/development.md) — 命令与验收
