# 发版

正式包只来自 GitHub Release。用户在桌面 App 的 **设置** 或 **关于** 里点 **检查更新**，或等启动后的更新提示。维护者不要在本机用当前开发 tip 重装给用户。

合进 `main` 不会发版。不推 tag，`latest.json` 就不会变，检查更新会停在上一个 Release。

## 怎么发

1. 把要发布的提交合进默认分支。
2. 让下面三处版本号相同，并且等于即将推送的 tag（去掉前缀 `v`）：
   - `apps/desktop/package.json` 的 `version`
   - `apps/desktop/src-tauri/tauri.conf.json` 的 `version`（更新器比较的就是这个）
   - 仓库根 `Cargo.toml` 里 `[workspace.package]` 的 `version`（桌面壳 `version.workspace = true`）
3. 在这份提交上推送 tag：

   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

   例：三个文件都是 `0.2.0` 时，tag 必须是 `v0.2.0`。tag 指向的提交里版本仍是 `0.1.0` 时，发布出去的 `latest.json` 也是 `0.1.0`，已安装 0.2.0 的用户不会看到更新。

4. [`.github/workflows/release.yml`](../.github/workflows/release.yml) 在 tag `v*` 上触发 `tauri-action`：
   - 创建非 draft 的 GitHub Release
   - 用 GitHub Secrets `TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 做 **minisign** 签名（更新器校验包没有被换掉）
   - 上传安装包、`.sig` 和 `latest.json`
   - 更新地址已写在 `tauri.conf.json`：`https://github.com/dangzhuotong/tongyun-dev/releases/latest/download/latest.json`

5. 用户侧：打开 **设置** 或 **关于**，看清「当前版本」，点 **检查更新**。有新版会下载并重启；已经是最新会提示「已是最新版本」。启动约 3 秒后也会静默检查，有新版同样弹窗。

`workflow_dispatch`（Actions 里手动跑 Release）只构建并上传 artifact，**不**创建 Release，也**不**写入 `latest.json`。冒烟必须推 tag。

`.github/workflows/build.yml` 只在手动触发时打未签名安装包，同样不写 `latest.json`。push 到 `main` 不再打安装包，日常检查留在 `check.yml`。

## 签名与 Apple

| | 作用 | 现状 |
| :--- | :--- | :--- |
| minisign（`TAURI_SIGNING_*`） | 更新器确认安装包未被篡改 | 公钥在 `tauri.conf.json`。私钥和密码只进 GitHub Secrets，不要写入仓库 |
| Apple Developer 签名与公证 | 去掉首次打开的「无法验证开发者」 | **未做**（Phase B）。不阻塞检查更新 |

首次从 dmg 安装时，macOS 仍可能提示无法验证。三种办法：

1. 系统设置 → 隐私与安全性 → 往下找到提示，点「仍要打开」；
2. 在「应用程序」里右键点 App，选「打开」，再确认一次；
3. 终端执行：

```bash
xattr -dr com.apple.quarantine /Applications/tongyun-dev.app
```

已经装上、且带本更新器的用户，用检查更新换的是 minisign 签过的包。`0.1.0` 没有更新器，需要从 [Releases](https://github.com/dangzhuotong/tongyun-dev/releases/latest) 手动装一次 `0.2.0`，之后才能应用内升级。

### 写入 Secrets

仓库里只有公钥。私钥由维护者写入仓库 Secrets（Settings → Secrets and variables → Actions）：

- `TAURI_SIGNING_PRIVATE_KEY`：minisign 私钥文件的**全文**（不是路径）
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`：生成私钥时设的密码

本仓库第一次生成的私钥没有进 Git 历史，也没有写进 Pull Request 正文。阿码从云端 agent store / artifact 取出后粘贴进上面两个 Secret。取走后如果那份副本还会被别人看到，应重新生成一对钥匙，只把新公钥提交进 `tauri.conf.json`，新私钥只进 Secrets。

需要重做钥匙时，在 `apps/desktop` 执行（不要把私钥提交进仓库）：

```bash
npm exec tauri signer generate -- -w "$HOME/.tauri/tongyun-dev.key" -f
```

把打印出来的公钥写进 `tauri.conf.json` 的 `plugins.updater.pubkey`，私钥全文和密码只放进 GitHub Secrets。

## 阿装

只在这两件事上介入：

- **Release 工作流失败**（签名 Secret 缺失、某一端构建失败等）：看 Actions 里该 tag 的日志，修好后重跑。正式包仍以修好后的 GitHub Release 为准。
- **用户本机环境坏了**（装不上、隔离解不开）：按上面的 macOS 说明协助，仍把用户带回 GitHub Release 或应用内检查更新。

`main` 每次合并之后，不做例行本机打包和重装。
