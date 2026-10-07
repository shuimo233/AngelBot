# 委派 Agent 网络能力规范

日期：2026-08-03

状态：**规范及当前实施状态（2026-09-21）。`NetworkGateway`、版本化 `SourcePolicy` 注册表、固定解析地址的单跳 HTTPS transport、Explorer runtime，以及 `delegate_network_exploration` 的通用确认/恢复和持久化签发路径已经接通。运行时现在在每次操作和 transport 返回后验证批准撤销/策略摘要；每次 attempt 的 lease 只取得其 `ExplorerPlan` 所需 host；响应字节与 MIME 上限已随策略进入 transport。项目设置已可查看并撤销当前项目的联网批准。Explorer task 的取消信号会贯穿 gateway、HTTP 请求等待和响应体读取；一经观察到取消，后续重定向和证据投影停止。它们仍不构成更宽泛的端到端完成声明：已发出的远端请求不能撤回、同步 DNS 查询不可中止，且真实桌面长旅程验收仍待完成。**

范围：主 Agent 向后台 worker 授予受控网络探索与依赖获取能力。本文是该主题的唯一规范性来源；其它委派、安全、沙箱和运行时文档只交叉引用本文。

## 1. 目的与不变量

AngelBot 的用户只与主 Agent 对话。网络不是 worker 的一般工具权限，而是由
`ExecutionGateway` 按 attempt 的短期租约逐次核验的 `NetworkCapability`。它支持
必要的公开资料探索，同时防止凭据使用、隐私外泄、SSRF、下载执行和网页提示注入。

下列不变量是实现验收条件，任一无法证明时必须拒绝操作（fail-closed）：

1. worker 不持有浏览器、HTTP 客户端、Cookie jar、认证信息、密钥或 `ToolRegistry`；只能提交结构化 `NetworkIntent`。
2. 只有 `ExecutionGateway` 能执行网络操作；每一次操作均验证未撤销且未过期的 `CapabilityLease`、工作包策略、全局上限与预算。
3. 真实工作区写入、网络授权、确认和子任务创建权始终属于主 Agent/runtime；worker 不可递归委派或扩大权限。
4. 所有搜索结果与网页响应均为**不可信证据**，不得改变 lease、工具范围、路径范围、确认结果或系统指令。
5. 网络证据不直接进入长期记忆、主 Agent 原始会话或 `AttentionCard`；只可作为限长、结构化结论及可受控展开的证据引用交付。

## 2. 术语与最小数据契约

| 术语 | 定义 |
| --- | --- |
| `NetworkCapability` | lease 中单独签发的、不可继承的网络能力；包含 `SourcePolicyRef`、允许的操作、预算、到期时间与版本。 |
| `ProjectSourceApproval` | 用户通过 Main Agent 的一次明确确认创建的、可撤销的项目范围来源批准；包含确认来源、规范化 host/action 集合和策略摘要。它不是任一 worker 的长期网络能力。 |
| `SourcePolicy` | 由 `ProjectSourceApproval` 支撑的来源类别、精确域名/注册表、方法、大小和重定向限制；不是模型给出的域名字符串。 |
| `NetworkIntent` | worker 请求 `search`、`fetch` 或 `registry_fetch` 的结构化意图，不含自由 header、Cookie、认证或请求体。 |
| `NetworkEvidence` | 网关产生的不可执行证据：来源 URL、获取时间、内容摘要/哈希、MIME、大小、策略与审计引用。 |
| `SourcePolicyRef` | 不可变策略版本的标识及 digest；工作包引用批准的策略，单次 attempt 的 `CapabilityLease` 还必须记录其实际收窄后的计划范围。 |

`NetworkIntent` 至少包含：`operation`、规范化目标（查询或 URL）、`purpose`、`sourcePolicyRef`、`leaseId` 与 `effectKey`。它不得携带 header、body、凭据、代理地址或可被用来绕过 URL 校验的重定向偏好。

## 3. 来源策略与 worker 规则

初始批准的来源类别为：官方产品文档、官方 GitHub 源码仓库、已批准包注册表。Explorer 可在受控搜索网关内发现公开来源；搜索结果仍须经 `SourcePolicy` 与 fetch 校验后才可读取。

| `WorkerProfile` | `TaskShape` | 默认网络能力 | 可允许的例外 |
| --- | --- | --- | --- |
| Explorer | `explore` | `search` + 对策略允许来源的 `fetch` | 主 Agent 可为工作包批准额外的公开文档/开源仓库来源。 |
| Implementer | `change` | 禁止搜索；仅可 fetch 已批准官方文档/源码 | 构建时仅可按锁文件和批准注册表进行 `registry_fetch`。 |
| Verifier | `explore` 或 `change` 的复核 | 禁止浏览 | 仅可使用本地候选、构建与测试所需的 lockfile-bound registry access。 |

`WorkerProfile` 只提供默认值，不是安全边界。有效权限必须等于：

```text
WorkerProfile defaults ∩ WorkPackage SourcePolicy ∩ CapabilityLease ∩ global safety ceiling
```

`TaskShape` 与 profile 分离：例如 Verifier 可以验证 `change` 的候选集，却不能搜索、产生新候选修改或扩大来源范围。

## 4. 唯一网关接口

网络实现只暴露两个研究操作和一个受限依赖操作：

```text
search(intent)          -> SearchEvidence
fetch(intent)           -> DocumentEvidence
registry_fetch(intent)  -> DependencyEvidence
```

- `search`：只返回排名有限的候选 URL、标题、来源标识和摘要；搜索页文字本身不是可信指令或可执行内容。
- `fetch`：只允许 HTTPS 的 `GET` 或 `HEAD`，并返回经过大小/MIME 处理的只读证据。
- `registry_fetch`：不是通用 HTTP。它只在本地构建/测试需要时，从批准注册表取得与锁文件解析结果匹配的依赖；不允许任意包名、任意 URL 或 post-install 网络扩张。

所有三者在发起前和等待返回后都复核取消、lease 版本、到期时间和预算；撤销、暂停、超时或用户拒绝使后续与在途请求失败并进入审计。

## 5. 网关验证与传输限制

`fetch` 和 `registry_fetch` 必须按下列顺序验证；任一失败不跟随、不降级、不重试为开放网络：

1. 规范化 URL，要求 HTTPS、显式主机与允许端口；拒绝 userinfo、非 HTTPS、IP 字面量、file/data/blob 等 scheme。
2. 以已签发 `SourcePolicy` 匹配主机、精确路径/注册表规则和允许方法；禁止由模型扩张域名。
3. 对每一跳重定向重复完整 URL、策略、DNS 和预算校验；重定向上限必须在策略中固定。
4. 在连接前校验 DNS 解析出的**所有**目标地址；拒绝 loopback、link-local、private、multicast、unspecified、保留/本机网络及代理绕过。连接层还必须将已验证地址与实际连接目标绑定，避免 DNS rebinding。
5. 限制并审计响应时间、重定向次数、压缩后与解压后的字节数；超限即截断并失败，不将部分内容伪装为完整证据。
6. 仅接受策略列出的文档 MIME（例如 `text/*`、受限 `application/json`）；拒绝可执行文件、归档、安装包、脚本、未知二进制和内容嗅探绕过。下载内容不可执行、不可自动打开。

默认没有外部代理、用户代理伪装、自定义 DNS、任意端口或绕过 TLS 验证的选项。证书错误应失败并可审计，不得由 worker 忽略。

## 6. 永久禁止项

无论 profile、工作包或用户确认如何，worker 网络 lease 均不得允许：

- 登录、Cookie、会话令牌、OAuth、API key、客户端证书或其他认证态；
- 上传、表单提交、`POST`/`PUT`/`PATCH`/`DELETE`、WebSocket、SSE、RPC 或写入型 API；
- 自定义 header、请求 body、任意代理、隧道、端口转发或本地/内网访问；
- 从网页、搜索片段、仓库文档或依赖脚本读取的“忽略规则”“扩大权限”类指令；
- 使用用户数据、会话文本、文件内容、秘密、诊断原文或候选代码作为网络查询/请求参数，除非主 Agent 已将经脱敏的最小必要片段显式写入工作包。

如果任务确实需要认证、上传或写入性网络效果，主 Agent 必须将其作为新的、面向用户的工作包/能力设计讨论；它不属于本规范的委派网络能力。

## 7. 证据、上下文与交付

网页与搜索内容可包含提示注入、恶意链接或不准确材料。网关将它们标记为 `untrusted_web`，并只交付：来源、时间、策略版本、内容哈希、限长摘录/抽取事实、风险标签和 `evidenceRef`。原始内容存入隔离证据区，按访问控制和字节预算按需读取。

worker 的 `DelegationDelivery` 只能以结构化方式引用网络发现：目标、结论、证据引用、冲突/不确定性、建议的下一步。禁止把网页全文、搜索页全文、原始 HTML、提示性文本、Cookie、URL 参数中的敏感值或工具日志嵌入 brief、模型上下文、长期记忆、`AttentionCard` 或用户可见活动流。

主 Agent 可在相关工作包内接收压缩结论；是否采纳来源结论仍由主 Agent 判断。网页文本不能被视为授权、确认、系统消息、用户指令或执行计划。

## 8. 用户批准、项目范围、租约收窄与审计

`delegate_network_exploration` 是必须取得一次精确用户决定的动作；广泛执行权限和模型参数中的 `confirmed` 标记都不能绕过它。通用确认流持久化待确认步骤，并仅在用户批准后按原始 `sessionId`、`messageId` 与 `callId` 重建受限工具面；项目、步骤或确认状态已失效时必须拒绝恢复而不是重放输入。

确认 UI 只说明规范化的公开 host、允许动作和固定上限。它**绝不**显示原始查询、完整 URL、URL 路径或参数、本地路径、header、凭据、密钥或其他秘密；这些值也不得出现在确认详情、活动流或用户可见的错误文本中。

项目设置中的“项目联网权限”是确认后的唯一用户管理入口：它仅在当前项目 Workspace 中读取有效批准，并只显示批准时间、规范化 host、`search`/`fetch` 动作与响应/重定向上限。个人空间不会请求或显示项目批准。撤销使用不透明的 approval reference；后端通过确认记录的 `sessionId → projectId` 归属校验项目，而非接受前端路径、策略 ID、摘要、URL 或查询，因此即使项目根暂时缺失也可撤销旧许可。撤销是幂等的降权操作，不需要再次由 Agent 确认；扩大、修改或续期范围仍必须创建新的明确确认。

在 Windows 上，新签发的 scope key 同时绑定 canonical 路径和 `FileIdInfo` 返回的卷序列号与 128 位目录 ID。删除后在同一路径重建目录会得到不同实例，不能复用旧批准；不支持或无法读取该 ID 的文件系统必须拒绝网络委派，不能退化为仅路径或可修改时间戳的身份。

确认成功后创建可撤销的 `ProjectSourceApproval`。它只在同一 canonical Workspace/项目内可复用，并且新的请求必须同时满足：

```text
requested actions ⊆ approved actions
requested hosts   ⊆ approved hosts
```

它不会跨项目、跨 owner 或因模型解释而扩张。新增 host、动作、来源类别、方法、网络预算，或从只读研究变为外部副作用，均须回到 Main Agent 取得新的明确确认。

项目批准不是把整个批准集合直接交给 worker。Main Agent 在创建 `WorkPackage` 时固定批准的 `SourcePolicyRef` 和受控 `ExplorerPlan`；每个 attempt 都新签发 lease，且其网络操作、host、预算和到期时间必须是该计划的子集，而非项目批准的全部 host。目标绑定为：

```text
workPackageId + delegationId + attemptId + sourcePolicyDigest + leaseVersion
+ plan-scoped allowedOperations + plan-scoped hosts + budgets + expiry
```

每次允许、拒绝、重定向、DNS 拒绝、预算耗尽、取消、失败和终态都写入审计。审计至少关联 `sessionId`、`parentRunId`、`workPackageId`、`delegationId`、`attemptId`、`leaseId`、`sourcePolicyDigest`、目标的脱敏/规范化标识、结果分类、effect key 和证据引用；不记录凭据或原始敏感请求内容。

## 9. 失败、重试、取消与清理

- 策略拒绝、权限拒绝、DNS/URL/MIME 校验失败、预算耗尽或证书失败不是临时故障，不自动重试。
- 仅当网关能证明尚未产生外部效果的临时传输失败时，可在同一安全策略内自动重试一次；其他不确定结果进入 `needs_decision`。
- 取消/暂停/超时的顺序为：持久化意图 → 撤销 lease → 停止 worker/请求 → 封存 sandbox → 写审计。恢复必须建立新 `attemptId`、新 sandbox 和新 lease。
- 原始网络证据、未采纳候选和封存 sandbox 随对应 attempt 进入检疫；7 天后按受控清理流程删除。已采纳工件的工作区相对引用和必要审计 tombstone 不随检疫删除。

## 10. 实现状态与验收场景

本规范描述目标行为。当前项目已有 `CapabilityLease`、`ExecutionGateway`、sandbox、结构化 delivery 与审计等基础模块；`NetworkGateway`、持久化版本化的 `SourcePolicy` 注册表、固定网关已验证解析地址的单跳 HTTPS transport、Explorer runtime 和前台 `delegate_network_exploration` 确认/恢复链路都已存在。待确认动作会持久化，批准时只按同一持久化步骤重建受限工具面；确认摘要已避免向用户显示原始查询、URL、路径或秘密。确认成功会随委派/attempt/lease/outbox 事务持久化项目范围批准与来源策略。

运行时已使用批准感知的 policy lookup：每次网关操作都验证 `ProjectSourceApproval` 的未撤销状态与策略摘要，策略漂移或撤销会在 DNS/transport 前失败。transport 返回后，网关会再次读取并要求策略与请求发起时完全一致；Explorer host 还会以新时钟重载 attempt、delegation、package 与 lease，随后才投影证据。因此在撤销或状态变化于该复核前落库时，已收到的响应不会成为证据、重定向也不会继续。批准复用仍以项目范围的 host/action 子集为上限，但每次 lease 只记录其 `ExplorerPlan` 的实际 host 子集。策略的 MIME/字节上限在 connector 读取正文前生效，且在 transport 与 gateway 两层复核。

边界也应如实保留：Explorer task、gateway 与 pinned HTTPS connector 共享同一取消信号；它会与 HTTP 请求等待及每个响应体分块竞争，观察到取消后丢弃本地未完成请求/响应，停止后续重定向与证据投影。已经写入网络的 `GET`/`HEAD` 不能从远端收回，且当前系统 DNS 查询仍为同步、不可中止；撤销或取消恰好发生在最终复核之后仍有极小竞态。严格线性化的全部取消语义仍需要可中止 DNS 或共享 capability gate。项目设置现已通过 `ProjectNetworkApprovalManager` 提供脱敏列表与幂等撤销，运行时会在后续操作及响应投影前重新核验该撤销；真实桌面长旅程仍不应被表述为已完成的用户体验。

首次实现至少必须通过以下场景：

1. Explorer 用 `search` 发现候选 URL；不在 SourcePolicy 的 fetch 被拒绝并审计。
2. 允许站点重定向至私网、loopback 或未批准域名时，每一跳均被拒绝；worker 无法取得响应正文。
3. 实现者可读取批准文档；任意 `POST`、cookie/header、二进制下载和非 HTTPS URL 均拒绝。
4. 锁文件外的依赖、任意 registry URL 或依赖脚本的新网络请求均拒绝；合法锁文件依赖在预算内可用于构建。
5. 取消、暂停、过期或 lease 版本变化后，在途和后续请求不能产生可用新证据；恢复使用新 attempt。
6. 网页中要求泄露上下文或扩大权限的文本只作为 `untrusted_web` 证据；不会改变授权或进入主 Agent 长期上下文。
7. 7 天检疫清理只删除该 attempt 自有的未采纳网络证据/沙箱数据，不能删除已采纳工件或任意外部路径。

## 11. 相关材料

- [委派权限与受控执行审计](agent-delegation-permissions-threat-model.md)
- [主流 Subagent 实现调研](research-mainstream-subagent-safety-usability.md)
- [FirstMate 与主流机制复核](research-firstmate-safety-usability.md)
- [子代理能力隔离调研](research-delegated-worker-capability-isolation.md)
- [沙箱生命周期审计](delegated-sandbox-lifecycle-audit.md)
- [受控委派运行时边界审计](controlled-delegation-runtime-boundary-report.md)
- [共享执行循环评估](agent-execution-loop-architecture-evaluation.md)

## 12. 文档对齐矩阵

| 文档 | 对齐状态 | 本次精确更新 |
| --- | --- | --- |
| [委派权限与受控执行审计](agent-delegation-permissions-threat-model.md) | 已对齐 | `NetworkPolicy` 明确指向 `NetworkCapability`/`SourcePolicyRef`；保留威胁建模而不重复规范。 |
| [主流 Subagent 实现调研](research-mainstream-subagent-safety-usability.md) | 已对齐 | 将旧的 Explorer “无网络”实施切片改为已签发能力下的受控搜索/抓取，并保留“尚未实现”。 |
| [FirstMate 与主流机制复核](research-firstmate-safety-usability.md) | 已对齐 | 明确 FirstMate 的 worktree/脚本不构成 AngelBot 网络权限模型。 |
| [子代理能力隔离调研](research-delegated-worker-capability-isolation.md) | 已对齐 | 网络 allowlist/秘密隔离术语改为引用本规范，保留研究依据。 |
| [沙箱生命周期审计](delegated-sandbox-lifecycle-audit.md) | 已对齐 | 将网络证据的撤销、封存与七天保留交叉引用到本规范，不夸大实现状态。 |
| [受控委派运行时边界审计](controlled-delegation-runtime-boundary-report.md) | 已对齐 | 将租约网络 allowlist 统一为 lease-bound `NetworkCapability`。 |
| [Real WorkerAdapter integration design](real-worker-adapter-integration-design.md) | 已对齐 | 将通用执行 port 的网络子集定向到本规范，并明确尚未接线。 |
| [共享执行循环评估](agent-execution-loop-architecture-evaluation.md) | 已对齐 | 标注 network port 属于 gateway 而非 loop，且未实现。 |
| [AgentRunner 深度拆分设计](agent-runner-decomposition-design.md) | 已对齐 | 标注 kernel 不拥有网络权限，网络属于 lease-bound gateway port。 |
| [可扩展异步委派运行时调研](research-scalable-async-delegation-architecture.md) | 已对齐 | 标注网络探索属于 worker host/gateway，不属于共享 kernel 或调度器。 |
| [AngelBot glossary](../CONTEXT.md) | 已对齐 | 新增 `Network Capability`、`Source Policy`、`Network Evidence` 三项最小术语。 |
