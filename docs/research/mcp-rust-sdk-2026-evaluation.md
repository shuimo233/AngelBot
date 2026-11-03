# MCP 官方 Rust SDK 迁移评估（Windows / stdio）

> 调研日期：2026-09-29。范围：仅评估，不改生产代码；官方 MCP 规范、`modelcontextprotocol/rust-sdk` 的 `rmcp-v3.5.0` 标签和官方 API 文档为依据。

## 结论

**值得做一个窄范围 PoC，但不应立即把 `McpProcessManager` 整体替换成 `TokioChildProcess`。** `rmcp 3.5.0`（2026-09-28 发布）支持 2026-07-28 和旧版协议；它能接管协议握手、消息编解码、分页、请求取消等。但 AngelBot 已有的 Windows Job Object、`CREATE_NO_WINDOW`、环境隔离、用户确认和“工具结果未知时不自动重试”是产品安全边界，不能由 SDK 兼容性代替。[官方 3.5.0 发布](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.5.0) · [SDK 生命周期示例](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/README.md#client-lifecycle-modes) · [MCP stdio 关闭规则](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio#shutdown)

建议的接口切口是：**保留 AngelBot 的服务配置/凭据/工作区授权/进程策略层，先把握手与 RPC 客户端封装替换为 `rmcp`；运输层能复用现有 `ProcessTree` 才准入生产。** SDK 的 `Transport`/`IntoTransport` 支持自定义运输层及 async read/write，但把目前同步 `std::process::Child` 的管道接入 Tokio 仍是需由 PoC 证明的工程推断，不是“直接换一个类型”。[SDK transport 说明](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/README.md#transports) · [固定版本 Transport trait](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/src/transport.rs#L1027-L1081)

## 已核实事实与项目影响

| 范围 | 官方事实 | AngelBot 影响 / 推断 |
| --- | --- | --- |
| 版本与双时代 | 2026-07-28 取消 `initialize`/`initialized`，每次请求携带 `_meta`；旧版继续用握手。stdio 双时代客户端应先以 `server/discover` 探测，非现代错误或超时后回退，现代版本错误则按支持列表重试，不能误回退。[规范：版本与兼容](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning#backward-compatibility-with-initialization-based-versions) · [规范：stdio](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio#backward-compatibility) | 当前客户端固定发送 `2024-11-05` 的 `initialize`；只添加 `rmcp` 依赖不会获得新版协议。见 [`mcp_client.rs`](../../src-tauri/src/mcp_client.rs)。 |
| SDK 启动模式 | `serve()` **仍默认旧握手**。须用 `ClientServiceExt::serve_with_lifecycle(..., ClientLifecycleMode::Auto { preferred_versions: [V_2026_07_28], legacy_version: Some(V_2025_11_25) })` 才会探测并回退；SDK 的未响应探测回退阈值是 10 秒。[README](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/README.md#client-lifecycle-modes) · [固定版本源码](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/src/service/client.rs#L637-L675) | 需验证旧服务“不识别 discover 后返回可关联的非现代 JSON-RPC 错误 / 静默”两类回退，以及传输错误、未关联响应、现代拒绝码不会误回退；静默探测额外 10 秒可能逼近 AngelBot 普通服务 15 秒启动预算（Windows 裸 `npx` 为 90 秒）。 |
| 子进程 | `TokioChildProcess::new` 自行启动并管道连接；builder 可设 `stderr`，默认继承；`close` 走先关 stdin、等待约 3 秒后 kill 的优雅停止。[SDK 源码](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/src/transport/child_process.rs#L59-L115) · [builder / Transport](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/src/transport/child_process.rs#L120-L223) | **未从官方源码证明其会把 Windows 后代进程纳入 AngelBot 的 Job Object**；不能把 `TokioChildProcess` 默认清理当成 `npx.cmd`/Node 孙进程清理。现有 [`ProcessTree`](../../src-tauri/src/child_process_tree.rs) 设置 `CREATE_NO_WINDOW`、`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，并在停止时终止 Job，必须保留或以等效 Windows 测试证明。 |
| 权限与环境 | stdio 规范只定义消息/进程关闭，并不提供 OS 文件或网络沙箱。[规范](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio) | 保留现有 `env_clear` 后仅传启动必需变量和该服务显式变量、系统凭据库只写语义、工作区启用审批。SDK 只负责协议，不替代这些策略。[本地启动策略](../../src-tauri/src/mcp_client.rs) |
| 工具发现 | SDK `list_tools` 是分页 API，`list_all_tools` 循环读取 `nextCursor`；SDK 可缓存带 `ttlMs` 的列表，默认过期重新获取失败时可能返回旧缓存。[SDK 客户端源码](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/src/service/client.rs#L1701-L1772) · [缓存文档](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/README.md#caching) | 当前 `tools/list` 只读一页。PoC 应用 `list_all_tools` 并验证刷新时“不以陈旧工具定义通过确认”；必要时禁用 stale-on-error 或缓存，仍保留 AngelBot 的工具契约快照与确认策略。 |
| 工具调用 | SDK 有 `call_tool`，新版可自动处理 `input_required` 的多轮请求，`call_tool_once` 则只返回当轮结果。[SDK 源码](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/src/service/client.rs#L1377-L1435) · [规范工具消息](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#protocol-messages) | 自动多轮可能引出用户补充输入/采样权限；PoC 首期只放行目前支持的完整文本/结构化结果，遇到 `input_required` 明确报“不支持/需用户介入”，不得静默代答。当前桥接忽略原始非文本内容；SDK 不会自动改变产品展示层。 |
| 取消与超时 | stdio 的请求取消用 `notifications/cancelled`；SDK 的 `send_cancellable_request`/`RequestHandle` 提供请求级 timeout 和显式 cancel，`RunningService::cancel` 是连接级停止。单纯 drop 高层 `call_tool` future 不应被假设会通知服务取消。[规范](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio#cancellation) · [SDK 请求句柄](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/src/service.rs#L551-L680) · [SDK 连接关闭](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/src/service.rs#L1094-L1172) | 原有用户停止、外部写操作超时“结果未知，勿自动重试”、服务进程关闭要分别建模；SDK 通知取消不保证外部动作已撤销。不可把超时自动重试等同于安全恢复。 |
| 运行时/依赖 | SDK 客户端使用 Tokio；stdio 客户端需显式开 `client` + `transport-child-process`（或自定义传输所需特性）。[SDK README](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/README.md#build-a-client) · [SDK Cargo 特性](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/Cargo.toml#L124-L184) | AngelBot 已依赖 Tokio，但现 MCP manager/命令为同步接口；新增异步桥接可能触及锁、生命周期和 Tauri 命令返回，应先测，不应顺手重构授权层。Windows `npx`/`npx.cmd` 解析仍需回归。 |

## 最小 PoC 与准入条件

1. 独立实验模块/测试 fixture，固定 `rmcp = "=3.5.0"` 并只启用客户端与必要传输特性；不接入真实用户配置或密钥，不修改现有生产路径。先验证 `ClientLifecycleMode::Auto` 对新版、旧版可关联的非现代错误回退、旧版静默回退、现代版本不匹配的行为，以及总启动时间边界。旧版 fixture 必须覆盖现有客户端所用的 `2024-11-05`，而不只测试 SDK 示例的 `2025-11-25`。[SDK README](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/README.md#client-lifecycle-modes) · [规范回退](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio#backward-compatibility)
2. **Windows 必测**：`npx.cmd` 启动 Node 子/孙进程，窗口不闪现，环境里看不到 AngelBot 模型密钥，停止、启动超时、应用退出、父进程异常退出后无遗留进程。若 SDK 内建子进程路径不能满足，改用 AngelBot `ProcessTree` + SDK 自定义 `Transport`；在此通过前不删 `ProcessTree`。[规范 Windows 关闭建议](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio#shutdown) · [SDK 运输接口](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/src/transport.rs#L1027-L1081)
3. 对两代 fixture 比较 `tools/list` 多页、工具定义变更后的确认失效、`tools/call` 文本/结构化/错误、单请求取消、超时后的结果未知提示。生产接入前还须证明新鲜 `tools/list`、schema 快照校验、`before_call`、`tools/call` 的顺序与现有单服务权限门等价，不能因异步化产生定义变更后仍调用的竞态。外部写操作**不自动重试**；拒绝把原本的审批和工作区限制移入 SDK 内部。[规范工具安全](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#security-considerations) · [SDK 请求取消](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.0/crates/rmcp/src/service.rs#L551-L680)
4. 评估异步桥接和二进制/依赖增量后，再决定是否分阶段迁移；只有上述行为被自动化测试证明等价或更严时，才考虑移除手写 RPC。全量测试仍以 `python scripts/verify.py full` 为交付门槛。

## 隔离协议 PoC（2026-09-30）

[`e2e/mcp-rmcp-poc`](../../e2e/mcp-rmcp-poc) 是独立、固定 `rmcp = "=3.5.0"` 的测试 crate；不编入 AngelBot 主程序，不读取用户服务配置或凭据，测试逻辑不访问网络。依赖首次获取可能需要联网；缓存后可运行 `cargo test --manifest-path e2e/mcp-rmcp-poc/Cargo.toml --target-dir src-tauri/target --locked --offline`。7 项内存双工测试通过：现代 `server/discover` 与后续请求元数据、可关联的 `-32601` 错误回退至实际旧客户端所用的 `2024-11-05`、无响应探测约 10 秒后回退；现代能力拒绝、无共同版本、未关联错误、连接关闭均不误走旧版初始化。这里的“回退”**不等于任何 JSON-RPC/传输错误都回退**。

这只证明协议协商的一个窄切面。尚未覆盖真实 Windows `npx.cmd` 子孙进程、Job Object/环境隔离、工具分页与定义变更确认、请求取消和未知结果、UI 非文本结果适配、同步/异步桥接。因此生产 MCP 路径保持不变；尤其普通服务 15 秒启动预算中，静默探测单独消耗约 10 秒，接入前必须重新定义总启动预算并做端到端验收。

**当前决策状态**：隔离协议 PoC 首关通过；仍未证明 Windows 进程与权限边界、超时/取消语义和 UI 结果适配，所以**不建议现在直接替换生产 MCP 客户端**。

## 工具错误与外部效果（2026-10-20）

**错误回执不等于外部操作已回滚，不能据此自动重试。** MCP 将 JSON-RPC 协议错误与 `result.isError: true` 的工具执行错误分开；后者可以是 API、输入或业务逻辑错误，并未承诺原子性、回滚或恰好执行一次。向模型提供错误以便纠正参数，也不等于允许重放可能已生效的动作。[MCP 工具错误定义](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#error-handling) · [新版仍保留这一区分](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#error-handling)

Microsoft 的 MCP 服务在 `await backend.callTool(...)` 及合并回执之后的异常统一返回 `isError: true`，该路径没有回滚步骤；本机固定安装的 `@playwright/mcp 0.0.83` 所依赖 Playwright 打包实现也是这一顺序。这直接说明“动作已发生、后续结果处理失败”不能被排除。[第一方固定提交源码](https://github.com/microsoft/playwright/blob/83b80cfb2cc159fa2d3c4d9ee61803b55cdd06a5/packages/playwright-core/src/tools/utils/mcp/server.ts#L113-L123)

JSON-RPC 2.0 的错误码只描述错误类别，不提供事务保证。尤其 **`-32602` 不能当作通用的未执行证明**：官方 TypeScript SDK `1.17.0` 先等待工具 callback，再校验输出；输出缺少结构化数据或不符合 schema 时抛 `InvalidParams`，协议层保留该码与原请求 ID 返回。因此它既可能是调用前参数拒绝，也可能是执行后的输出失败。[SDK 调用与输出校验](https://github.com/modelcontextprotocol/typescript-sdk/blob/1.17.0/src/server/mcp.ts#L158-L220) · [SDK 错误响应发送](https://github.com/modelcontextprotocol/typescript-sdk/blob/1.17.0/src/shared/protocol.ts#L419-L434)

| 回执 | AngelBot 应采用的最小处理 |
| --- | --- |
| `isError: true` | 已收到执行错误，但外部效果未知；暂停本批后续操作，先核对实际状态，不自动重放。 |
| `-32602`（参数无效） | 同样按效果未知处理；不能凭错误文本猜测它来自输入校验还是输出校验。 |
| `-32603`（内部错误）、`-32000..-32099`（实现定义的服务错误）及其他未明确处理码 | 没有未执行证明，沿用结果未知与暂停机制。 |
| `-32700`（解析失败）、`-32600`（无效请求）、`-32601`（方法不存在/不可用） | 正确关联到本次请求时，可兼容地保留为“协议拒绝”，但不宣传为事务性无副作用或自动重试保证；仍需修正请求或服务。 |
| ID 不匹配、无法关联、响应缺失/损坏、超时/断连 | 不能把它当作本次调用的已知拒绝；调用一旦可能已发出，保留结果未知。解析/请求错误可能带 `id: null`，尤其不能跨请求误配。 |

上述错误码含义与响应关联要求来自 [JSON-RPC 2.0 §5–5.1](https://www.jsonrpc.org/specification#response_object)；“兼容协议拒绝”是 AngelBot 的有限产品分类，并非协议承诺。`tools/list` 等明确只读的发现步骤与已分发的 `tools/call` 不应混为一谈。

本次核查前，调用方把所有 `isError` 和 JSON-RPC error 当作普通已知失败，会使模型在外部动作可能已完成时继续纠错、重试或执行依赖动作。应复用已有 `ResultUnknown` 与主循环暂停，不另造恢复框架，也不按工具名、错误文本猜测是否只读。当前工具模型未保存可信的只读/幂等契约，故保守处理的代价是实际上只读的 MCP 工具报执行错误时也会暂停；后续只有具体可信服务提供可验证的无副作用或幂等契约，才适合窄化这一边界。工具 annotations 本身也须按来源判断可信度，而不是事务证明。[MCP annotations 信任要求](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#tool)
