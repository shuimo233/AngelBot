# Windows Daily-Use Release：平台能力边界

> 调研日期：2026-09-17
>
> 范围：只定义 AngelBot 首个 Windows 发行版所需的平台能力、核心接口与明确排除项；不设计 macOS/Linux 实现，也不形成实现 backlog。

## 结论

Daily-Use Release 应只承诺 **Windows 11 x64、当前仍受支持的版本**。Windows 10 已于 2025-10-14 结束常规支持，而 Windows 11 Home/Pro 仍处于支持期；首发继续兼容 Windows 10 或同时交付 ARM64 会扩大安装、签名、WebView2、自动化和恢复矩阵，却不会增加核心日常工作能力。[Windows 10 生命周期](https://learn.microsoft.com/en-gb/lifecycle/announcements/windows-10-end-of-support) [Windows 11 生命周期](https://learn.microsoft.com/en-us/lifecycle/products/windows-11-home-and-pro)

平台边界应是七个小端口，而不是一个包揽一切的 `PlatformService`。业务核心只表达“保存凭据、取得文件授权、显示通知、接收 OAuth 回调”等语义；Windows/Tauri 代码负责注册表、Credential Manager、托盘、协议注册、安装器和 UI Automation。组合根直接装配 Windows 实现，测试装配 deterministic fake；首发不需要平台枚举、运行时后端发现或其他系统的空实现。

| 能力 | Daily-Use Release 必须具备 | 明确不承诺 |
| --- | --- | --- |
| 凭据 | 当前 Windows 用户的系统凭据库；核心只持有 opaque reference | 明文 SQLite/日志、机器级共享密钥、自动导出秘密 |
| 启动与后台 | 用户主动开启登录启动；关闭到托盘；显式退出；重启后恢复持久任务 | Windows 服务、无人登录运行、睡眠期间准点执行 |
| 通知 | 已安装应用的本地通知；点击后回到对应任务；用户/系统禁用时可降级 | 必达、秘密正文、复杂通知内操作 |
| 文件/文件夹 | 原生选择器产生可撤销、按根目录和读写模式限定的 grant | 默认扫描整盘、模型提交任意绝对路径、跨 reparse point 越权 |
| URI/OAuth | 系统浏览器、Authorization Code + PKCE；优先 loopback，必要时静态 deep link；单实例接收 | 内嵌 WebView 登录、客户端密钥、仅凭 URI 即信任回调 |
| 浏览器/系统自动化 | 独立浏览器自动化 profile；桌面只允许已登记应用的启动和草稿填充 | 接管默认浏览器 profile、任意脚本/选择器、密码/UAC/提权/提交操作 |
| 安装/更新/恢复 | 签名的 per-user NSIS；签名更新；安装前 checkpoint；启动时恢复；数据导出/安全模式 | 静默提权、自动数据库降级、把 Windows ARR 当作唯一恢复机制 |

## 推荐的最小接口

这些是能力端口，不是必须照抄的 Rust API。每个调用仍需经过现有 Capability Lease / action policy；模型、worker 和 WebView 都不直接取得原生对象。

```text
CredentialVault
  put(CredentialKey, SecretBytes) -> CredentialRef
  read(CredentialRef) -> SecretBytes
  delete(CredentialRef)
  probe() -> available | unavailable | needs_user_action

AppPresence
  status() -> { launch_at_login, background_mode, process_state }
  set_launch_at_login(bool)
  show_main_window(route?)
  request_quit(reason) -> checkpoint_then_exit

Notifier
  publish({ category, title, redacted_body, activation_ref, dedupe_key })
    -> shown | suppressed | unavailable

FileGrantBroker
  pick({ file|directory, read|read_write, multiple }) -> FileGrant[]
  authorize(FileGrantId, CandidatePath, Operation) -> ResolvedPath
  revoke(FileGrantId)

ExternalAuthBroker
  begin(ProviderAuthRequest) -> { attempt_id, authorization_url }
  accept(Activation) -> AuthCode   // exact attempt/state/path/expiry validation
  cancel(attempt_id)

BrowserAutomation
  execute(BrowserPlan, BrowserLease) -> StructuredEvidence

DesktopAutomation
  execute(LaunchTrustedApp | OpenWindowsSettings | PrepareDraft, DesktopLease)
    -> verified | dispatched | denied | unavailable

ReleaseMaintenance
  check() -> UpdateState
  install(UpdateId) -> checkpoint_then_install
  startup_health() -> normal | recover_previous_state | safe_mode
  export_user_data(destination)
```

保持接口小还有两个安全收益。第一，Tauri 的 capability 只约束 WebView 能调用哪些 IPC；Tauri 明确说明 Rust 核心代码仍拥有完整系统权限，所以实际 scope 检查必须留在命令/端口实现中，不能把 capability 配置误当完整 sandbox。[Tauri 安全模型](https://v2.tauri.app/security/) [Tauri command scopes](https://v2.tauri.app/security/scope/) 第二，通知、托盘、文件选择和更新不应被直接作为通用前端插件表面暴露；前端只调用上述产品语义，避免把插件升级新增的命令自动变成 Agent 权限。

## 1. 凭据存储

Windows 首发应使用当前用户的 **Windows Credential Manager** 保存模型 API key、OAuth refresh token 和 connected-service token。微软对新桌面开发的建议优先使用 Credential Manager；`CredRead` 读取的是当前 token 登录会话关联的 credential set。[Windows 密码处理指导](https://learn.microsoft.com/en-us/windows/win32/secbp/handling-passwords) [CredRead](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credreadw)

边界要求：

- SQLite 只保存 `CredentialRef`、provider/account、scope、创建时间与失效状态，不保存 secret。
- key 名必须由受信代码从 provider/account 构造，不能接受模型提供的 target name。
- secret 只在实际请求前短暂 resolve，不进入日志、错误、事件、导出或 worker payload。
- `put/read/delete/probe` 足够；批量明文导入导出、任意枚举和前端直读不属于发行表面。
- Generic Credential 的 blob 上限为 2560 bytes；token bundle 超限时应拆为单独条目，或只把加密数据密钥放入 Credential Manager 并用 DPAPI 保护本地 blob，不能静默退回明文文件。[CREDENTIAL 结构和大小/持久性](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw) [CryptProtectData](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata)
- 备份默认不包含 secret。恢复后无法读取凭据时显示“重新连接”，不能把 vault 错误伪装成“尚未配置”。

仓库已有 `tauri-plugin-keyring-store`，其 Windows backend 是 Credential Manager，且 `keychain.rs` 已避免把 API key 镜像到 SQLite。这可继续作为 Windows adapter，但应收束到上面的 opaque-reference 端口；业务层不依赖该插件的 Stronghold-shaped session/backup API。

## 2. 登录启动、托盘和后台行为

Windows 桌面应用隐藏窗口后仍是普通运行进程；它不会像 UWP 那样因转入后台自动 suspend，但 Modern Standby、关机、更新、Task Manager 或崩溃仍可停止它。微软要求桌面应用保存状态并处理意外终止，不能假设后台始终获得完整 CPU。[Windows 桌面应用生命周期](https://learn.microsoft.com/en-us/windows/apps/develop/launch/app-lifecycle)

首发语义应固定为：

1. 登录启动默认关闭，由 Local Owner 明确开启；状态读取以 OS 注册为准，而不是只读本地设置。
2. 开启后台模式时，关闭主窗口只隐藏到托盘；托盘至少提供“打开 AngelBot”和“退出”。Tauri 原生支持 tray menu 和由托盘恢复/聚焦窗口。[Tauri system tray](https://v2.tauri.app/learn/system-tray/)
3. “退出”先停止接收新工作、持久化 checkpoint、把未知结果的 attempt 标为待恢复，再结束进程。
4. 登录启动或普通启动后都运行同一个 recovery/reconciliation 流程：认领到期自动化、恢复可安全重试的 attempt、把未知外部副作用升级为需用户确认。
5. 睡眠、关机或应用未运行期间不承诺准点执行；下一次唤醒/启动只做 catch-up。首发不安装 Windows Service，也不为每条 automation 创建 Task Scheduler task。

Tauri autostart 插件提供 `enable/disable/isEnabled`，且默认阻止危险命令直到 capability 显式开放；Windows 也允许用户在 Task Manager 禁用 startup app，因此 AngelBot 必须尊重 OS 事实，不能循环强制重开。[Tauri autostart](https://v2.tauri.app/plugin/autostart/) [Windows startup apps](https://learn.microsoft.com/en-us/windows/win32/w8cookbook/startup-apps)

## 3. 本地通知

通知是 **attention delivery**，不是 durable state。真正的任务结果和待确认事项先写入本地状态，再尽力发送通知；通知被系统关闭、Focus Assist 抑制或发送失败时，主界面仍能恢复全部信息。

最小 payload 只含 category、短标题、脱敏正文、`activation_ref` 和 `dedupe_key`。点击通知应启动/显示单实例并导航到对应 Attention State；通知本身不承载 secret、原始邮件正文或一次性授权 code。复杂 inline reply、多个 side-effect action button 和后台直接提交均推迟。

Tauri notification 插件在 Windows 上只对 **已安装应用**正常工作，并要求先检查/请求 permission，因此通知验收必须在签名安装包上运行，开发模式 smoke 不能替代。[Tauri notifications](https://v2.tauri.app/plugin/notification/) Windows 通知本身可以启动应用或触发后台动作，但 AngelBot 首发只需要“点击后回到对应任务”，所有有副作用的动作仍回到 Main Agent/action policy。[Windows app notifications](https://learn.microsoft.com/en-us/windows/apps/develop/notifications/app-notifications/)

## 4. 文件与文件夹访问

原生选择器是授权入口。Tauri dialog 在 Windows 返回文件系统 path，并支持文件、目录、multiple 和 save；这足够建立持久 `FileGrant`。[Tauri dialog](https://v2.tauri.app/plugin/dialog/)

一个 grant 至少包含：稳定 ID、规范化根路径、`read`/`read_write`、是否递归、Workspace owner、创建/撤销时间。每次真正 I/O 都用 grant ID + 相对路径重新授权；模型或 WebView 不能用任意绝对路径绕过。已有路径 canonicalize 后校验 containment；新建目标先 canonicalize 最近存在的父目录。Windows reparse point 能把目录导向另一设备或位置，因此递归遍历和写入必须在使用时检测/解析 junction、symlink 和其他 reparse point，默认拒绝越出授权根。[Windows reparse points](https://learn.microsoft.com/en-us/windows/win32/fileio/reparse-points)

Tauri filesystem scopes 可按 path allow/deny，但官方也强调自定义 command 必须自己正确执行 scope；因此静态 capability 只保护 IPC，持久 FileGrant 才是 Workspace/Agent 的授权事实。[Tauri filesystem](https://v2.tauri.app/plugin/file-system/) [Tauri command scopes](https://v2.tauri.app/security/scope/)

首发无需 Windows Storage Access Framework token、整盘索引或默认 Documents/Desktop 权限。普通 Win32/Tauri 应用直接使用用户选择的路径；当权限、网络盘、OneDrive placeholder 或文件锁导致失败时，返回稳定错误并允许重新选择/重试。

## 5. URI、deep link 与 OAuth callback

OAuth 必须用系统默认浏览器，不在 AngelBot WebView 内收集账号密码。RFC 8252 要求 native app 使用 external user-agent，public native client 必须使用 PKCE；桌面应用可在随机端口监听 loopback callback。[RFC 8252](https://datatracker.ietf.org/doc/html/rfc8252) 系统浏览器打开可由 Tauri opener 的受限 URL scope 完成。[Tauri opener](https://v2.tauri.app/plugin/opener/)

推荐顺序：

1. provider 支持时首选 `http://127.0.0.1:{ephemeral_port}/oauth/callback/{provider}`；只监听 loopback，短超时，单次消费。
2. provider 不支持 loopback 时才用安装器静态注册的 `angelbot://oauth/callback/{provider}`。
3. 两种方式都要求 authorization code flow、PKCE S256、不可预测 `state`、exact path/provider、attempt expiry 和一次性消费；callback 只把 code 交给对应 attempt，不能携带 access token，也不能自行启用 connected service。
4. OAuth 完成后立即关闭 listener，并把 refresh token 写入 `CredentialVault`。

Tauri 说明 Windows deep link 默认在安装时注册，会作为新进程命令行参数送达；与 single-instance 插件结合时，应先注册 single-instance，再把事件转交原实例。Tauri 同时警告用户可手工伪造 deep-link 参数，因此 scheme 匹配并不构成认证。[Tauri deep linking](https://v2.tauri.app/plugin/deep-linking/) [Tauri single instance](https://v2.tauri.app/plugin/single-instance/)

Deep link 是 **OAuth callback fallback 与任务导航入口**，不是通用命令协议。首发不接受 `angelbot://run?prompt=...`、文件路径、shell 参数或任何能创建 automation/External Action 的 URI。

## 6. 浏览器与 Windows 系统自动化

### 浏览器

Daily-Use Release 的最低浏览器能力应是一个 AngelBot 管理的独立 automation profile，支持导航、结构化读取、点击、填表、下载到已授权目录，以及在 External Action 前停下确认。Playwright 明确警告近期 Chrome policy 下不支持自动化默认 Chrome profile，并要求使用单独 user data directory；非持久 BrowserContext 则不会把浏览数据写盘。[Playwright persistent context](https://playwright.dev/docs/api/class-browsertype#browser-type-launch-persistent-context) [Playwright BrowserContext](https://playwright.dev/docs/api/class-browsercontext)

因此：

- 不读取或复用用户默认浏览器的 cookie、密码库、历史和扩展。
- 用户如需登录网站，应在明确标记的 AngelBot profile 中亲自登录；profile 的使用仍受站点/origin 与 task lease 限定。
- OAuth 总是走系统浏览器，与 automation profile 分离。
- 首发不需要“接管当前任意 tab”。若以后通过浏览器扩展连接现有 tab，该扩展是单独安装、单独授权的 capability；Chrome 要求 native messaging permission，并要求最小化 host permission、把 content-script 消息视为不可信输入。[Chrome native messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) [Chrome extension security](https://developer.chrome.com/docs/extensions/develop/security-privacy/stay-secure)

### 桌面系统

首发继续保持仓库现有窄边界：`OpenTrustedApp`、白名单 `OpenWindowsSettings`、`PrepareDraft`。`PrepareDraft` 只对用户登记的 executable + 唯一、精确、非密码 Edit control 使用 UI Automation `ValuePattern`，写入后回读验证；“发送/提交/保存到外部系统”仍是单独 External Action，不能由 draft 自动连带完成。

Windows UI Automation 的定位是让辅助技术和测试工具读取/操作其他应用控件，但并非所有应用都暴露稳定 automation tree。[UI Automation fundamentals](https://learn.microsoft.com/en-us/windows/win32/winauto/entry-uiautocore-overview) 首发明确拒绝：

- 任意 PowerShell/selector/script、坐标宏、全局键鼠录制；
- password、secure desktop、UAC、锁屏、登录界面；
- 提升 AngelBot 权限以控制 elevated app；
- 声称后台或窗口失焦时输入必定送到正确目标。

`SendInput` 受 UIPI 限制，只能注入相同或更低 integrity level 的应用，而且返回值无法明确指出 UIPI 阻止；它不应成为 release contract 的 fallback。[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)

## 7. 安装、更新和恢复

### 安装

首发只支持一个签名的 **NSIS per-user x64 installer**。Tauri 的 Windows installer 支持 MSI/WiX 与 NSIS；NSIS 默认安装到当前用户 `%LOCALAPPDATA%`，无需管理员权限，正好匹配单一 Local Owner。[Tauri Windows installer](https://v2.tauri.app/distribute/windows-installer/) 不在首发同时维护 machine-wide、portable zip、MSI、Store 与 ARM64。

安装器应检测/引导 Evergreen WebView2 Runtime。Windows 11 已包含 Evergreen Runtime，但微软仍建议安装或更新时检查；Evergreen 自动获得安全更新，比随包固定一个 250MB+ runtime 更适合日常应用。[WebView2 distribution](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution)

分发包和 executable 需要 Authenticode 签名，并在各版本保持一致 publisher identity。微软说明未签名或 self-signed 的公开分发会触发强 SmartScreen 阻止；有效签名仍可能在新 publisher 建立信誉前提示。[SmartScreen reputation](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation) 这与 Tauri updater artifact signature 是两件事，两者都要有。

### 更新

生产 updater 只接受 HTTPS release endpoint 与 Tauri 强制的 artifact signature；Tauri 的更新签名验证不能关闭。Windows 安装步骤会自动退出应用，因此 `on_before_exit` 必须完成 checkpoint，UI 要明确展示版本、release note 与“现在安装/稍后”。默认使用有进度反馈的 `passive`，不使用无法自行请求权限且无反馈的 `quiet`。[Tauri updater](https://v2.tauri.app/plugin/updater/)

### 恢复

恢复主路径是持续持久化，不是在 crash callback 里抢救：

- 每个 run/attempt/External Action 在执行前后写 durable state；
- startup reconciliation 区分“可安全重试”和“副作用结果未知”；
- 更新前建立数据库 checkpoint/backup，并在 migration 成功后才标记新 schema 健康；
- 连续启动失败进入不加载自动化/扩展的 safe mode，仍可导出用户数据、查看诊断和重装；
- 卸载/重装默认不删除 user data；删除数据必须单独明确确认。

Windows Application Recovery and Restart 可作为以后改善 crash UX 的附加能力，但不是最低要求。微软明确指出 installer 更新组件时 WER 不会调用 recovery callback，并建议应用周期性保存状态；这正是 durable task state 必须独立存在的原因。[RegisterApplicationRecoveryCallback](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-registerapplicationrecoverycallback)

## 与当前仓库的对齐情况

| 现有基础 | 结论 |
| --- | --- |
| `src-tauri/src/keychain.rs` + keyring-store | 保留 backend，补 opaque `CredentialRef`、可用性/错误语义，避免把插件的宽 IPC 作为产品接口 |
| tray + `auto-launch` + 本地 setting | 基础存在；需要固定 close-to-tray / explicit quit / recovery 语义，并以 OS 状态校准 setting |
| notification plugin + channel toggles | 基础存在；需要 installed-build 验收、activation route、suppressed/unavailable 结果和脱敏规则 |
| dialog + workspace paths | 原生选择存在；需要持久 FileGrant 与 reparse-aware 的每次使用校验 |
| optional updater + frontend check | 基础存在；生产配置仍需 endpoint/pubkey、双重签名、checkpoint 和 failed-start recovery |
| `DesktopAdapter` 的 launch/settings/draft | 与推荐边界一致；不要扩大为通用 shell、坐标输入或自动 submit |
| 右侧 iframe“browser” | 不是浏览器自动化；发行版仍需独立 automation profile 与受 lease 的结构化执行端口 |
| deep-link / single-instance / OAuth broker | 当前未形成完整边界，是 connected-service 首发前的必要缺口 |

## Release gate

只有以下场景在真实、已安装、非管理员 Windows 11 x64 环境通过，才可声称完成 Windows Daily-Use capability boundary：

1. 保存、重启读取、撤销模型 key 和 OAuth refresh token；SQLite/日志/导出中不存在 secret。
2. 用户开启登录启动后可在 Windows Startup Apps 中看到并关闭；窗口关闭到托盘，显式退出后不再执行后台任务。
3. 安装包通知可显示、可被 OS/用户禁用而不丢 durable attention；点击回到正确任务。
4. 用户选择一个文件夹后只能在 grant 范围内按批准模式操作；junction/symlink 越界被拒绝。
5. loopback OAuth 用系统浏览器完成 PKCE；伪造/过期/重放 callback 被拒绝；需要 custom scheme 的 provider 经安装后可回到同一实例。
6. 浏览器任务使用独立 profile；默认浏览器数据不可见。桌面 draft 不触碰密码控件、不跨 integrity boundary、不自动提交。
7. 签名 per-user installer 在干净系统安装；签名 updater 在 checkpoint 后退出并升级；强制终止或失败启动后可恢复任务并导出数据。

这组能力足以支撑用户已确认的“小到中等、边界清晰、有限并发”的个人助理体验；Windows Service、全系统 RPA、默认浏览器接管和跨平台 parity 都应留在 Daily-Use Release 之外。
