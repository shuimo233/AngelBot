# Windows 邮件与日历连接策略

> 调研日期：2026-09-17
>
> 范围：仅 Windows 桌面版；仅引用 RFC、Google、Microsoft 等一手资料。

## 结论

AngelBot 可以在**没有 AngelBot 用户账户、没有 AngelBot 后端、没有可保密客户端密钥**的前提下连接邮件和日历，但不能同时做到“没有提供商应用注册”。Google 与 Microsoft 都要求 OAuth 客户端先取得 `client_id`；桌面应用只是被正确地注册成不能保守秘密的 public client。[RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html) [Google 桌面 OAuth](https://developers.google.com/identity/protocols/oauth2/native-app) [Microsoft public client](https://learn.microsoft.com/en-us/entra/msal/msal-client-applications)

推荐采用混合方案：

1. Google 使用 Gmail API 与 Calendar API；Microsoft 使用 Microsoft Graph。两者都走 delegated user authorization，令牌只保存在本机。
2. 官方发行版内置由 AngelBot 维护者拥有的**公开 `client_id`**，不内置 `client_secret`；另提供 BYO client ID，供企业和高级用户使用。
3. 通用 IMAP + SMTP + CalDAV 作为其他提供商的兼容层，而不是 Gmail/Microsoft 的首选路径。
4. Windows 首版统一采用系统浏览器、授权码、PKCE S256、随机 loopback 端口；Microsoft WAM 可以后续作为体验增强，不作为第一版前置依赖。
5. 没有公网后端就不承诺实时推送。应用运行时使用本地轮询和增量同步；应用关闭时明确显示不会持续监控。

这一方向兼顾了普通用户的一键连接和开源/企业部署的自主性。它也承认一个无法用协议技巧消除的事实：公开发行仍需要维护者承担 Google OAuth verification、Microsoft publisher identity、配额和滥用处理责任。

## 1. “无账户、无 secret”真正允许什么

原生桌面应用可被反编译，因此 RFC 8252 将其定义为 public client：静态分发在安装包里的 secret 不能被视为机密；public native client 必须使用 PKCE。传统 Windows 桌面应用通常在系统浏览器完成登录，再由仅绑定 loopback 的临时 HTTP listener 接收授权码。[RFC 8252 §§6–8、附录 B.3](https://www.rfc-editor.org/rfc/rfc8252.html)

因此需要区分：

- `client_id` 是应用身份标识，可以公开并随源码/二进制分发。
- `client_secret` 是 confidential client 的凭据，不能放进 AngelBot 桌面程序；对 public client 应省略。
- access token 与 refresh token 是用户凭据，必须仅在本机安全存储，不能写日志、模型上下文或普通 SQLite 字段。
- Google Cloud project / Microsoft Entra app registration 仍然必须存在。它们不要求用户注册 AngelBot 账户，也不要求 AngelBot 运行登录服务器，但必须有维护者或部署组织作为应用发布方。

如果项目把“完全没有 AngelBot 持有的第三方应用注册”也列为硬约束，唯一可行方案是 BYO client ID。它适合企业部署与开发者模式，却不适合普通消费者：用户要自己创建 Cloud/Entra 项目、配置 consent screen、redirect URI 和权限，并自行承担审核与租户策略。

## 2. Windows 授权基线

### 推荐基线：系统浏览器 + loopback + PKCE

AngelBot 当前是 Tauri/Rust Windows 桌面应用，最小且跨提供商一致的基线是：

- 使用系统默认浏览器，不在 WebView2 中嵌入提供商登录页；Google 会拒绝不允许的 embedded user-agent。[Google 桌面 OAuth](https://developers.google.com/identity/protocols/oauth2/native-app)
- 每次授权生成新的 PKCE verifier/challenge，使用 S256，并用随机 `state` 绑定请求与回调。
- 临时监听 `127.0.0.1` 或 `[::1]` 的随机端口，只绑定 loopback，在收到回调后立即关闭。Google 明确推荐 Windows desktop 使用 loopback；Microsoft 的 system-browser desktop 配置使用 `http://localhost`。[Google 桌面 OAuth](https://developers.google.com/identity/protocols/oauth2/native-app) [Microsoft desktop app configuration](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-configuration)
- 不使用 implicit flow、用户名/密码收集或已停用的 OOB 手工复制授权码。[RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html) [Google 桌面 OAuth](https://developers.google.com/identity/protocols/oauth2/native-app)

Windows App SDK 已提供面向通用 OAuth 提供商的 `OAuth2Manager`，但截至本次调研它仍只在 Experimental release channel；不应成为首版生产依赖。其安全模型同样是系统浏览器、authorization code + PKCE、public client 不传 secret。[Windows OAuth2Manager](https://learn.microsoft.com/en-us/windows/apps/develop/security/oauth2)

### Microsoft WAM：后续增强

Microsoft 推荐 Windows 上使用 MSAL.NET + Web Account Manager (WAM)，可复用 Windows 已知账户、支持 Windows Hello 和 Conditional Access，并在不可用时回退浏览器。[MSAL.NET with WAM](https://learn.microsoft.com/en-us/entra/msal/dotnet/acquiring-tokens/desktop-mobile/wam)

WAM 的价值真实，但官方集成路径主要围绕 MSAL.NET，而 AngelBot 后端是 Rust/Tauri。第一版为 WAM 增加 .NET helper 或 WinRT interop 会引入新的 Windows 专用部署与 IPC 边界。建议先用标准浏览器 flow 验证产品价值，再把 WAM 作为 Microsoft 连接器的可替换认证前端；后台自动化仍必须在缓存令牌失效时转成“等待用户重新连接”，不能尝试无人值守地弹登录 UI。

### 本机令牌存储

refresh token 应进入现有 keyring/secret broker 抽象，底层使用 Windows 受保护存储；同步游标和非秘密账户元数据才进入 SQLite。Windows Credential Locker 可供桌面应用安全存取凭据，并明确反对把凭据以明文放进 app data；Microsoft 的桌面 token-cache 指南也给出了加密持久化和 DPAPI `CurrentUser` 的做法。[Windows Credential Locker](https://learn.microsoft.com/en-us/windows/apps/develop/security/credential-locker) [MSAL token-cache serialization](https://learn.microsoft.com/en-gb/entra/msal/dotnet/how-to/token-cache-serialization)

## 3. 开放标准能做什么，不能做什么

| 方案 | 能力与覆盖 | 优点 | 主要限制 |
| --- | --- | --- | --- |
| IMAP + SMTP | IMAP 读取/整理邮件，SMTP submission 发信 | 提供商覆盖广；可复用成熟库 | 两套连接与错误模型；文件夹、标签、线程语义不统一；OAuth scope 和 app registration 仍由提供商决定 |
| CalDAV + iCalendar | 读写日历对象；可加服务发现与 scheduling 扩展 | 标准化程度高，适合作为其他 CalDAV 提供商的连接层 | 服务器支持子集不同；发现、认证、共享日历与会议语义仍需兼容代码 |
| Gmail/Calendar API | Google 原生邮件标签、线程、草稿、发送、增量同步与日历能力 | 权限可比 IMAP 更窄，模型贴近 Google 产品 | Google project、审核、scope 政策与配额 |
| Microsoft Graph | Outlook/Microsoft 365 邮件和日历统一 API | 同一授权模型；personal 与 work/school 账号；细粒度 delegated permissions | Entra app registration、企业 consent policy、发布者信任 |

IMAP4rev2 要求安全传输，并引用 TLS 邮件访问建议；OAuth token 可以通过 SASL `OAUTHBEARER` 一类机制用于非 HTTP 邮件协议。[RFC 9051](https://www.rfc-editor.org/rfc/rfc9051.html) [RFC 8314](https://www.rfc-editor.org/rfc/rfc8314.html) [RFC 7628](https://www.rfc-editor.org/rfc/rfc7628.html) 但 SASL OAuth 只解决“如何把 token 交给 IMAP/SMTP server”，不解决“谁给桌面程序发 client ID、请求什么 scope、是否要审核”。后者仍然是提供商策略。

CalDAV 是基于 WebDAV 的日历访问标准，iCalendar 是交换格式；RFC 6764 定义发现，RFC 6638 补充 scheduling。[RFC 4791](https://www.rfc-editor.org/rfc/rfc4791.html) [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html) [RFC 6764](https://www.rfc-editor.org/rfc/rfc6764.html) [RFC 6638](https://www.rfc-editor.org/rfc/rfc6638.html) 但 Google CalDAV 仍要求 Google OAuth client，并只实现规范的一个子集，因此 CalDAV 不能绕过 Google 注册或合规要求。[Google CalDAV guide](https://developers.google.com/workspace/calendar/caldav/v2/guide)

## 4. Google 路径

### 身份与授权

Google 明确说明 installed app 不能保守秘密。Windows desktop 应选择 Desktop app OAuth client，使用系统浏览器、PKCE 与 loopback；token exchange 中 `client_secret` 是可选的。refresh token 由桌面应用安全持久化，从而在用户不在场时刷新 access token。[Google 桌面 OAuth](https://developers.google.com/identity/protocols/oauth2/native-app)

公共产品仍需要维护者拥有 Google Cloud project 与 consent screen。Testing 状态最多 100 名测试用户，测试授权通常七天过期；未验证 sensitive/restricted scopes 会显示警告并受项目生命周期 100 个新用户上限约束。[Google app audience](https://support.google.com/cloud/answer/15549945?hl=en) [无需验证的例外](https://support.google.com/cloud/answer/13464323?hl=en)

### 邮件：Gmail API 优先

Gmail 的标准协议支持 OAuth，但 IMAP/POP/SMTP 统一使用全邮箱 `https://mail.google.com/` restricted scope。[Gmail XOAUTH2](https://developers.google.com/workspace/gmail/imap/xoauth2-protocol) Google 的审核 FAQ 更明确：如果不需要绕过回收站永久删除，应迁移到更窄的 Gmail API scope；只为 SMTP 发信而请求 `mail.google.com` 违反最小权限要求。[Google OAuth verification FAQ](https://support.google.com/cloud/answer/13463817?hl=en)

建议按能力设计清晰的连接档位：

- 仅发送：`gmail.send`，属于 Sensitive。
- 阅读正文：`gmail.readonly`，属于 Restricted。
- 阅读、标记、移动/整理、草稿与发送：`gmail.modify`，属于 Restricted。
- 不请求 `https://mail.google.com/`，除非产品确实需要永久删除且能证明必要性。

上述分类来自 [Gmail scope reference](https://developers.google.com/workspace/gmail/api/auth/scopes)。Google installed app 不支持传统 incremental authorization，因此“仅发送”和“可读写邮箱”应被设计为用户可理解的连接档位；升级权限时重新连接，并检查实际获批 scopes，而不是悄悄扩大授权。[Google 桌面 OAuth](https://developers.google.com/identity/protocols/oauth2/native-app)

Gmail 是本方案最大的发布风险。Restricted scope 必须经过 restricted-scope verification；如果受限邮件数据被存到服务器或传输到服务器，还必须经过 security assessment。[Gmail scope reference](https://developers.google.com/workspace/gmail/api/auth/scopes) 对 AngelBot 而言，将邮件正文发送给远程模型 API 即属于“传输到服务器”是合理且保守的政策解释，即使 API key 是用户自己的、AngelBot 没有自建后端。产品不能用“本地桌面应用”描述掩盖这一数据流；在启用 Gmail 阅读/整理前，应完成政策与评估方案，并在 consent/privacy disclosure 中明确模型提供商会接收什么数据。

### 日历：Calendar API 优先

Calendar API 可按场景选择较窄 scope，例如 `calendar.events.readonly`、`calendar.events` 或只读 free/busy scope；公共应用访问用户数据仍可能需要 verification。[Google Calendar scopes](https://developers.google.com/workspace/calendar/api/auth) 对简单个人助理任务，Calendar API 比 CalDAV 更容易表达创建/修改事件、参会者和 Google 自身语义，CalDAV 保留为通用提供商适配器即可。

## 5. Microsoft 路径

### 一个 public-client registration 覆盖两类账号

Microsoft Entra app registration 可配置为同时接受任意组织目录以及 personal Microsoft accounts；桌面应用注册为 public client，不使用 secret。[Microsoft tenancy](https://learn.microsoft.com/en-us/entra/identity-platform/single-and-multi-tenant-apps) [Microsoft desktop app configuration](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-configuration)

邮件与日历首选 Microsoft Graph delegated permissions：

- 邮件列表但不含正文/附件：`Mail.ReadBasic`
- 读取正文：`Mail.Read`
- 创建、读取、修改、删除邮件：`Mail.ReadWrite`；它不包含发送
- 发送：`Mail.Send`
- 日历基础详情：`Calendars.ReadBasic`
- 完整读取：`Calendars.Read`
- 创建、修改、删除事件：`Calendars.ReadWrite`

Microsoft 的权限表把这些 delegated permissions 标为 `AdminConsentRequired: No`，并说明其可用于 personal Microsoft accounts。[Microsoft Graph permissions](https://learn.microsoft.com/en-us/graph/permissions-reference) 但这不保证每个企业用户都能自行同意：Entra tenant 可关闭 user consent，或只允许 verified publisher 和被归类为低影响的权限；其余情况要走管理员批准。[Microsoft user/admin consent](https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/user-admin-consent-overview)

因此公共多租户发行应设置 publisher domain，并争取 Publisher Verified。验证需要组织 Entra 账号、匹配域名及 Microsoft AI Cloud Partner Program 身份；它不是 public-client flow 的技术 secret，却会直接影响企业租户能否信任和批准应用。[Microsoft Publisher Verification](https://learn.microsoft.com/en-us/entra/identity-platform/mark-app-as-publisher-verified)

### 为什么不把 Microsoft 也默认放到 IMAP/SMTP

Microsoft 365 与 Outlook.com 确实支持 IMAP/POP/SMTP OAuth，delegated scopes 分别是 `IMAP.AccessAsUser.All`、`POP.AccessAsUser.All`、`SMTP.Send`，并可请求 `offline_access`。[Microsoft mail OAuth](https://learn.microsoft.com/en-us/exchange/client-developer/legacy-protocols/how-to-authenticate-an-imap-pop-smtp-application-by-using-oauth)

但 Microsoft 建议组织禁用 SMTP AUTH，安全默认值也可能已经将其关闭，且组织级和 mailbox 级都能单独禁用。[Exchange Online SMTP AUTH](https://learn.microsoft.com/en-us/exchange/clients-and-mobile-in-exchange-online/authenticated-client-smtp-submission) Graph `Mail.Send` 不依赖 SMTP AUTH，Graph 还统一覆盖邮件、日历、共享资源和细粒度权限，因此应作为 Microsoft 的默认连接器；标准协议只作为特定服务器或兼容场景的后备。

## 6. 没有 hosted backend 的同步边界

无后端意味着授权回调可以完全本地，但服务端 push 通常不可用：

- Gmail 的 push notifications 依赖 Google Cloud Pub/Sub 和接收端；Google 对 installed app / user-owned device 明确建议用轮询式同步。Gmail API 可保存 `historyId` 做 partial sync，游标失效时回退 full sync。[Gmail push](https://developers.google.com/workspace/gmail/api/guides/push) [Gmail sync](https://developers.google.com/workspace/gmail/api/guides/sync)
- Microsoft Graph webhook 要求公网可访问的 HTTPS endpoint，因此纯桌面程序不能直接可靠接收。[Graph webhooks](https://learn.microsoft.com/en-us/graph/change-notifications-delivery-webhooks) Graph 的 delta query 可用 `@odata.deltaLink` 维护本地增量状态，日历 `calendarView/delta` 已在 v1.0 提供。[Graph event delta](https://learn.microsoft.com/en-us/graph/api/event-delta?view=graph-rest-1.0)

推荐同步语义：应用启动/恢复时同步、用户显式刷新、受控的本地定时轮询；每个账户保存独立 sync cursor。不要声称“应用关闭后实时监听”。若以后增加云中继，那是新的产品与隐私决策，不应作为当前连接器的隐式依赖。

## 7. 决策选项

| 选项 | 普通用户体验 | 提供商独立性 | 安全/合规 | 结论 |
| --- | --- | --- | --- | --- |
| A. 只做 IMAP/SMTP/CalDAV | 中到差，配置项多 | 表面最高 | Gmail 仍需最宽 restricted scope；Microsoft SMTP 可能禁用；认证仍分叉 | 不推荐作为默认 |
| B. 只做 Gmail/Calendar API + Graph | 最好 | 最低 | scope 可最小化；发布方承担两套审核与配额 | 可做首版，但缺少其他提供商出口 |
| C. Provider API 为主，标准协议兜底 | 好 | 中高 | 需要清晰 adapter 边界和两套测试，但权限与兼容性最佳 | **推荐** |
| D. 完全 BYO client ID | 差 | 最高 | 责任由部署方承担；普通用户门槛过高 | 仅作为高级/企业模式 |

### 推荐的发布顺序

1. 先建立统一的 `ConnectedAccount` / capability adapter 边界与本机 public-client OAuth 基线。
2. Microsoft 先接 Graph；它可用一套 registration 覆盖个人与组织账号，delegated permission 边界清晰。
3. Google 先接 Calendar 和 Gmail send-only；只请求当前已实现功能需要的 scopes。
4. Gmail read/triage 在 restricted verification、远程模型数据传输和 security assessment 路径明确后再发布，不默认开启。
5. 再增加通用 IMAP/SMTP/CalDAV，优先支持 OAuth；app-specific password 仅作为用户主动选择的高级兼容方式，必须使用 TLS 并只存入系统凭据存储。
6. 官方 build 默认使用 AngelBot 公共 client IDs；设置页允许组织覆盖为自己的 Google/Entra registration。

## 8. 必须保持的产品与安全约束

- 连接时展示精确能力，不用笼统的“访问邮箱”；发送、读正文、整理邮件、写日历分别说明。
- 默认最小权限；用户尚未启用的能力不提前申请 scope。
- 所有外发邮件与新增/修改日历事件在提交前保留可见确认；读取和本地摘要可在已授权范围内自动执行。
- access/refresh token、OAuth code、PKCE verifier 不进入日志、SQLite、崩溃报告、模型 prompt 或子代理交付。
- 断开账户时清除本机 token/cache，并调用提供商 revocation endpoint（提供时）；保留的非秘密历史要让用户可选择删除。
- 对管理员策略、scope 被部分拒绝、token revoked、refresh 失败和 sync cursor 失效给出结构化状态，不把它们伪装成网络错误。
- Google restricted 邮件数据发送到远程模型前，必须完成相应政策、披露和评估判断；“用户自带模型 key”不能被当作绕过数据传输责任的依据。

## 最终建议

选择 **C：provider-native API 为主、开放标准兜底、public client + PKCE、本机令牌、无 hosted backend**。这不是 provider lock-in，而是把锁定限制在适配器内部：上层只看“列出/读取/整理/发送邮件”和“列出/创建/修改事件”等能力；Google 与 Microsoft 连接器负责各自语义，IMAP/SMTP/CalDAV 负责其他提供商。

首版不要把 WAM、实时 webhook、Gmail 全邮箱读取或长期无人值守监控设为上线前置。对“能处理一些较为简易的任务”的 AngelBot，可靠的显式连接、有限轮询、最小 scope 与发送前确认，比追求完整邮件客户端覆盖更符合产品边界。
