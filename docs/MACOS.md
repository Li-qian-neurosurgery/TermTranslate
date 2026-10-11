# TermTranslate：Apple Silicon Mac 版

本版本仅支持 Apple Silicon（M 系列芯片），系统要求 macOS 12 或更新版本。界面与 Windows 0.3.1 一致。

## 安装与使用

打开 `TermTranslate_0.3.1_macOS-arm64.dmg`，将 TermTranslate 拖入 Applications，然后从应用程序中打开。也可解压 `TermTranslate-0.3.1-macOS-arm64.zip`，把其中的 `TermTranslate.app` 放入 Applications。

当前构建使用 ad-hoc 本地签名，未经过 Apple Developer ID 签名和公证。首次打开时，macOS 可能阻止启动；确认安装包来自本仓库后，可按 [Apple 官方说明](https://support.apple.com/zh-cn/102445)，在“系统设置 → 隐私与安全性”中对该应用选择“仍要打开”。不需要关闭系统的整体安全检查。

- **Enter** 发送原文，**Shift+Enter** 换行；也兼容 Command+Enter。
- **Command+Option+T** 唤起窗口。关闭窗口后继续在菜单栏运行；在菜单栏图标中选择“退出 TermTranslate”结束程序。
- 右上角“设置”可以使用 ChatGPT 登录或配置 API Key。Windows 上的登录凭据不会随安装包迁移，需要在 Mac 上重新登录。
- 提示词、设置与成功历史存放在 `~/Library/Application Support/com.termtranslate.desktop`，API Key 和 ChatGPT 令牌使用 macOS Keychain 保存。

## 云构建

仓库 Actions 页选择 **macOS Apple Silicon** → **Run workflow**。提交主分支中的源码或构建脚本也会触发一次构建。流程使用 GitHub 的 macOS 15 Apple Silicon runner，构建目标固定为 `aarch64-apple-darwin`。

构建成功后，在该次运行的 Artifacts 区域下载 `TermTranslate-macOS-arm64-<运行编号>`。包含 DMG、保留可执行权限的 app ZIP、使用说明、SHA-256 校验、构建信息和原生诊断报告。工作流只上传构建附件，不自动创建公开 Release。

流程执行前端和 Rust 测试，检查可执行文件仅包含 arm64、包版本、代码签名完整性和 DMG 完整性，并在独立数据目录中进行原生启动、Keychain、菜单栏、快捷键及单实例诊断。诊断使用虚构凭据，不需要真实 LLM Key。云端诊断不等同于完整的 Mac 人工界面测试，也不代表真实厂商接口已联调。

## 在 Mac 上本地构建

安装 Node.js 22.12 或更新版本、Rust stable 和 Xcode Command Line Tools 后，在项目目录执行：

```bash
bash scripts/build-macos.sh
node scripts/smoke-macos.mjs
```

产物位于 `release/macos-arm64/`。本地原生诊断需要 Apple Silicon Mac；构建脚本也能在有 Apple Silicon Rust target 的 Intel Mac 上交叉编译。

正式发行签名入口保留：在构建 Mac 的 Keychain 中安装 Developer ID Application 证书，设置 `APPLE_SIGNING_IDENTITY`；公证可设置 `APPLE_ID`、应用专用密码 `APPLE_PASSWORD` 和 `APPLE_TEAM_ID`。凭据只配置在构建机环境或仓库 Secrets 中，不写入源码。当前云流程固定使用 ad-hoc 签名，配置正式证书时需相应更新工作流。

参考：[Tauri macOS 签名](https://v2.tauri.app/distribute/sign/macos/)、[GitHub 托管 runner](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)。
