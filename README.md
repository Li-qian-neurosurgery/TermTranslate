# TermTranslate

终端对话风格的 LLM 桌面翻译器。Tauri 2 / React / TypeScript / Rust，黑底、等宽字体、中文界面，默认中英自动互译。

## Windows 使用

运行 `TermTranslate_0.3.1_x64-setup.exe` 安装，或解压 `TermTranslate-0.3.1-win-x64.zip` 后运行 `TermTranslate.exe`。程序需要 Microsoft Edge WebView2；安装包会在缺少它时下载运行时。无需安装 Node、Rust 或 .NET。升级前保存正在编辑的原文并从托盘退出旧版；新版本继续使用原来的设置、提示词、登录凭据和历史数据。版本号可在窗口标题栏和设置页查看。

0.3.1 精简主界面：移除顶部品牌标题、标语和空白对话的欢迎说明，收紧顶部留白，直接展示翻译对话与输入框。

### 对话式翻译（0.3 新增）

- 底部输入或粘贴原文，**Enter 发送，Shift+Enter 换行**；也兼容 Ctrl/Command+Enter。中文输入法选词时不会发送。
- 每次发送追加一组 `› 原文`、`• 译文`，译文逐步显示。发送后输入框清空，可继续准备下一段。
- 每条译文可独立复制、重新翻译，显示完成状态和耗时。重试使用当前模型、语言和提示词，保留旧回复与尚未发送的草稿。
- 向上滚动查看旧内容时不会被流式输出拉走，点击“回到最新”恢复跟随；长段落、公式和代码按纯文本换行显示。
- 生成中可停止，也可直接发送下一段：上一轮保留已生成文字并标为停止。
- 右上角“新对话”清空当前显示和输入草稿，已保存的成功翻译仍在“历史”里。历史详情可“加入当前对话”。
- 底部可快速切换模型、目标语言和模板。账户、订阅登录及模板编辑在“设置”中。

当前对话保留在窗口中，退出程序后可从本机历史找回最近 100 条成功翻译；未发送的草稿和失败、取消的回复不会自动落盘。对话界面中的每段原文仍是一次独立翻译，不把前面的对话一起发送给模型。

### 使用 ChatGPT 订阅额度（0.2 新增）

1. 打开设置 → 模型接口 → **ChatGPT 订阅**，点击 **使用 ChatGPT 登录**。
2. 在系统浏览器中自行登录 ChatGPT、选择工作空间并授权套餐使用权限。无需向本软件填写账号密码或 API Key。
3. 返回软件，阅读首次套餐使用提示；从账户返回的模型列表中选择模型，点击 **使用此模型**。
4. 返回工作台开始翻译。底部显示“ChatGPT 套餐”，点击可打开官方用量设置；设置页也保留“管理订阅额度”。

已有账户可重新登录；“使用 ChatGPT 登录”可添加另一个账户或工作空间。每个账户保留独立注册与凭据。首次未授权套餐时，可点击“启用订阅额度”重新授权。登录等待支持取消，10 分钟后超时。

本功能使用 OpenAI 官方 [Sign in with ChatGPT 本地应用流程](https://developers.openai.com/siwc/token-sharing-open-source)，通过系统浏览器授权、127.0.0.1 回调和官方 Responses API 工作，不读取 ChatGPT 对话历史。可用性、模型、额度及是否允许使用额外点数以账户和工作空间授权为准；达到套餐或应用上限时保留部分译文并显示原因。软件不会自动改用 API Key 计费。

访问凭据会在接近到期时自动续期。退出会尝试撤销远端会话、清除本机凭据，并保留账户注册用于下次登录；网络故障导致撤销未确认时会明确提示，也可在 ChatGPT 设置中断开应用。API Key 和 OAuth 访问、刷新、身份令牌均保存在系统凭据库，较长的令牌按分块与版本指针保存；普通文件不含令牌。

需要网络可访问 `auth.openai.com`、`api.openai.com`，以及系统浏览器可访问 ChatGPT 登录页面。本功能有官方[预览限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)。账户不支持或权限受限时，软件会展示提示；本次未代替用户登录或使用真实订阅额度。

### 使用 API Key

1. 点击右上角设置，选择厂商预设。
2. 填写模型 ID 和 API Key，核对 Base URL。模型权限、Key 和计费由对应厂商提供。
3. 点击“保存配置”；可用“测试连接”发送一条简短请求验证，会使用厂商 API 额度。
4. 输入或粘贴原文，按 `Enter` 翻译。点击每条译文旁的“复制”获取纯文本译文。

`Ctrl+Alt+T` 从其他软件唤起窗口，`Shift+Enter` 换行。中文输入法组词期间不会误触发翻译。关闭窗口后驻留托盘，通过托盘菜单退出程序。重复启动会打开已有窗口。

默认原文以中文为主时译成英文，否则译成简体中文；语言栏可指定目标语言。语言判断由同一次模型请求完成。翻译准确性及格式保留取决于所选模型。

## 模型配置

| 预设 | 协议 |
|---|---|
| OpenAI | Responses |
| Claude | Anthropic Messages |
| Gemini | streamGenerateContent |
| DeepSeek、通义、豆包、Kimi、智谱 | OpenAI 兼容 Chat Completions |
| 自定义 | 以上四种协议任选 |

Base URL 填基础地址，不包含末尾的 `/responses`、`/messages`、`/chat/completions` 或模型操作路径。通义需要把预设中的 `{WorkspaceId}` 替换为业务空间 ID，并核对地区。Kimi 预设使用国内地址，国际账号应填写对应地址。豆包可以填写支持的模型 ID 或推理接入点 ID。

每个配置可独立设置名称、地址、模型和 15–600 秒超时。可以保存多个同厂商配置。模型名自由填写，软件不假定账户可访问某一型号。不启用工具调用，不发送翻译历史作为上下文。

云服务使用官方 API Key；本地免鉴权服务可留空。网络请求由 Rust 发送，支持进程启动时的 `HTTPS_PROXY`、`HTTP_PROXY` 和 `NO_PROXY` 环境变量。应用不会自动继承浏览器扩展代理。网络异常、限流、输出截断会明确显示，保留部分译文，由用户决定重试。

## 提示词与历史

“提示词”页内置通用、学术、技术文档模板；支持编辑、复制、新建、删除自建模板和恢复内置默认值。变量为 `{{source_language}}` 和 `{{target_language}}`，可展开查看最终指令。原文作为单独的用户消息发送；保存修改后，下次翻译生效。

默认仅保存最近 100 条成功翻译。历史支持搜索原文/译文、复制、恢复到工作台及删除。偏好设置中可关闭新增记录，已有历史由历史页清空。失败、取消和连接测试不写入翻译历史。

普通设置、提示词和历史保存为带版本号的 JSON，原子替换写入；文件损坏时保留 `corrupt-*.json` 副本并提示。`chatgpt.json` 保存主机 ID、账户标识、客户端注册和授权状态，令牌保存在 Windows Credential Manager / macOS Keychain，不写入 JSON 或日志。复制整个数据目录不会携带 API Key 或 ChatGPT 登录凭据。

- Windows 数据目录：`%APPDATA%\com.termtranslate.desktop`
- macOS 数据目录：`~/Library/Application Support/com.termtranslate.desktop`

首版无遥测、NAS 同步、OCR 或文件翻译。应用本身不提供免费模型额度。安装包目前未配置代码签名。

## 开发与构建

需要 Node.js 22.12+、Rust stable，以及平台开发工具：Windows 的 Visual Studio C++ Build Tools + Windows SDK；macOS 的 Xcode Command Line Tools。依赖由 `package-lock.json` 与 `src-tauri/Cargo.lock` 固定。

Windows：

```powershell
.\scripts\build-windows.ps1
.\scripts\build-windows.ps1 -Publish
```

脚本优先使用上级工作区 `.tools/cargo` 和 `.tools/rustup`，否则使用 PATH 中的工具。安装器、程序 ZIP、源码 ZIP 和 SHA-256 校验文件输出到 `release`。

开发窗口：先将 Cargo 加入 PATH，再运行 `npm run tauri -- dev`。

macOS：

```bash
bash scripts/build-macos.sh
```

该脚本只构建 Apple Silicon（arm64）版本，最低 macOS 12，输出 `.dmg`、保留文件权限的 `.app` ZIP 和 SHA-256 校验到 `release/macos-arm64/`。也可通过 GitHub Actions 的 **macOS Apple Silicon** 工作流云构建。安装、云构建、原生诊断及正式签名配置见 [Mac 使用与构建说明](docs/MACOS.md)。默认使用 ad-hoc 签名；发布签名可通过 `APPLE_SIGNING_IDENTITY` 配置，公证使用 `APPLE_ID`、应用专用密码 `APPLE_PASSWORD`、`APPLE_TEAM_ID`，凭据仅置于构建主机环境中。

## 独立验证

`npm test` 检查前端请求竞态、部分结果、输入法键盘行为与 ChatGPT 登录交互。`cargo test --manifest-path src-tauri/Cargo.toml --locked` 检查四类协议、本地 HTTP/SSE、OAuth 登录与续期、取消、错误、持久化和保留规则，不消耗云端额度。`src-tauri/src/chatgpt/test-private-key.pem` 仅为本地测试签名夹具，不能用于 OpenAI 生产服务。

Windows 原生诊断使用独立数据目录：

```powershell
$env:TERMTRANSLATE_DATA_DIR = "$PWD\.artifacts\native-smoke"
.\src-tauri\target\release\termtranslate.exe --smoke-test
```

结果写入该目录的 `native-smoke.json`。诊断使用独立实例标识，创建并删除临时测试凭据，检查短凭据及较长 OAuth 令牌的分块保存、新进程读取、托盘、窗口、快捷键冲突、单实例和文字剪贴板往返。旧版仍运行时，应在诊断目录设置不同的测试快捷键以免冲突。普通启动不会执行诊断。

本地 UI 模拟服务：

```powershell
node scripts/mock-server.mjs --write-fixture
```

在另一个终端将 `TERMTRANSLATE_DATA_DIR` 设为项目的 `.artifacts/ui-smoke` 后启动程序。四个模拟配置返回固定文本，仅用于 UI 与协议验证；不属于真实厂商联调。正常使用前移除该环境变量。验证范围见 `docs/VALIDATION.md`。

## 卸载

从托盘退出，再卸载或删除程序目录。要删除 Key，可先在模型设置中删除对应配置；ChatGPT 凭据通过“退出此账户”清除；要删除本地文字，可在历史页清空。卸载程序不会自动删除用户数据或系统凭据。
