# tongyun-dev · 打磨第 1 项任务池（Windows 反重力顺序跑）

对应总览 PRD「第 1 项：一组一起起 / 认不出就引导」的**剩余缺口**。  
已落地的分组 / depends_on / profiles 骨架见 `docs/plan.md`「已补」；本目录只收还没做完或 GUI 路径仍弱的部分。

## 跑法（顺序）

1. `01-group-start-ready-order.md`
2. `02-scan-profile-env.md`
3. `03-scan-nonspring-guide.md`
4. `04-fail-oneline-from-log.md`
5. `05-card-quick-actions.md`

单次 ≤15 分钟；做不完就提交当前进度，下一轮接着同一文件。  
合入由阿党审 PR，**不要 push main**。

## 全局禁止

- 公司 / 真实项目名、本机绝对路径、内网地址、端口表不进仓
- 不改 `~/.config/tongyun-dev`、不启用户真实项目
- 示例只用虚构名（`demo_api`、`acme-web`、`~/code/demo/...`）
