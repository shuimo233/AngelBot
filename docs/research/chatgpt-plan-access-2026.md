# AngelBot 接入已有 ChatGPT 套餐：官方资料研究

核验日期：2026-10-04。研究已用于本轮运行时实现；第 12 节记录实施范围与验收，真实账号授权和套餐推理仍未验收。

资料边界：先搜索官方主题，再实际读取官方页面；仅使用 `developers.openai.com`、`platform.openai.com`、`learn.chatgpt.com`。长文索引抓取返回 HTTP 403 后，改用网页读取接口读取正文。没有读取真实 `auth.json`、令牌、环境秘密或用户运行数据；没有发起 OAuth 注册、登录、刷新、撤销或模型推理。

## 1. 结论与适用范围

**官方已证实：存在不要求用户单独配置 OpenAI API Key 的直接接入方案。** 2026-09-28 官方发布的开源应用教程说明：符合资格的 ChatGPT Plus/Pro 用户，可授权本地应用消耗自己的套餐或可用 credits；适用对象包括开源工具、本地运行的个人项目和部分获准私有应用。[官方发布教程](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt)

这不是“只能让 Codex CLI 代跑”的方案。OSS 动态注册取得 OAuth 凭据后，可以直接访问公开的 Responses API；不需要 client secret 或 partner API key。接入须申请独立的套餐使用权限，身份登录本身不授权推理，也不开放 ChatGPT 对话、记忆或用户 API Key。[Quickstart](https://developers.openai.com/siwc/quickstart)、[注册与登录](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)

**工程推论：可以作为 AngelBot Main 和子代理共用的模型提供方，但不是全部功能无条件兼容。** 官方明确支持本地函数/自定义工具，以及 Codex 的本地工具与子代理编排；这并不意味着 Responses 顶层 `multi_agent` 功能或全部托管工具可用。AngelBot 自己编排多个推理请求时，每个请求仍须分别满足相同的账号、模型、参数和工具限制。[预览限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

**待确认：AngelBot 的发行资格。** 付费或由开发者远程托管、对外提供的产品，不能据本地 OSS 文档直接认定获准，应先申请对应访问资格。自托管 VM 有单独说明，不等同于任意商业 SaaS。[概览](https://developers.openai.com/siwc/token-sharing-open-source)、[官方发布教程的政策说明](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt)

## 2. 用户资格、额度、费用和地区

| 项目 | 官方已证实 | 不能据此推定 |
| --- | --- | --- |
| 套餐 | 文档明确写 eligible ChatGPT Plus/Pro。[Quickstart](https://developers.openai.com/siwc/quickstart) | Free、Go、Business、Enterprise、Edu 或所有工作区都能用此直接路径；拥有 Plus/Pro 也不证明某次请求符合全部政策。 |
| 额度来源 | 使用已有 Codex / ChatGPT Work 套餐使用量；连接应用不会新增额度。[用户说明](https://learn.chatgpt.com/docs/sign-in-with-chatgpt) | “第三方应用免费无限调用”或与 Codex/Work 完全独立的额度池。 |
| Plus 五小时窗口 | 所有使用套餐的私有/开源应用共享五小时总量；Pro 不适用这个五小时限制。[账号与会话](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions) | Pro 不存在其他配额、应用限额或政策限制。 |
| 应用限额 | 用户可在 Settings → Usage 设置应用的每周百分比上限；这是上限，不是新增或预留额度。[用户说明](https://learn.chatgpt.com/docs/sign-in-with-chatgpt) | 应用在某次 429 后能自行算出剩余额度、耗尽范围或准确重置时间。 |
| 额外费用 | 用户可以控制达到限额后是否允许其他应用消耗 credits；第三方应用可能另收费。[用户说明](https://learn.chatgpt.com/docs/sign-in-with-chatgpt) | 没有 API Key 就绝无费用；本文未证实该路径专属美元单价、精确 token 配额或 credits 消耗计算。 |
| 错误时计费路径 | 套餐错误会停止推理，OpenAI 不会默默切换其他计费路径。[错误与恢复](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery) | AngelBot 可以未经用户选择自动改用 API Key、credits 或其他付费渠道。 |
| 地区 | 直接接入可能因 permitted serving region 等政策返回 403。[错误与恢复](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery) | 登录成功就表示当前地区可执行推理。 |

**地区待确认：** 本次读取的 SIWC 文档没有专属完整地区列表。普通 OpenAI API 的官方支持名单不列中国大陆，并警告在名单外访问/提供访问可能导致封禁或停用；不能把套餐接入解释为绕过地区限制，也不能直接把普通 API 名单当成 SIWC 全部资格的证明。[API 支持地区](https://developers.openai.com/api/docs/supported-countries)

这里不从用户语言、时区或仓库路径推断实际所在地或账号资格；实施时按官方服务资格与实际返回的政策检查处理。

## 3. 模型与每次推理请求的合同

### 3.1 动态模型发现

用当前选中账号的同一 access token 调用 `GET https://api.openai.com/v1/models`。这里返回 `models` 数组；筛选 `visibility == "list"`，保留服务端顺序，界面显示 `display_name`，推理使用 `slug`。切换账号后刷新目录，不把普通 API 模型清单、Codex 缓存清单或示例中的 `gpt-6.1-sol` 当成所有用户的固定允许列表。[模型与推理](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)

**未知：** 没有真实登录或请求，因此没有证实某位用户能使用哪个具体模型。模型目录本身也不代替最终推理资格检查；官方 Codex 说明以成功完成的实际请求验证该次模型访问。[Codex app-server](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)

### 3.2 HTTP/SSE 请求限制

官方直连入口是 `POST https://api.openai.com/v1/responses`，凭据为 OAuth access token 的 `Authorization: Bearer ...`；不要改用 ChatGPT `backend-api`。[模型与推理](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)

| 请求部分 | 官方当前要求 |
| --- | --- |
| 传输与存储 | `store: false`、`stream: true`。 |
| 上下文 | `input` 必须是包含本次必要历史的数组；通过 `instructions` 或 developer message 传递行为指令；显式 `{type:"message", role:"system"}` 被拒绝。 |
| HTTP 会话 | 不传 `previous_response_id`；客户端自行带齐历史。WebSocket 仅能延续同一认证连接上的响应，不提供持久会话存储。 |
| 工具 | function/custom 工具应放在 namespaces 中，或由 `additional_tools` input items 提供；web search 仍受模型与账号/工作区政策限制。 |
| 输入 | 所选模型接受时可用文字、图片、文件；不支持音频/视频输入、Files 上传 API、transcription API。 |

表中限制均来自[预览限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)。

必须省略的字段：`background`、`conversation`、`max_output_tokens`、`max_tool_calls`、`metadata`、`moderation`、`multi_agent`、`prompt`、`prompt_cache_retention`、`safety_identifier`、`temperature`、`top_logprobs`、`top_p`、`truncation`、`user`。[预览限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

不支持的托管能力：image generation、file search、Code Interpreter、native computer use、hosted MCP/connectors、Responses `tool_search`；改为客户端执行也不会让 `tool_search` 获得支持；顶层 `tools` 不接受 `programmatic_tool_calling`。[预览限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

**工程建议：** AngelBot 的浏览器、文件、终端等本地执行功能应通过受支持的函数/自定义工具表示，且继续由 AngelBot 自己做权限检查。不要把“本地工具能用”混同为对应 OpenAI 托管工具能用；也不要把 `multi_agent` 字段禁用误读为禁止客户端编排 Main/子代理。

### 3.3 成功与失败的终态

只有收到 `response.completed` 才能把推理标为完成。流开始后仍可能收到 `response.failed`，包括额度超限或额度暂不可查；`response.incomplete`、显式错误、连接中断和没有终态的 EOF 须分别处理。Connected 状态、有模型目录或收到若干文字 delta 都不足以证明成功。[模型与推理](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)

### 3.4 无服务器存储时，工具循环必须保留完整 Responses items

**官方已证实：** 手动管理 stateless reasoning context 时须保存并回放 `response.output` 的每个 item，而不只是 `output_text`。完整回放保留 encrypted reasoning 和 assistant `phase`。[Conversation state](https://developers.openai.com/api/docs/guides/conversation-state)

截至核验日，通用 Responses 文档说明 `store:false` 下 reasoning items 默认返回 `encrypted_content`；旧 `include:["reasoning.encrypted_content"]` 仍兼容，但不再是获得该内容的必要条件。该字段是供后续请求回放的 opaque 加密状态，不是可展示的原始思维链。[Reasoning models](https://developers.openai.com/api/docs/guides/reasoning)

工具调用回合中，必须将返回的 reasoning items 与 tool-call outputs 一起传回。`function_call_output.call_id` 对应原 `function_call.call_id`，不能把 response/item 的 `id` 当成这个关联 ID；保留完整工具调用项、参数、相关 namespace 信息以及执行结果。[Function calling](https://developers.openai.com/api/docs/guides/function-calling)

**工程建议：** 为每个 Main/子代理保留独立的 Responses item ledger，并在统一 adapter 维护回放与序列化，流程至少为：

1. 用当前 ledger 作为数组 `input` 发请求，消费完整流，取得完成响应的所有 output items。
2. 把 output items 转为可回放 input items 并按原顺序追加，保留 `reasoning.encrypted_content`、message `phase`、function/custom-call ID 与参数等状态；不要仅从文字 delta 重建 history。
3. 在原有执行权限检查通过后执行工具，追加匹配 `call_id` 的 `function_call_output`（自定义工具使用对应 output item）。再次发送完整必要上下文，直到完成该工具循环。

旧 Chat Completions 的 `assistant content + tool_calls + role:tool` 历史不等价于这个 ledger。只做消息外形转换会丢掉 Responses opaque state；把 reasoning/function-call/function-call-output items 在手动续轮时丢掉，也是官方迁移说明明确列出的常见错误。[Migration guide](https://developers.openai.com/api/docs/guides/migrate-to-responses)

**边界与待验：** 上述通用协议指导不能覆盖 SIWC 的特有限制，仍不传 HTTP `previous_response_id`、`conversation` 或 `store:true`。通用文档的 `reasoning.context:"all_turns"` 只适用于支持持久 reasoning 的模型，不据此保证某个套餐模型接受该选项；不要为上线默认开启。实际模型/工具形状、SSE item 组装和 opaque 状态回放须用 fixtures 与经用户授权的真实小请求分别验收。[预览限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)、[Reasoning models](https://developers.openai.com/api/docs/guides/reasoning)

## 4. 动态注册与 OAuth/OIDC 安全合同

以下是官方规则的实施核对表，不是已经运行过的登录过程。

1. **准备 host。** 每个运行宿主持久化独立、opaque 的 `ext_agent_host_id`，不能用邮箱/用户 ID。重启、重登和账号切换不创建新 host；host ID 不是凭据。可用 `urn:uuid:<UUIDv4>`；官方优先推荐公钥 JWK thumbprint URI，也接受 `did:key`，但当前不会验证私钥持有证明。[概览](https://developers.openai.com/siwc/token-sharing-open-source)
2. **准备登录事务。** 先监听 `127.0.0.1` HTTP loopback，再生成每次全新的 `state`、OIDC `nonce`、PKCE verifier 和 `S256` challenge。回调只可在后续登录变更端口，scheme/host/path 不变；一次授权和 code exchange 必须使用完全相同 URI，不能替换为 `localhost`。[注册与登录](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
3. **打开系统浏览器。** 首次使用 `client_id=dynamic_agent_client`、实际应用名 `agent_name_hint=AngelBot`、host ID；重授权用该账号已签发的 client ID，省略 `agent_name_hint`。请求身份 scopes `openid profile email`，另请求 `offline_access resource.invoke chatgpt.tokens.use.direct`，`resource=https://api.openai.com/v1`，`response_type=code`。[注册与登录](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
4. **处理回调。** 先验证 state 与错误，再使用 code。首次回调必须有已签发的 `client_id`；`dynamic_agent_client` 不能用于换 token 或保存为已签发 ID。返回登录若提供不同 client ID 应拒绝，不能替换已选注册。[注册与登录](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
5. **换取 token。** 对 token endpoint 发 form-urlencoded `authorization_code`，携带已签发 client ID、code、verifier、原 URI 和 resource；不发 secret。code 的 `invalid_grant` 应丢弃 code 并重启授权，保留该次已签发 ID。[注册与登录](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
6. **验证身份而非只 decode JWT。** 使用 OpenAI JWKS 验签，检查 issuer、audience（已签发 client ID）、expiry 和原 nonce；用已验证 `sub`。返回登录先确认身份与选中注册一致，再替换凭据。[注册与登录](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
7. **单独验证套餐权限。** 以 token response 的实际 granted scopes 为准，检查 `chatgpt.tokens.use.direct`，不是回调的显示值或请求值。有合法 ID token 而没有该权限时保留身份登录，标记套餐不可用，不发推理。[错误与恢复](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery)
8. **保存账号注册。** 每条 profile 把 issuer、subject、issued client ID、tokens、scopes、expiry 放在一起；email/sub 不代表 workspace。应用注册绑定已选用户和 workspace；同一注册可有多个 host，但各 host ID 不同。[概览](https://developers.openai.com/siwc/token-sharing-open-source)、[账号与会话](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)

公开 OIDC discovery 是 `https://auth.openai.com/.well-known/openid-configuration`。读取其中 issuer、authorization/token/JWKS 地址；官方当前示例为 issuer `https://auth.openai.com`、authorize `/api/accounts/authorize`、token `/api/accounts/oauth/token`、JWKS `/.well-known/jwks.json`。维护好一次性事务过期/消费、JWT 时钟容差和 JWKS 轮换；不能把网站 confidential-client 示例的 secret 要求套进 OSS public-client 路径。[身份验证参考](https://developers.openai.com/siwc/website)

**工程建议：** 登录事务应短时、一次性、与用户显式启动的操作绑定；取消/错误/成功都释放 listener 和事务。新登录在身份验证完成前不能替换 active profile。为 callback mismatch、重放、nonce mismatch、错误 issuer/audience/signature 等建立确定性负向测试。

## 5. 刷新、撤销和 Windows 凭据保存

官方 access token 有效期一小时；refresh token 有效期 30 天，每次成功刷新返回新的 token 和新的 30 天期限，持续有效的逐次刷新没有固定总次数限制。响应含 `expires_in`、`scope`、`earliest_refresh_at` 等字段；不要从 access token payload 寻找 refresh token，也不要解析 OpenAI opaque auth metadata 自行推导权限。[Token reference](https://developers.openai.com/siwc/token-sharing-open-source/token-reference)

| 场景 | 官方合同 |
| --- | --- |
| 刷新 | token endpoint 的 form-urlencoded `grant_type=refresh_token`；使用该 token set 对应 issued client ID、refresh token 和 resource；省略 scope 保留原 grant。相同 session 的刷新必须串行，避免 rotating token 竞争。 |
| 更新 | 原子替换 access token、expiry、granted scopes、replacement refresh token；不能混合两个注册的 ID/token。 |
| 登出 | 先停止请求；从 discovery 获取 `revocation_endpoint`，POST `token=<refresh_token>`、`token_type_hint=refresh_token`、issued client ID；空 HTTP 200（含已无效 token）是成功，再清理本地 tokens。 |
| 撤销未确认 | 网络/5xx 可有限退避；最终本地登出应清掉 tokens，并明确远程撤销未确认。保留注册映射与 host ID。撤销 session 不删除 registered client。 |
| 账号切换 | 验证选中 profile 后才激活，展示当前账号；不必登出其他已保存账号。 |

本表来自[账号与会话](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)；原子更新的保存合同见[注册与登录](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)。

官方要求 credentials 留在受保护的本地/自托管运行时存储；不进入 browser storage、源码、日志、analytics、支持记录。access/refresh token 不进入 URL；仅 OpenAI authorize 接口可用 retained ID token 作 `id_token_hint`，对应 URL 也须脱敏。[账号与会话](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)

**工程建议，非官方 Windows SDK 保证：** Windows 用受当前 OS 用户保护的 Credential Manager/DPAPI 或等效 secret store；若另保存 profile JSON，限制 NTFS ACL、原子替换，日志/IPC/诊断中不暴露令牌。不要把 Unix `0600` 当成 Windows 上已落实的访问控制。Main/子代理共用一个 refresh 管理器与会话锁，不各自复制 refresh token。

## 6. SDK、Rust/Windows 与是否需要 Node sidecar

| 路径 | 官方已证实 | 工程判断与未确认点 |
| --- | --- | --- |
| 直接 OAuth + Responses | 文档给出完整 HTTP/OIDC 合同，以及用 OAuth token 填入 `api_key` 参数的 Python OpenAI SDK 流式示例。[注册与登录](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)、[模型与推理](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference) | Node 不是协议前提；可由现有可信后端实现。Rust HTTP/JWT/SSE 原生适配在工程上可行，但本文没有验证实现。 |
| 官方 DevKit 示例 | 2026-09-28 教程使用 Electron/React，`@siwc/local` 管理登录/profile/model/stream，`@siwc/react` 提供界面；凭据和推理留在 main process，界面桥接不传 tokens。[官方发布教程](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt) | 已读取允许域的教程，未读取其外链 DevKit 仓库或 npm 包；未确认包版本、分发许可细则、Windows secure-storage/打包承诺、原生 Rust SDK或兼容矩阵。 |
| Codex app-server | 可把本应用 OAuth token 传给子进程的 Responses provider；stdio RPC 控制 conversation，无需额外 Codex 登录。token 刷新由应用负责，`env_key` 方案须重启 app-server 再 `thread/resume`。[Codex app-server](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server) | 这是可选执行引擎，不是 AngelBot 全部推理必须依赖的路径；采用它会增加进程、协议和工具执行边界。 |

**建议：** 若目标是保留 AngelBot 自有 Main/子代理与工具策略，优先评估“受保护本地后端 OAuth + 统一 Responses adapter”。只有选择复用 TypeScript DevKit、而可信后端又不是 Node 时，才把 Node sidecar 作为可选实现方案；不能写成官方强制要求，也不能直接在前端引入 SDK 保存 token。

## 7. 身份、推理计费与本地执行权限必须分离

官方将 verified identity、ChatGPT plan usage scope、应用自己的 session/authorization 分开；接入应用负责自己的账号、会话、授权与连接器访问策略。[Quickstart](https://developers.openai.com/siwc/quickstart)、[身份验证参考](https://developers.openai.com/siwc/website)

**工程建议：** AngelBot 至少区分三层状态，不能由“ChatGPT 已连接”一次性开启所有能力：

- 身份/profile：谁登录、使用哪个 issued client ID 与 workspace 注册。
- 推理资格：granted plan scope、token 有效、动态模型、该次请求政策/额度是否允许。
- 执行授权：用户是否允许文件写入、命令、浏览器操作、外发消息、敏感数据访问；OAuth 成功绝不自动批准这些动作。

所有 Main/子代理模型请求走同一提供方边界：冻结任务所选 profile，不让运行中的任务因界面账号切换误用另一用户 token；统一刷新与错误分类，集中移除不支持的字段，规范 namespace/additional-tools 包装，保留客户端 history 和 terminal-event 检查。凭据不放入 system prompt、工具参数、agent memory 或前端事件。

这里的 OpenAI 账号/workspace 注册与 AngelBot 的 Personal/Project Workspace 是不同概念：前者决定推理资格，后者决定本地上下文与执行权限。不能根据 OAuth 选中的工作区新建、合并或授权 AngelBot 项目。

## 8. 错误与恢复的最低覆盖

以下恢复规则来自[错误与恢复](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery)。

| 类别或代码 | 应用动作 |
| --- | --- |
| 缺 plan scope / 用户拒绝 | 保留有效身份，套餐 disabled；由用户选择重新授权或其他计费路径。重新授权用已保存 client ID；不要每次普通登录强迫 consent。`force_reconsent=true` 须经 OpenAI 确认部署；之前支持 OAuth `prompt=consent`，不要混为 Responses body 的 `prompt`。 |
| pre-stream admission 401/403/503 | 可能只有 `detail` 而无标准 `error`；保存真实 status、body shape、request ID；检查身份/scope、地区/政策或短时路由不可用。 |
| `subscription_sharing_user_not_eligible` | 403；解释账号/workspace/policy 限制，不重复同请求或循环 OAuth。 |
| `subscription_sharing_usage_limit_exceeded` | 429；暂停套餐新请求，引导 Manage usage；不自认全套餐耗尽，也不猜 reset time。 |
| `subscription_sharing_usage_unavailable`、`subscription_sharing_user_unavailable` | 503；保留凭据，有限退避。 |
| `subscription_sharing_unsupported_capability` | 400；检查 `error.param`，修改不支持的参数/工具/模型/执行能力/service-tier override，不原样重试。 |
| `subscription_sharing_route_not_supported` | 403；核对 HTTP method/endpoint，不能从其他客户端支持推定本路径获准。 |
| `subscription_sharing_invalid_user` | 401；保留 request ID，诊断凭据；确认撤销或终态刷新失败后再要求重登。 |
| `chatpass_v2_scope_not_authorized`、`chatpass_v2_invalid_authorization_context` | 403；核对注册与实际 grant，不靠重试或换计费掩盖。 |
| refresh 的 `invalid_grant`、`invalid_refresh_token`、`token_expired`、`refresh_token_expired`、`refresh_token_invalidated`、`refresh_token_reused` | 清理不可用 tokens，用已签发 client ID 重走 OAuth；`invalid_client` 则修复 client 配置。 |
| 用户在 ChatGPT 断开应用 | 当前没有断开通知；请求/refresh 确认断开后停止使用并要求重新登录，不能仅因短时网络错误抹掉凭据。 |

## 9. 施工前差距、真实账号待验证与后续验收

**本研究文件没有实现或验证** OAuth listener、动态注册、ID-token 验签、secure storage、refresh/revoke、动态模型目录、Main/子代理 Responses adapter 或真实套餐推理。本文不能作为“已接入”“全部代理可用”“Windows 原生 SDK可用”或 CI 通过的证据，也不据此判定其他开发工作是否完成。

**尚未被官方允许域资料或真实验收确立：** 用户/工作区真实 eligibility；完整 SIWC 地区矩阵；所有可用模型；精确套餐/应用余量、周额度与 credits 单价；DevKit Windows/Rust 支持矩阵；特定 AngelBot 工具 schema 能否在当前账户成功运行；长任务和并发规模的上限。不能把这些未知写成承诺。

**工程建议的离线验收：** 用确定性 HTTP/OAuth/JWKS fixtures 覆盖首次/返回/切换账号，拒绝 consent/state/client-ID/nonce/signature mismatch，模拟 refresh rotation 竞争、原子写入失败、撤销未确认、模型刷新、pre-stream 非标准错误、mid-stream failure/incomplete/EOF。工具循环覆盖两个以上调用、reasoning encrypted state、assistant phase、call-ID 对应关系与历史回放不丢项。用 spy 确保 Main/子代理均走套餐 provider，且没有秘密进前端、日志或 prompt；本地工具仍受原执行权限约束。

真实首次推理只能在用户明确启动登录并授权套餐后做：先 discovery，再用实际允许模型做小请求，以 `response.completed` 为通过标准；展示 Using ChatGPT plan / Manage usage，并清楚说明共享额度与可选 credits。仓库测试按 `python scripts/verify.py` 的 canonical flow 执行，不把真实凭据或网络接入纳入 full profile。该段为验收建议，本次研究没有执行。

## 10. 施工前代码核对与最小实施接缝

本节是开始实现时的基线审查，不代表施工后的当前代码；运行时状态与未完成边界以第 12 节及统一路线图为准。

以下为 2026-10-04 的只读代码核对，不是接入已完成。按模块设计技能，优先在已有 provider 解析与调用接口增加真实 adapter，集中认证复杂度，不新建平行 Agent 循环。

| 现有模块 | 核对结果与改造范围 |
| --- | --- |
| `src-tauri/src/llm/mod.rs` | `LlmProvider` 已是统一调用接缝，可保留调用方法形状；`Message` / `LlmResponse` 没有完整 Responses items 载体。新增后端私有、绑定 provider/model/账号注册的续轮状态，不能用 provider 内存缓存替代持久化。 |
| `src-tauri/src/llm/openai_compat.rs` | 现发送 `/chat/completions` 的 `messages/max_tokens/temperature` 和嵌套 function 工具。保留原 API 兼容 adapter，独立构建套餐 Responses 请求与 SSE parser；流中工具或文字不是完成，缺失 `response.completed` 必须失败。 |
| `commands/foreground_model_factory.rs`、`agent/workers/delegated_model_host.rs` | 主/子代理分别解析 provider，须复用一个认证模块及按账号注册串行刷新的机制。当前后台重新读取活跃配置，若与不可变任务绑定不一致则拒绝执行，既有实现不是多账号注册表；恢复原绑定对应的账号注册/profile 仍待新增，不能随设置中的活跃账号变化。凭据引用目前硬编码 `keychain:default`，须识别套餐账号注册而非存 token。 |
| `commands/foreground_history.rs`、`commands/foreground_run_store.rs`、`agent/workers/delegated_model_execution_host.rs` | 目前只持久化/重建普通消息、tool calls 与 call ID。续轮载体必须贯通执行、保存、后续历史、编辑分支、恢复与子代理请求；UI 仍只投影普通文字和既有安全活动，不展示 opaque reasoning。 |
| `keychain.rs`、`commands/api_config.rs` | 复用系统凭据库与非秘密配置投影；OAuth token bundle 不进入普通 JSON 配置或前端。套餐 adapter 固定官方 endpoint，禁止令牌发往用户自定义地址或被重定向到其他来源。 |
| `src/components/Settings/pages/ApiSettings.tsx`、`src/lib/model-config.ts`、`commands/api_config.rs` | 前端、聊天 readiness 与后端保存目前都要求多数 provider 有 Key。认证模式和脱敏连接状态须贯通设置 IPC、配置映射与 readiness；不能放宽所有 provider 的 Key 检查。模型目录查询与小请求完成验收要分开，不把 GET `/models` 误报为生成成功。 |

最小实施顺序与可观察完成标准：

1. **协议合同与 fixtures：** 固定允许参数、工具 namespace、完整输出项回放与流式终态；确定性错误/截断流不执行工具，不污染已提交历史。
2. **认证模块：** 系统浏览器授权、一次性 loopback 事务、ID-token 验证、实际 granted scopes、凭据原子替换、单次刷新与撤销；界面不收到 token。正常登录不创建 AngelBot 产品账号。
3. **Responses adapter 与恢复：** 保留现有其他 provider；同一套餐路径完成文字、两次工具续轮、追问、压缩/编辑、重启恢复和子代理绑定验证。opaque 状态按来源和账号隔离，不盲目跨模型/账号回放。
4. **设置与引导：** 在模型设置提供“使用 ChatGPT 套餐”连接及原有 API 方式；展示未连接、已连接但缺权限、可选择模型、需重新登录、额度/地区/策略受限等可理解状态。连接不阻止进入应用，不自动修改执行权限或切换计费。
5. **验收与激活：** 沿用 `python scripts/verify.py quick/full` 与隔离桌面旅程；最后经用户显式授权，用真实允许的模型完成有界小请求和一次受控工具续轮，才可声明该账号套餐连接可用。

目前这五步均为待实施计划；本文件不替代当前统一路线图，也不要求使用 Node sidecar、外部 Codex 进程或自建云服务。

本轮最终变更仅限本研究文件、统一路线图与 `.gitignore` 的研究文件白名单。`python scripts/verify.py quick`、`full` 均退出码 0：前端 60 个文件、461 项通过；Rust library tests 963 项通过、0 失败、1 项原生交互测试按设计忽略。验证保持离线依赖、确定性替身和无真实模型凭据；没有修改应用运行时代码，因此该结果只确认既有基线，不是套餐接入完成证据。独立代码核对未发现待修的完成状态宣称。

## 11. Pi 参考：通用厂商接入与 ChatGPT 套餐

本节为 2026-10-04 的只读源码研究，未修改 AngelBot 应用代码，未读取用户凭据文件，未执行登录、模型推理或 Pi 测试。访问 `badlogic/pi-mono` 的 GitHub API 得到现仓库 `earendil-works/pi`；本次以 GitHub `main` 与独立源码 checkout 均核实的提交 **`200387122ca450d6387f033949423114a270b96c`** 为依据，提交时间为 `2026-10-04T00:41:25Z`。下列链接全部固定到该 SHA，不能用旧版 Pi 文档、搜索结果中的 fork 或今后的 `main` 替代本次证据。[仓库重定向入口](https://api.github.com/repos/badlogic/pi-mono)、[固定提交](https://github.com/earendil-works/pi/commit/200387122ca450d6387f033949423114a270b96c)。

“Pi 已实现”在本节仅指固定提交中存在相应代码，**不等于已验证其官方协议合规性或真实账号可用性**；“AngelBot 建议”均为待实施设计，不覆盖前文尚未实现的安全、恢复与验收要求。

### 11.1 Model、provider、API 分开，不把厂商名称当协议

**Pi 已实现：** 模型对象分别携带 `id`、`provider`、`api`、`baseUrl`。`Provider` 管认证、目录、刷新/过滤与调用；`createProvider` 可以共用一个 API 实现，也可按 `model.api` 映射到多个实现。因此“一个厂商有多个模型”“多个厂商说同一种协议”“一个厂商提供多种协议”并非同一问题。当前无副作用的核心入口不再默认导入全局 API registry、全部目录或 OAuth 实现；旧的全局 `registerApiProvider` 保留在 `compat.ts`，不能把旧 registry API 当作新版唯一架构。[模型字段](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/types.ts#L1099-L1145)、[Provider 接口](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/models.ts#L144-L232)、[API 分派](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/models.ts#L989-L1074)、[核心入口](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/index.ts#L4-L8)、[旧注册接口](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/compat.ts#L128-L156)。

**AngelBot 建议：** 保留 Rust `LlmProvider` 的执行接缝，在统一 factory 前显式解析 provider 身份、wire API、认证模式与模型元数据；不要继续仅按厂商名把未知接入全部落到 Chat Completions。共享协议 adapter 与厂商认证/兼容策略分离，不要求采用 TypeScript、Node sidecar 或 Pi 的包结构。

### 11.2 已知协议用配置，特殊行为才用 provider 扩展

**Pi 已实现：** `models.json` 可配置兼容 endpoint、API、模型、header 和模型元数据覆盖；模型级 endpoint 优先于 provider endpoint。扩展可注册完整原生 `Provider`，或使用兼容旧扩展的 `ProviderConfig`，并可注销以恢复被替换的内建行为。Pi 等待异步扩展 factory，避免动态接入在启动选模型之后才出现。新版 `MutableModels` 本身也有 `setProvider/deleteProvider`，coding-agent 的 `ModelRegistry` 是运行时的兼容门面。[配置路径](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/coding-agent/docs/models.md#L45-L66)、[扩展注册与优先级](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/coding-agent/docs/custom-provider.md#L19-L70)、[集合注册](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/models.ts#L355-L368)、[兼容门面](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/coding-agent/src/core/model-registry.ts#L210-L223)。

**AngelBot 建议：** 通用 API-key 接入先支持少量明确 API 类型与自定义 endpoint；认证、动态目录或协议确有差异时才新增后端 adapter。不能因为 Pi 支持 `!command` 配置就引入隐式 shell 执行，也不能让自定义 endpoint 覆盖套餐令牌的官方目标地址。套餐账户与代理网关/API Key 配置应保持不同授权边界。

### 11.3 模型能力是元数据，但认证成功不等于模型获准

**Pi 已实现：** 模型包含输入模态、reasoning 支持、thinking-level 映射、context window、输出上限、成本、缓存和协议兼容字段；扩展文档要求 compatibility flags 根据实际服务器验证，不能只信“OpenAI-compatible”声明。`Provider` 可动态刷新并按 credential 过滤目录，`Models` 分开提供 `checkAuth`、`getAvailable` 与 `refresh`。[能力字段](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/types.ts#L1099-L1145)、[目录与过滤合同](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/models.ts#L160-L198)、[认证/可用目录接口](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/models.ts#L244-L284)、[能力验证要求](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/coding-agent/docs/custom-provider.md#L90-L108)。

**本次核查的边界：** 当前 `openaiProvider()` 仍装载静态 `OPENAI_MODELS`，该 factory 没有 `fetchModels` 或 credential-specific filtering；不能从 Pi 通用目录能力推导出它已按当前用户 SIWC 权限查询全部获准模型。AngelBot 应把“配置已连上”“服务实际允许的模型”“模型完成一次生成”分开，并将能力/上限放入后端模型描述，不靠前端模型名猜测。[当前 OpenAI factory](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/providers/openai.ts#L7-L24)。

### 11.4 统一流事件，同时保留明确成功/失败合同

**Pi 已实现：** 各 API 输出统一 `AssistantMessageEvent`，分别表示 text、thinking、tool-call 的 start/delta/end，以及最终 done/error，并带结构化 `partial` 或最终 message。事件流的 `result()` 在 error 事件上返回错误消息对象，而非仅凭 Promise resolved 就表示成功。[事件联合类型](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/types.ts#L769-L785)、[结果语义](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/utils/event-stream.ts#L86-L102)。

**AngelBot 建议：** 既有 string delta 可继续作为 UI 的文字投影，后端协议 adapter/runner 之间逐步增加类型化工具事件与终态；最终仍使用 Rust `Result` 和明确 stop reason，不把任意流关闭当成功。不能直接照搬 Pi 的旧 Codex 终态兼容：该 adapter 将 `response.done`、`response.completed`、`response.incomplete` 统一成 `response.completed`；前文 SIWC 的完成/失败合同必须由独立 fixtures 验证，不能因此放宽。[旧 Codex 归一化](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/api/openai-codex-responses.ts#L767-L782)。

### 11.5 可见消息投影与 opaque 续轮状态分离

**Pi 已实现：** 文本保留 `textSignature`，thinking 保留 provider-specific `thinkingSignature`，工具调用可保留 Google thought signature 和 namespace。Responses adapter 将完整 reasoning item 序列化存入 signature 后回放，文本 signature 保留 item ID 与 `phase`；并对最终 response 才提供的 encrypted reasoning 作补全。这不是仅保存 reasoning 摘要、文本或 `previous_response_id`。[内容类型](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/types.ts#L397-L426)、[reasoning/phase 回放](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/api/openai-responses-shared.ts#L255-L289)、[最终 encrypted state 补全](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/api/openai-responses-shared.ts#L534-L550)、[输出 item 捕获](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/api/openai-responses-shared.ts#L683-L703)。

**Pi 的跨模型规则及限制：** `transformMessages` 以 provider/API/model 三者相同判断 signature 可复用；跨模型丢弃 redacted opaque payload，移除工具 thought signature，并同步归一化调用/结果 ID；可读 thinking 跨模型转为普通文本。它还跳过 error/aborted assistant 消息，为孤立工具调用补错误结果。此处没有比较账号注册身份，所以不能当作 AngelBot 的跨账号隔离实现。[转换规则](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/api/transform-messages.ts#L83-L177)、[不完整 assistant 回放规则](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/api/transform-messages.ts#L187-L207)。

**AngelBot 建议：** 现有 `Message` 仍是可见 `content/tool_calls/tool_call_id`，需要另设后端私有 continuation 载体，在保存、恢复、编辑分支及主/子代理执行中贯通；绑定 provider/API/model/**账号注册/profile**，未知 opaque item 保真，不展示、不当作普通 prompt 或可读思维。跨模型、账号或协议切换应按本地安全合同重建可见上下文，不能机械照搬 Pi 将可读 thinking 降级为文本的行为，也不能把合成工具错误当作真实工具已执行。

### 11.6 CredentialStore 负责串行刷新，不能照搬文件保管方案

**Pi 已实现：** `Credential` 用 `api_key/oauth` 判别；`CredentialStore` 管 read/list/serialized modify/delete，OAuth 的 refresh 与 `toAuth` 分离。将过期检查放进互斥的 read-modify-write 中再次检查，只有仍需刷新时才刷新并在释放锁前持久化轮换结果；其他等待请求使用已更新的 credential。已存 credential 拥有该 provider 的认证选择，刷新失败或 handler 不匹配不偷偷退回环境 Key。[credential 类型与存储合同](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/auth/types.ts#L17-L95)、[无隐式计费回退](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/auth/resolve.ts#L27-L90)、[双检查刷新](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/auth/resolve.ts#L105-L158)。

**不可误读的差异：** Pi 的 coding-agent 以 `proper-lockfile` 锁住磁盘 JSON 并直接 `writeFileSync`；创建文件设置 POSIX mode，不等于 Windows 系统凭据库、加密存储或原子文件替换。其公共 credential store 明确每 provider 一个 credential，也不等于多账号注册表。[文件后端](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/coding-agent/src/core/auth-storage.ts#L24-L65)、[锁内直接写入](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/coding-agent/src/core/auth-storage.ts#L157-L196)、[读取最新项后更新](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/coding-agent/src/core/auth-storage.ts#L449-L471)。

**AngelBot 建议：** 在现有 keychain 后建立共享认证服务与按账号注册互斥的 token bundle 更新；主/子代理复用，保留任务不可变账号绑定。保留 Pi 的“刷新失败不切 API Key”语义，但原子持久化、撤销、账号切换和失败恢复必须按 AngelBot 自己的安全要求实现，秘密不得流入配置 JSON、日志、UI 或 prompt。

### 11.7 新 SIWC 与 legacy Codex 是两条不同路径

**Pi 已实现：** 当前仓库不只存在旧 Codex 登录。两条实现必须分别理解：

| Pi 路径 | API/endpoint | 认证与用途 |
| --- | --- | --- |
| `openai` | `openai-responses`；`https://api.openai.com/v1` | 同一 provider 有 API-key handler 与新的 ChatGPT subscription OAuth handler，OAuth 以 access token 作为请求认证。 |
| `openai-codex`（源码标注 legacy） | `openai-codex-responses`；`https://chatgpt.com/backend-api` | 旧 Codex 专用 OAuth、固定 Codex client ID 与 Codex 传输兼容逻辑。 |

[OpenAI factory](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/providers/openai.ts#L7-L24)、[legacy Codex factory](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/providers/openai-codex.ts#L7-L22)、[旧 Codex 认证常量](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/auth/oauth/openai-codex.ts#L22-L37)。

新 `openai-chatgpt.ts` 使用 `dynamic_agent_client`、稳定安装 UUID 对应的 host URN、loopback callback、PKCE/state/nonce、resource、`chatgpt.tokens.use.direct` 等 scope；回调取得 issued client ID，交换/刷新均使用它，保存实际返回 scopes，并要求 direct-token scope。这可以作为前文官方 SIWC 接缝的独立源码参考，**不能把旧 `openai-codex.ts` 直接等同 2026-09-28 官方 SIWC**。[新流程常量](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/auth/oauth/openai-chatgpt.ts#L15-L29)、[state 与 issued client ID](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/auth/oauth/openai-chatgpt.ts#L53-L76)、[返回 scopes 与 token bundle](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/auth/oauth/openai-chatgpt.ts#L162-L179)、[新授权参数](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/auth/oauth/openai-chatgpt.ts#L225-L263)、[refresh 与请求认证](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/auth/oauth/openai-chatgpt.ts#L208-L222)。

**已核出的安全缺口，不能以“参考 Pi”覆盖前文要求：** 新流程生成 nonce，却仅检查 ID token 是否存在；源注明不利用 ID token 识别用户，没有在该实现里完成 issuer/audience/nonce/signature/JWKS 验证。该文件也不是完整撤销或真实账户获准模型发现方案。AngelBot 仍需按官方合同实现完整验证、单次事务、固定目标 origin、安全持久化与撤销；也不能照搬 token endpoint 错误中直接拼接完整响应体的处理。[ID token 存在性检查](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/auth/oauth/openai-chatgpt.ts#L183-L205)、[原始错误体处理](https://github.com/earendil-works/pi/blob/200387122ca450d6387f033949423114a270b96c/packages/ai/src/auth/oauth/openai-chatgpt.ts#L134-L151)。

### 11.8 AngelBot 的最小借鉴范围与未核实项

**本地代码核对：** `src-tauri/src/llm/mod.rs` 已有 `LlmProvider` 与 `create_provider_from_config`；`foreground_model_factory.rs` 和 `delegated_model_host.rs` 已分别提供主/子代理解析接缝。保留这些模块和现有 Agent 工具循环，新增一个受控的 provider 描述/解析层、一套共享认证服务、真实 Responses adapter，以及后端私有 continuation 的存储/回放通道即可。先让通用 API-key 接入与套餐接入使用相同调用接缝，再按确有需求扩展其他协议；不以迁 Node、接 Pi CLI 或重写全部 provider 为前置条件。

**待实施建议：** 模型选择记录明确 API 与认证模式；动态目录按账号权限过滤；主/子代理共享认证刷新但不共享未经绑定的 opaque 状态；类型化流对 UI 只作安全投影；仅明确成功的终态提交历史/执行工具。离线 fixtures 覆盖多 provider、Key/OAuth 不回退、刷新 rotation 竞争、跨账号/模型状态隔离、完整 reasoning/phase/tool-ID 回放、截断流与重启恢复；验证继续使用仓库 canonical `python scripts/verify.py`，真实登录/推理另需用户明确授权。

**本节未核实：** Pi 新 SIWC 的官方认可/完整合规性、真实套餐账户的模型/地区/额度/credits、Pi 所有 provider 的真实兼容性、Pi 的 Windows ACL 实际保护效果，以及本地 Rust 实现的完整可用性。Pi 当前实现的静态目录、磁盘 JSON 或宽松 legacy 流转换都不是这些结论的证据。本节没有新增应用实现，也没有重新运行或改变前文已经记录的测试结果。

## 12. 实施与验收记录

实施验收日期：2026-10-05。官方资料核验与开工日期：2026-10-04。

### 12.1 当前落地范围

Pi 的参考落实为“厂商身份 / 协议 / 认证各司其职”，没有引入 Pi CLI、Node sidecar 或另一套 Agent。模块设计技能用于保留现有 provider / factory / runner 接缝；OpenAI Docs 用于确定正式授权与 Responses 合同；仓库验证技能用于统一离线验收。

| 接缝 | 实现 | 责任 |
| --- | --- | --- |
| 模型连接 | `llm/catalog.rs`、`protocol.rs`、`auth.rs` | 三种接口协议、API Key / 显式免认证 / ChatGPT 套餐，复用同一个 provider factory。 |
| 套餐授权 | `llm/chatgpt_auth.rs` | 动态 public client、loopback PKCE、OIDC/JWKS、加密存储、并发刷新、取消、断开、更换账号。 |
| Responses | `llm/openai_responses.rs` | 严格 SSE completed 终态、工具白名单、完整 output、跨模型/账号安全转换、撤销后的晚到响应拒绝。 |
| 主/子代理 | 现有 runner、execution kernel、delegated host、history/store | 原生私有账本、工具续轮、真实审批回执、已完成回合重开、编辑裁剪；没有第二套执行循环。 |
| 控制面 | `commands/model_connection.rs`、`api_config.rs`、现有 model factory/tool surface | 无密钥 IPC、账号模型目录、完整表单的小请求测试、非秘密连接引用与委派精确绑定。 |
| 设置 | 现有 ApiSettings / settings store / model readiness | 显式登录、取消、换号、断开、自定义协议，授权不等于配置已保存，连接测试不等于全工具能力验收。 |

旧配置未指定协议与认证时维持原行为；Ollama 仍走 OpenAI 兼容协议，不声称新增原生 Ollama、Gemini 或任意厂商私有协议。现有当前模型配置与系统凭据库复用，本轮不是多套 API 账户/Profile 管理器。

### 12.2 使用路径

1. 在 Windows 桌面应用打开“设置 → 模型与 API”，选择 OpenAI 与“ChatGPT 套餐”。
2. 点击 Continue with ChatGPT，完成官方账号登录和套餐用量授权。只有这个显式动作（或更换账号）才打开浏览器。
3. 从当前账号目录选择模型，或手填该账号获准的模型标识。目录读取失败有提示，不用静态清单伪造资格。
4. 可点击“测试连接”发送一个简短模型请求；这可能消耗少量套餐/API 用量，不只是探测网络。
5. 点击“保存”后才启用当前连接配置。用户没有 AngelBot 云账户；令牌、refresh token 和系统加密主密钥不进入设置或聊天消息。
6. “更换账号”验证成功后生成新的不透明引用；旧委派不会换到新账号。取消或失败保留旧连接。断开失败仍保守失效本地授权，不保证远程撤销已成功。

授权共享 ChatGPT 既有额度和可能启用的 credits；AngelBot 不承诺免费无限调用，不自动切换为 API Key。套餐认证与文件、应用、MCP 操作权限完全分离。当前官方资格、实际模型和地区以服务响应为准，不提供绕过限制的方案。[官方模型与推理合同](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)

### 12.3 安全与兼容取舍

- OS vault 只保存随机加密主密钥，避免 Windows 凭据大小上限；令牌 bundle 使用有版本 AAD 的 AES-GCM、原子替换和应用路径命名空间。
- 签名、issuer/audience/azp/expiry/nonce/JWK 用途均验证；不照搬 Pi 的“只检查 ID token 存在”路径。实际 scopes 决定套餐可用，不以登录成功冒充 entitlement。
- 刷新使用进程内 single-flight 与跨进程锁；轮换令牌持久化失败即停止，不把旧 refresh token 当作已成功更新。
- 凭据 source 固定授权 epoch。断开、重新授权或更换后，旧 source 与晚到的 completed 响应失效；完成态检查失败不交付工具调用。
- 委派绑定包含协议/端点/认证的 profile 标识、模型和凭据引用；待确认动作从 session metadata 重建原始连接，不用当前表单替换它。API Key 更新与连接变化轮换私有续轮引用；环境密钥只产生非明文身份指纹。
- 损坏或未知认证模式的配置在真实模型解析和设置读取时失败关闭，不静默恢复默认 API Key 计费模式。
- 设置草稿与实际保存配置分离。聊天标签、就绪提示与新会话绑定只读 active 配置；授权成功只更新草稿，保存失败不激活，更换/断开只禁用旧套餐绑定。保存成功后先提交脱敏快照，再读回规范化配置，读回失败不显示旧计费模式。
- 后端账本不进入普通 Message/聊天活动投影。数据导出仍可能携带后端消息 metadata；它不是令牌备份。

### 12.4 已知未完成边界

- **未做真实授权/推理验收。** 本轮不读取用户真实凭据，不调用真实 OAuth 注册、登录、刷新、撤销或模型服务。离线全量通过不等于特定账号套餐可用。
- **协议感知压缩未完成。** 私有 Responses 会话保留未压缩原生历史，不做旧式文本压缩或静默 100 行裁剪；超过请求/存储预算明确失败。长时间重度使用仍需后续原生回合边界压缩。
- **运行中逐轮 durable checkpoint 未完成。** 已完成回合可持久化重开；崩溃中的 opaque 中间状态不自动续接。现有 Scheduler 封存已领取任务，foreground 进入 needs_attention，不能盲目重放未知副作用。
- OAuth 注册和令牌文件未进入既有备份迁移，换机需要重新授权。断开不能撤销已经发生的外部操作。
- 未收录到既有可信模型元数据的模型仍使用保守上下文预算；本轮账号模型目录只用于选择与资格提示，不依据未知 `/models` 扩展字段虚构模型能力或上下文上限。
- 模型接入没有自动新增图像/PDF/Office 附件、远程 hosted 工具或更强的 computer use；本地工具继续受现有执行权限约束。

### 12.5 离线验证

聚焦认证 22 项、Responses 26 项和 private protocol 13 项 fixtures 已通过；覆盖真实签名假数据、轮换并发、错误/截断终态、撤销、换号、工具续轮、确认回执、编辑与真实临时 SQLite 重开。前端另覆盖协议/auth 映射、显式授权、动态目录、未保存表单测试和异步迟到竞态。所有凭据均为 fixture，数据库/锁文件使用临时目录。

统一 `python scripts/verify.py quick`、`python scripts/verify.py full` 均通过。最终 full：62 个前端测试文件、489 项测试通过，生产前端构建通过；Rust 类型检查与 1032 项 library tests 通过、0 失败、1 项原生交互测试按设计忽略。测试进程移除凭据环境变量，并启用 Cargo/npm 离线模式；不读取真实授权或发送模型推理。

补充 `cargo check --offline --manifest-path src-tauri/Cargo.toml --features desktop-e2e` 通过，确认桌面测试功能开关的编译兼容性；这不代表原生交互或真实账号验收已完成。

中间探测暴露的 pending confirmation 绑定问题已修正；旧 JSON 安全断言现直接检查密钥字段缺失，继续检查密钥值没有保存，避免把合法 `auth_mode: "api_key"` 字符串误判为密钥。长测试超过索引工具的超时后改用持久测试会话完成 canonical full，不以超时或修复前结果声称通过。

## 13. Windows 登录前失败修复（2026-10-05）

用户实际验收反馈：点击 Continue with ChatGPT 后未打开浏览器，立即显示“OpenAI 返回了无效的认证响应”。源码确认浏览器启动前先请求 OIDC discovery；公开、无凭据探测返回 HTTP 403、HTML，符合此错误路径。尚未取得用户原始请求的响应，因此不把该探测当成账号授权或 JWT 校验失败的证据。

本机启用了 Windows 手动系统代理，但 AngelBot 的 reqwest 0.12 构建关闭了默认特性，实际依赖图未启用 `system-proxy` / `hyper-util/client-proxy-system`。本次直接启用上游 `reqwest/system-proxy`，不新增代理模块、不修改用户系统设置。它支持 Windows 手动代理及绕过列表，不等于实现 PAC/WPAD；主动设置 `.no_proxy()` 的 DNS/IP 固定安全传输继续保持原边界。

认证响应解析复用同一纯函数 seam。HTTP 拒绝现在保留安全状态码，元数据、签名密钥、授权码交换和刷新错误带固定阶段标签；不会输出响应体、授权 URL 或凭据。OAuth 终止刷新代码仍可识别，非 JSON 的 403/5xx 不被当成账号已经撤销。没有删除前置元数据校验，也没有放宽 TLS、固定端点、禁止重定向、体积限制或 JWT 签名验证。

按 Diagnosing Bugs 流程，`discovery_http_denial_reports_status_without_response_body` 先以旧泛化报错失败，再以新 HTTP 403 提示通过。认证 26 项 fixtures 全通过，覆盖拒绝响应、状态码与阶段、敏感响应体不外泄、终止刷新分类及 HTTP 200 无效元数据拒绝。统一 `python scripts/verify.py quick` 和 `full` 均通过：前端测试与生产构建、Rust 类型检查通过，1036 项 library tests 通过、0 失败、1 项原生交互测试按设计忽略。测试清除凭据环境变量，使用离线依赖和进程级 `NO_PROXY=*` 隔离本机代理设置，不修改系统配置，也不调用真实登录或模型。

当前配置的本地系统代理公开探测仍遇到 TLS/连接重置，不能据此承诺网络已连通。需要用户在新构建中再次显式登录；本次未访问真实账户令牌，未完成授权、Luna 模型目录或推理验收。

修复后的 `npx.cmd tauri build --no-bundle` 通过，桌面程序已重新启动且窗口正常响应。旧进程未正常退出，在确认其可执行文件路径后结束该 AngelBot 进程；未修改系统代理、删除用户数据或覆盖模型配置。

## 14. 回调后的授权码交换诊断（2026-10-05）

用户再次授权时，浏览器已收到 loopback 回调，AngelBot 在“交换登录授权码”阶段报告网络失败。回调接收不等于令牌已验证或账号已连接；没有读取、复用截图中的授权码，也没有访问用户真实令牌或 Codex 凭据。

### 14.1 已观察到的证据与未确定项

以与生产相同的 reqwest 构建特性及客户端配置（系统代理、20 秒超时、禁止重定向）运行无凭据诊断：discovery 返回 HTTP 200，耗时 593 ms；token endpoint 的 POST 返回 HTTP 400，耗时 236 ms，两者均为 HTTP/1.1。POST 只提交一个故意不支持的 grant 类型，不带 client ID、code、verifier 或账户信息，不会注册客户端或交换真实令牌。只输出状态、耗时、正文长度和安全错误类别，没有输出正文或原始网络错误。

这个探测说明诊断时该 HTTP 路径可收到响应，**不证明真实登录、Luna 资格或推理已成功，也没有稳定复现用户的那一次连接失败**。尚不能确定是短暂连接故障、请求超时、浏览器等待后的连接复用，还是响应中断。没有据此调整代理、扩大超时、关闭连接池、切换 TLS 信任根或改动 OAuth 参数。

### 14.2 最小诊断和恢复改进

- 复用既有 `AuthError` 接缝，为真实传输失败增加四种固定类别：请求超时、连接建立失败、请求发送中断、响应读取中断。响应读取失败不再误报为连接未建立。只投影固定类别和既有阶段，不输出错误对象、URL、授权参数或响应体。
- 回调页明确提示返回 AngelBot 检查账号连接是否完成，不再使“收到回调”看起来像连接已经成功。
- 送达状态未知的真实刷新传输失败会结束本次调用，不在内部循环重放旋转令牌，也不按账号撤销处理；保留旧凭据。回归证明的是**单次调用不重发**，不是跨调用或持久化层面的“永不重放”：后续调用仍可能再次尝试过期的 refresh token，跨调用未知送达状态尚需进一步收敛。授权码失败仍需新的授权尝试，不复用截图中的 code。[官方错误与恢复规则](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery)
- 临时无凭据诊断源码已删除；TLS、PKCE/state/nonce、issuer、签名和范围校验保持不变。没有修改用户系统设置、运行数据或模型配置。

### 14.3 验证状态

真实 `read_response` 接缝的本地截断 HTTP fixture 先复现旧的泛化提示失败，再以“响应读取中断”通过。新增 fixtures 还覆盖连接拒绝、超时、敏感 URL/正文不进入提示、传输失败不重放刷新且不清空凭据。本地模拟服务的 accept/read/write 均有截止时间，不依赖外部网络或真实凭据。认证 30 项 tests 通过；canonical `python scripts/verify.py quick` 通过。

canonical `python scripts/verify.py full` 通过：62 个前端测试文件、489 项测试通过，前端生产构建和 Rust 类型检查通过；1040 项 Rust library tests 通过、0 失败，原有 1 项原生交互测试按设计忽略。测试进程移除凭据环境变量，并使用离线依赖和进程级 `NO_PROXY=*`；没有调用外部认证或模型，也没有读取真实运行数据。

旧刷新 backoff 的泛化 `Network` 分支现仅由历史 FakeTransport 驱动，真实 token send/body 返回 `Transport`，本轮不会进入该重试分支。撤销的显式服务不可用重试独立保留；生产刷新不再宣称支持网络失败自动 backoff。该测试驱动的旧刷新分支可在后续聚焦清理中删除。

`npx.cmd tauri build --no-bundle` 已通过。首次构建遇到运行中的旧可执行文件占用，确认其精确路径后请求关闭；旧进程未退出，因此结束该 AngelBot 进程，再构建成功。新版桌面程序已重新启动并正常响应，没有清理用户数据。实际账户仍需用户在新构建中显式登录；当前不宣称 Pro 的 Luna 已接入完成。

## 15. 保存与 Luna 连接失败排查（2026-10-05）

本轮按 Diagnosing Bugs 的证据与反馈接缝流程排查，使用 AngelBot Verify 进行离线验收，并按 OpenAI Docs 重新核对官方套餐推理约束。任务是排查，没有据尚未证实的原因修改生产实现、账号、密钥、系统代理或用户运行数据。

### 15.1 本次保存确实落盘，推理并未通过

用户确认浏览器授权与 AngelBot 的保存均已完成。首次检查时，Roaming/AngelBot/app_config.json 仍为旧 DeepSeek 配置，修改时间为 2026-09-17；这说明此前的套餐配置没有进入该文件，但不能证明用户漏点保存。

随后用户在模型页再次保存并测试。2026-10-05 10:18:06 UTC，实际文件更新为：

```text
provider=openai
model=gpt-5.6-luna
auth_mode=chatgpt_plan
protocol=openai_responses
base_url=https://api.openai.com/v1
credential_ref_present=true
```

只核对这些安全字段，不记录密钥、账号、实际 credential_ref 或令牌。当前运行的是正常 release 构建，不是 desktop-e2e 测试构建。没有发现模型路由环境变量覆盖，也没有另一个 Local 或工作区配置文件。

同一实际设置页显示 OpenAI、gpt-5.6-luna、ChatGPT 套餐授权和“已保存”；连接测试报错：`Responses 未返回 SSE 事件流；未提交工具调用。` 因此此时应区分“配置已保存”与“推理不可用”，不能把连接失败归因于配置回退。关闭、重新打开设置的后续页面验收未完成：用户切换到其他应用后停止了界面输入，没有继续操作其他应用。

### 15.2 保存成功提示有可复现缺口，但未证实是原始回退原因

真实 SettingsModal + settings store 的临时离线诊断以 mock native command 为边界，不替换保存逻辑：

- 当 mock save 真正更新持久层、随后 load 返回已保存结果时，保存、关闭、重新打开后保留套餐配置，测试通过。
- 当 save 返回成功、但 load 仍返回旧 DeepSeek 时，“不得显示已保存”的诊断断言失败；实际页面仍显示已保存。

原因是 persistApiConfig 保存后调用 loadApiConfig，但后者吞掉加载异常，且没有校验读回模型档案是否与提交结果一致；SettingsModal 只根据 promise 是否抛错投影成功提示。异步加载也没有防止迟到响应覆盖新草稿的版本约束。这个缺口有离线证据，但不能据此认定用户先前遇到的是同一路径；实际 native fs::write 的成功与当前文件更新已经另外确认。没有把 mock backend 不落盘当作真实 native writer 的行为。

临时诊断共 1 通过、1 失败；已删除临时测试文件，不将故意红灯的诊断留在正式测试集合。生产代码本轮未修改，缺口尚未修复。

### 15.3 连接失败已定位到响应类型检查，具体响应仍未知

openai_responses.rs:207–223 在读取 SSE 前先拒绝非成功 HTTP 状态。当前提示只可能在 HTTP 状态属于 2xx、但 Content-Type 缺失或不是 text/event-stream 时产生；忽略大小写和 charset 参数的处理已存在。

该分支尚未读取正文，也未保留具体 HTTP 状态、归一化响应类型或安全错误类别。因此不能仅凭当前提示确定是 200 JSON、HTML、204 空响应，还是正文实际为 SSE 但响应类型错标；不能直接判定账号、模型或代理失效。

test_model_connection 复用生产 provider，只发送一条固定测试输入，不带历史、工具或 reasoning 配置；套餐请求固定使用官方 Responses 地址，store=false、stream=true，并过滤套餐不接受的 max_output_tokens 和 temperature。没有发现这个测试请求明显违反当前官方约束。[模型与推理](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)、[预览限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

拒绝非 SSE 后提交工具的行为应保留；不能为了让连接测试变绿而接受 HTML、无终态的输出、切换计费方式或关闭 TLS 校验。

### 15.4 无凭据公开路径检查

用与生产相同的 reqwest 特性及响应客户端配置（系统代理、禁止重定向、禁用自动重试）向官方 Responses 地址发送一次故意不带 Authorization 的固定测试请求，只采样状态码、响应类型类别和耗时，不读取响应正文。结果是 HTTP 401、JSON、576 ms，符合未认证请求被拒绝的路径。

这说明诊断时公开入口能返回正常的未认证错误，不能证明同一账户请求会返回 SSE，也不能排除之前的间歇网络问题。此次没有访问真实令牌、发送历史对话、消耗模型额度或修改网络设置。临时诊断源码已删除。

### 15.5 下一步最小处理范围

1. 先补充响应类型错误的安全诊断：具体 HTTP 状态、归一化 media type；如需辨别正文，仅以受限长度分类 JSON/error/response/SSE/HTML/empty，不投影原文。只允许既有 allowlist 的 error.code 影响恢复提示，不输出 error.message、响应正文、Authorization 或回调 URL。
2. 用户再显式运行一次连接测试后，按实际类别决定是服务拒绝、响应协议差异还是网络中间层问题；不猜测或自动重试真实推理。
3. 保存链再单独补齐读回失败与不一致的处理，显示准确失败状态；异步草稿加载不覆盖新的选择。不要把连接失败绑定成自动切回 DeepSeek。
4. 真实 provider 的本地 HTTP fixtures 覆盖标准 SSE、JSON response/error、HTML、空 body、缺失类型，以及非 2xx 的独立错误路径；原有 Responses fixtures 主要验证 body 编码和 SSE parser，不能代替 HTTP 层测试。

本轮尚未修复这些缺口，也没有完成真实 Luna 推理验收。离线测试通过不能替代这一验收。

### 15.6 本轮离线验证

删除临时诊断文件后运行 canonical `python scripts/verify.py full`，7 项检查全部通过：diff hygiene、staged diff hygiene、release preflight、前端单元测试、前端生产构建、Rust 类型检查、Rust library tests。Rust 1040 项通过、0 失败，原有 1 项原生交互测试按设计忽略。

测试进程移除凭据环境变量，使用 Cargo/npm 离线模式和进程级 NO_PROXY=*；没有让自动化测试依赖外部服务、真实令牌或用户运行数据。公开入口检查与离线测试是独立步骤，不混作账户推理成功的证据。生产源码没有因本轮排查而改动，用户已保存的 OpenAI/gpt-5.6-luna/chatgpt_plan 配置仍在。

## 16. Luna 接入修复与分阶段验证（2026-10-06 至 2026-10-07）

本节记录用户授权修复后的实现与验证进展，更新第 15 节“尚未修复”的历史状态，保留前节原始排查记录。当前目标仍是用户已选择的 `gpt-5.6-luna` 与 ChatGPT 套餐连接；没有通过更换模型、改用 API Key 计费或放宽工具权限掩盖失败。离线验证、正常 release 构建和真实账号推理是分别验收的三个层次。

### 16.1 保存读回、草稿竞态与成功提示已补齐

`src/stores/settings.ts` 在保存后读取实际生效配置，检查其与提交的模型档案是否一致；读取失败、不一致或被更新的配置读取取代时，明确拒绝投影保存成功。文件写入完成与确认生效分开表达，不把“写入成功但读回失败”误报成“没有保存”。

配置读取增加请求版本约束，迟到结果不能覆盖较新的读取；读取期间发生的新草稿修改也不会被旧结果覆盖。`SettingsModal` 在保存后仍有更新的草稿、用户继续修改或切换页面时，不保留不准确的“已保存”提示。上述修改不把连接测试失败绑定成自动切回 DeepSeek，也不把 OAuth 授权完成当作模型档案已保存。

一次正常 release 重启后的实际设置仍保留 OpenAI / `gpt-5.6-luna` / `chatgpt_plan`。这确认了该次配置持久化结果，但不证明此前用户遇到的回退一定由某个已补齐的竞态造成；第 15 节的原始原因仍不得补写成既定事实。

2026-10-07，在最终正常 release 中再次保存现有套餐配置，页面显示“已保存”；关闭设置、重新打开并进入模型页后，仍显示 OpenAI、`gpt-5.6-luna`、ChatGPT 套餐及 OpenAI Responses，没有回退至 DeepSeek。未更换账号、重新登录或输入密钥。

### 16.2 真实缺失响应类型与最小兼容修复

修复前正常 release 的真实最小连接测试，经受限诊断确认返回 HTTP 200、缺失 Content-Type，正文类别为 SSE 格式。这里没有保留正文、账号、令牌或实际 credential reference，也没有证据确定响应类型缺失发生于服务端、代理或其他网络环节。

使用真实 Responses provider 的本地 HTTP fixture，将有效 `response.completed` 与缺失 Content-Type 配对，先复现拒绝，再完成最小兼容修复：仅当该 header 缺失或为空时，复用原有 SSE decoder，并启用严格 framing。除标准 `data/event/id/retry` 字段、注释、空行与首行 BOM 外，其余 framing 被拒绝；非空错误 MIME 仍拒绝。没有接受 HTML 包裹、普通 JSON Response、无完成终态或被截断的事件流作为成功。

该路径仍要求有效的 `response.completed`，并保留 output、namespace、工具白名单、模型与凭据续接绑定、提交前授权状态校验及大小限制。没有第二套 parser，没有重定向凭据、关闭 TLS 校验、自动重试推理或切换计费路径。官方本地接入示例也处理直接推理返回 SSE 但缺失 Content-Type 的情况；它的文本处理不能替代 AngelBot 的工具校验。[官方接入示例](https://github.com/openai/sign-in-with-chatgpt-devkit/blob/main/packages/local/src/responses.ts)、[模型与推理合同](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)

### 16.3 完成事件空输出数组的兼容修复

响应类型兼容修复后的正常 release 复测中，一次固定最小请求在 `request.send` 阶段失败，没有取得 HTTP 响应头；不能据此判断账号资格或具体网络原因。后续最小请求收到 HTTP 200、缺失 Content-Type，越过原响应类型检查，但被“私有续接数据”校验拒绝。进一步固定阶段诊断确认原因为 `Invalid private model continuation: empty output`：完成对象中的 output 数组为空。这是最终回填修复前的结果，不是最终版本的验收结论。

官方 Codex 将 `response.output_item.done` 的完整输出项与 `response.completed` 的生命周期完成信号分别处理。AngelBot 借鉴这一分工，但仍等到有效完成终态后才接受结果或提交工具，不采用提前执行策略。[官方 Codex Responses 事件处理](https://github.com/openai/codex/blob/main/codex-rs/codex-api/src/sse/responses.rs)

最终实现只在完成对象的 output 正好为 `[]` 且已经收到完整 done 项时重建输出。每项必须有合法 output_index，索引小于 128、从 0 连续且无重复；所有已开始的项都必须完成，已提供的 added/done ID 必须一致，创建事件与完成对象的 response ID 也必须一致。done 项累计编码上限与最终私有续接共用 512 KiB 限制。非空终态 output 仍为权威结果，不与暂存项合并；缺失或 null output、只有文本/参数 delta、未完成或失败的流不会因此变为成功。

重建后复用原有 output 解析、工具白名单、namespace、完整 JSON 参数、ID 去重和旧调用防重放校验，并在返回前验证凭据会话。推理私有内容只用于续接，不投影到界面或诊断中。这是有限的空数组兼容，不代表任意 SDK 都会重建空 output，也不放宽模型或工具权限。

### 16.4 固定诊断标签与保密边界

Responses 增加固定的 event、完成对象、output、usage 校验阶段，以及 typed 网络失败类别；只投影静态类别，不输出事件名称、ID、phase、usage 值、远端 message、响应正文、请求头或回调内容。2xx 非 SSE 的诊断限量读取正文并返回固定类别；该诊断读取受 5 秒与 256 KiB 限制，不将这些限制误写成整个正常 SSE 推理只有 5 秒。

`ProtocolContinuation::validate` 将原有拒绝条件拆为固定原因：binding、empty output、output count、encoding、output size、unknown output type。原协议、绑定长度、非空 output、最多 128 项、512 KiB 编码上限与允许的 output 类型保持不变；adapter 保留这些静态原因供下一次核验定位，不输出实际模型绑定或私有续接内容。

### 16.5 最终离线验证与正式构建

- Responses 目标测试 34 项通过，包括真实 provider 的本地 HTTP seam、缺失类型严格 SSE 路径、拒绝无效正文、静态阶段及保密断言。最终全量验证还覆盖空终态回填的拒绝矩阵：缺索引、越界、ID 冲突、过大、重复 done、索引缺口、未完成 added、response ID 不符、未准入工具、失败终态及仅有 delta；成功例验证逆序 done 还原、私有推理保留与非空终态优先。
- Protocol 的单个参数化测试完成红→绿，覆盖 binding、empty output、output count、output size 与 unknown/missing output type 的固定原因，验证无私有标记泄漏，并保留合法正向例；目标测试 1 项通过。正常 `serde_json::Value` 数据无法构造编码失败，本轮没有虚造该防御性分支的动态验收。
- 最终 Canonical `python scripts/verify.py quick` 和 `full` 均通过。`full` 的 7 项检查全部通过：diff hygiene、staged diff hygiene、release preflight、前端单元测试、前端生产构建、Rust 类型检查及 Rust library tests。63 个前端测试文件、498 项测试通过；Rust 1049 项通过、0 失败，原有 1 项原生交互测试按设计忽略。
- 全量测试进程移除凭据环境变量，使用 Cargo/npm 离线模式及进程级 `NO_PROXY=*`；HTTP fixtures 仅使用本地 loopback 模拟服务，不调用外部认证或模型，不依赖真实账号及用户运行数据，也不修改系统代理。
- 最终 `npx.cmd tauri build --no-bundle` 通过，生成正常 release 桌面程序，不是 desktop-e2e 测试构建。代码只读复核未发现阻塞问题；工具提交的终态门槛和拒绝边界保持不变。

### 16.6 真实 Luna 连接与设置重开验证通过

首次最终构建的界面验收遇到 Windows Computer Use 的 `failed to activate captured window`，刷新窗口选择后重试一次仍失败，因此当时停止输入、保留实机待确认状态。2026-10-07，原进程已关闭，重新启动同一修复后的正常 release 后，新窗口可以正常捕获和操作；没有通过自制自动化或系统设置绕过故障。

在该窗口中，使用现有 ChatGPT 套餐授权与 `gpt-5.6-luna`，点击一次“测试连接”，页面明确显示“连接测试通过”和“当前表单可以连接到模型服务”。随后保存现有配置、关闭并重新进入模型页，OpenAI / Luna / ChatGPT 套餐配置保持正确。此前请用户手动测试的请求已由这次实际验收替代，无需重复点击测试。

这确认了最终版本的现有授权、模型配置及固定文本推理路径可用。连接测试不发送历史、不带工具，也不等于已经验收真实工具续轮、子代理、computer use 或套餐额度的长期稳定性。应用保持启动，未切换 API Key 计费、修改系统代理或删除用户数据。
