# Agent 架构借鉴：有界视图、原件回查与恢复边界

研究日期：2026-10-29。外部机制调研只使用官方文档与官方源码，未安装或运行这些代理，未调用真实模型，未操作浏览器／桌面，未读取用户运行资料；随后 AngelBot 自身的隔离模型回归在文末独立记录。本文中的“对 AngelBot 的推论”是候选工程判断，不是上游实现事实，也不是长期可靠性或效果增益证明。

本地基线是 [current-roadmap.md](../current-roadmap.md) 最新 2026-10-29 记录：来源邻域对照为 2/4→2/4，另一组分组对照为 4/4→4/4，均无增益证据且未入生产；生产仅调整稳定提示前缀。不能混合不同语料分数，不能据此增加 episode／冲突图或另一套记忆框架。

## Codex

本轮固定到 [`openai/codex@c6fce1caceaae6c4b2896e5c5ae456b5be3bf501`](https://github.com/openai/codex/tree/c6fce1caceaae6c4b2896e5c5ae456b5be3bf501)，提交时间为 2026-10-28T17:13:33Z。源码含独立的 `codex-rs/memories/read`、`memories/write` 和 `ext/memories` 模块；它们的存在不代表完整运行编排都归入 write crate，也不能只据 README 的路径说明推断最新编排归属。官方滚动文档、固定源码与模板合同分开引用；未证明任一模板在所有客户端默认启用。

### X1. 读写职责分开，摘要作入口，原件在需要时回查

**源码与模板合同：** README 将读取辅助和引用统计放在 `memories/read`，将提示渲染、文件／diff helpers 等放在 `memories/write`，不能把后者等同完整生成运行时。仓库中的 v2 读取模板要求用注入摘要指导偏好与工作续接；只有原句、时序或不确定性会改变答案时才读取对应 rollout，所需路径缺失时才选择性搜索；同时明确记忆不是当前行为的证明，不能为记录读取额外制造无用调用。[读写分工](https://github.com/openai/codex/blob/c6fce1caceaae6c4b2896e5c5ae456b5be3bf501/codex-rs/memories/README.md)、[v2 读取合同](https://github.com/openai/codex/blob/c6fce1caceaae6c4b2896e5c5ae456b5be3bf501/codex-rs/ext/memories/templates/memories/read_path_v2.md)

**对 AngelBot 的推论：** 复用已有后台复盘、SQLite 原句、前台有界投影和 `recall_memories`，不要把它们再次合成一个包办所有事情的“MemoryAgent”。摘要／top-N 只是工作视图；本轮补遗漏提示，比把整个偏好库或所有原会话塞回上下文更符合现有模块职责。

### X2. 更正、作用域和状态要保留，但不是由文件格式保证

**模板合同：** v2 consolidation 提示要求区分可复用的用户偏好与单任务选择／更正，保留适用范围、时序、归属和观察／提议／已完成／被取代／不确定等状态；较新更正优先，不得从旧摘要恢复已更正或删除的内容。v2 读取模板对用户明确的记住／忘记／更正请求要求追加小 note，而非直接编辑生成的汇总。[整理合同](https://github.com/openai/codex/blob/c6fce1caceaae6c4b2896e5c5ae456b5be3bf501/codex-rs/memories/write/templates/memories/consolidation_v2.md)、[前台更正模板](https://github.com/openai/codex/blob/c6fce1caceaae6c4b2896e5c5ae456b5be3bf501/codex-rs/ext/memories/templates/memories/read_path_v2.md)

**对 AngelBot 的推论：** 继续保留来源及用户撤销标记，不让迟到复盘恢复被撤销记录；任务状态不变成全局个人习惯。这些是要验证的语义合同，不是上游已证明能正确判断所有更正的证据。不据此直接增加自动 supersedes 图、多个整理代理或把 SQLite 换成 Markdown。

### X3. 后台生成有准入，不是每轮都多调一次模型

**文档合同：** README 描述的生成路径筛选旧且空闲的符合条件会话、受单次批量约束，处理前通过状态库 lease 防止并发重复。官方 memory 文档还说明，接近额度限制时可以跳过后台生成，并将“生成新记忆”与“使用已有记忆”分开配置；本轮未运行 Codex 验证各客户端的触发时机。[固定版本的生成说明](https://github.com/openai/codex/blob/c6fce1caceaae6c4b2896e5c5ae456b5be3bf501/codex-rs/memories/README.md#phase-1-rollout-extraction-per-thread)、[官方 memory 控制](https://learn.chatgpt.com/docs/customization/memories)

**对 AngelBot 的推论：** 核对现有 cooldown、来源去重和同会话 single-flight 后再决定是否补准入；不能因为想“更会学习”就添加常驻模型循环。Codex 的分阶段专用模型、文件工作树和全局整理器不应整体移植到这个多厂商个人助手中。

## Claude Code：把常驻索引与可回读原文分开

### 来源与可核验范围

官方仓库固定到 [`anthropics/claude-code@58d6da1e34d6cd1c6379dcbf568bd66f5f9b6102`](https://github.com/anthropics/claude-code/tree/58d6da1e34d6cd1c6379dcbf568bd66f5f9b6102)，提交时间为 2026-10-27T22:22:36Z。仓库提供产品说明、变更记录、插件等内容；本研究没有取得可逐函数核验下述机制的核心代理运行时源码，不能将它描述成完整开源运行时。[README](https://github.com/anthropics/claude-code/blob/58d6da1e34d6cd1c6379dcbf568bd66f5f9b6102/README.md) 与 [LICENSE.md](https://github.com/anthropics/claude-code/blob/58d6da1e34d6cd1c6379dcbf568bd66f5f9b6102/LICENSE.md) 也不提供这样的开源实现依据。以下是访问当日的官方行为文档，不把滚动网页绑定到该仓库提交。

### C1. 小索引不是完整记忆，也不是事实不存在的证据

**官方合同：** auto memory 的 `MEMORY.md` 是索引；每次会话开始只加载前 200 行或前 25KB，以先达到者为准，详细内容放独立主题文件按需读取。写后接近上限会提醒缩短索引；超限时文件仍写入成功，但工具返回错误要求重写索引，因为超出部分下次不会加载。这里丢掉的是初始模型视图，不是删除磁盘原文。文件留存直到用户或 Claude 修改／删除。[官方 memory：How it works](https://code.claude.com/docs/en/memory#how-it-works)

**对 AngelBot 的推论：** 最值得复用的是“当前看到的只是有界投影”这一合同，而不是复制 200 行、Markdown 文件或新增目录。既有学习偏好投影若只取最新 N 条，应明确标记不完整，并指向已有回查入口：未投影≠没有记录；谈及旧约定或与当前表态冲突时先找来源，不因索引缺席就覆盖／删除旧偏好。应由本地确定性回归验证提示装配和范围边界；这些文档不证明一句提示足以修好真实模型行为。

### C2. 更正可以持久化，但记忆文本不是强制策略

**官方合同：** 用户维护的 `CLAUDE.md` 与 Claude 根据更正／偏好写入的 auto memory 是两种机制；官方特别说明这些是上下文，不是强制配置，需要不受模型决定影响的动作阻断时应使用工具前检查。`/memory` 可查看／编辑记忆。[官方 memory](https://code.claude.com/docs/en/memory#auto-memory)

**对 AngelBot 的推论：** 保持既有事实来源、用户更正、临时例外、长期撤销与执行权限的区分。不能从“会把更正写入文件”推导出确定性的矛盾消解、可靠遗忘或无须确认的扩权；也不应用普通文本覆盖 AngelBot 现有持久状态与审批合同。

### C3. 渐进加载要保留边界与失效条件

**官方合同：** 普通 skill 的描述先进入上下文，完整正文在调用时加载；正文相同的重复调用只补短提示，正文变化则重新加载。压缩后按独立预算重新附着最近调用的 skills，旧 skill 可能被预算排除；正文继续存在不意味着其 `allowed-tools` 授权继续存在，授权在下一条用户消息时清除。MCP 当前文档默认延迟发现工具；10% 是 `ENABLE_TOOL_SEARCH=auto` 的阈值模式，不能写成所有会话的默认阈值。[Skills：调用与上下文](https://code.claude.com/docs/en/skills#control-who-invokes-a-skill)、[Skills：工具预批准](https://code.claude.com/docs/en/skills#pre-approve-tools-for-a-skill)、[MCP：Tool search](https://code.claude.com/docs/en/mcp#configure-tool-search)

**对 AngelBot 的推论：** 这支持审查既有按需工具／技能路径，而非再建一个 router：稳定工具身份和当前权限应在执行时重查；压缩丢掉正文后不能仅凭“以前加载过”拒绝回读；载入内容不能变成永久权限。是否值得追加去重机制，需要先证明当前路径存在重复成本或正文失效缺陷。

### C4. 恢复上下文、恢复文件与恢复外部业务不是一回事

**官方合同：** 会话持续保存到本地 transcript，可 resume／branch；checkpoint 保存 Claude 文件编辑工具涉及的快照，官方明确远程数据库、API、部署等外部动作不能靠 checkpoint 撤销，Bash 引起的变化也有跟踪限制。[会话保存](https://code.claude.com/docs/en/how-claude-code-works#work-with-sessions)、[Checkpoint 限制](https://code.claude.com/docs/en/checkpointing#limitations)

**对 AngelBot 的推论：** 保留已有 journal、终态分类及未知结果先只读核对的合同。恢复对话或回滚本地文件不授权重放发送、付款、写入等外部动作；没有理由为模仿“rewind”而扩建一个通用业务回滚层。

## NousResearch Hermes Agent：预算和原件回查写进工具回执

### 来源与范围

本轮源码固定到 [`NousResearch/hermes-agent@190c0b7f65e26d9d57837c9946d4a01e7c645ae8`](https://github.com/NousResearch/hermes-agent/tree/190c0b7f65e26d9d57837c9946d4a01e7c645ae8)，提交时间为 2026-10-28T16:59:28Z。研究对象是 Hermes Agent，不是 Hermes 模型权重。此前复盘／学习链的研究保留在 [2026-10-15 记录](hermes-long-term-learning-2026-10-15.md)，该记录固定在旧提交，本轮不把旧实现自动当成当前事实。

### H1. 容量可见、写入失败明确，不静默压掉已有记忆

**实现事实：** `MemoryStore` 默认 `MEMORY.md` 2,200 字符、`USER.md` 1,375 字符；`_render_block` 在模型可见头部展示使用量，`_add_overflow` 返回当前条目和超出字符数，要求有界整理后再写，而非偷偷删旧条目。外部编辑导致文件已超预算时，加载路径会告警但仍保留条目，不截断用户原文；因此这些是工具正常写入预算，不是任何情况下的绝对注入上限。[`tools/memory_tool_store.py`：默认预算](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/memory_tool_store.py#L188)、[加载越界](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/memory_tool_store.py#L248)、[超限回执](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/memory_tool_store.py#L329)、[可见容量](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/memory_tool_store.py#L669)

**对 AngelBot 的推论：** 容量、遗漏、拒绝和成功都应是模型可观察的合同。AngelBot 的存储并非 Hermes 的小文件全量快照；不应因此给整个历史／偏好库增加 2,200 字符硬上限。当前可选最小改动是让既有有界投影说明自己的不完整性，保留原有持久记录与按需读取路径。

### H2. 检索结果带原文锚点，截断需要可见且可继续读取

**实现事实：** `session_search` 区分发现、按消息锚点滚动、读取会话与浏览近期会话；返回数据库真实消息，不另用 LLM 生成回忆。`_shape_message` 保留消息 ID、角色、时间与工具关联；内容被有界截短时返回 `content_truncated` 和 `original_content_chars`。发现结果还携带 `match_message_id`、前后消息数量，支持后续滚动，而非让模型把片段当整段会话。[`tools/session_search_tool.py`：消息形状](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/session_search_tool.py#L237)、[发现结果](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/session_search_tool.py#L330)、[滚动](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/session_search_tool.py#L528)、[工具合同](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/session_search_tool.py#L662)

源码与概述文档有值得保留的差异：同提交的 memory 文档仍概括历史结果为“无截断”，但发现路径实际对窗口消息使用 4,000 字符上限，对首尾片段使用 1,200 字符上限；应以代码中的显式截断标志和后续读取合同为准，不能复制概述中的绝对说法。[概述文档](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/website/docs/user-guide/features/memory.md#L235)、[窗口与首尾读取](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/session_search_tool.py#L322)

**对 AngelBot 的推论：** 继续复用已有原件、来源时间、只读回查和范围隔离；本轮不是增建 session-search 系统。优先检查所有压缩／top-N 投影是否同样明确“缺了什么、如何回查”。FTS5 命中只提供候选证据，不证明跨语言泛化、正确理解临时例外或长期习惯稳定性。

### H3. 重复加载优化必须随上下文可见性失效

**实现事实：** `skill_view` 返回正文与链接文件的按需读取指引；`dedup_skill_view` 按任务、技能／文件和返回内容的 SHA-256 判断重复，返回 `content_returned: false`，不是永远只保留一个“已加载”布尔值。压缩结束路径会重置读取去重缓存，使此前可能被压掉的正文可重新取回；setup-needed 视图不去重，因为配置状态可独立于正文变化。[`tools/skills_tool.py`](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/skills_tool.py#L698)、[`tools/skills_tool_dedup.py`](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/skills_tool_dedup.py#L25)、[`agent/conversation_compression.py`：重置与调用](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/agent/conversation_compression.py#L3351)

**对 AngelBot 的推论：** 可将“压缩后能重新读到、变更后不返回旧正文”列为既有渐进加载路径的回归条件；这不是当前必须新增缓存、后台代理或新技能格式的理由。

### H4. 冻结快照和写入完成回执各有明确边界

**实现事实：** `format_for_system_prompt` 返回加载时冻结的快照，中途写入不会更新它，以保持前缀缓存。写入成功回执带 `done`、当前使用量／条目数量及不应重复提交的提示；批处理失败明确所有操作未应用。这些是记忆存储语义，不是外部业务的恰好一次保证。[冻结快照与成功回执](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/memory_tool_store.py#L649)、[批失败](https://github.com/NousResearch/hermes-agent/blob/190c0b7f65e26d9d57837c9946d4a01e7c645ae8/tools/memory_tool_store.py#L339)

**对 AngelBot 的推论：** 不宜直接复制“新会话才看到新记忆”的用户边界到持续 Main；用户刚更正的事实必须能由当前回合及既有回查路径获得。成功持久化、上下文可见、语义正确与日后业务更好是四项不同验收；本研究没有把它们合并。

## 本轮优先级：不引入新语义层

三个可复用的机制，按当前缺口排序：有界投影明确不完整并给出既有回查入口；返回原件锚点和显式截断／剩余范围；渐进加载的去重随压缩和内容变化失效。前两项最贴近当前 Main 学习偏好投影的可见性合同，第三项先作为既有路径的检查条件。

两个不宜照搬的机制／解释：一是把冻结记忆和显式 `/new` 当作持续 Main 必需的用户操作；二是把本地 checkpoint、写入成功或 resume 当作外部副作用可回滚／可安全重放的保证。它们均不构成迁移到文件记忆、替换 SQLite、自动晋级学习候选、扩大权限或再次真实模型试跑的理由。

本文是机制研究和最小候选判断，未证明本地候选改动有效；实现、回归与是否接入生产由路线图和对应验收另行记录。

## AngelBot 本轮落地：只补有界投影的完整性合同

在现有 `adaptive_constraints` 模块中，前台查询一次返回最多12条记录及同一筛选口径的遗漏数；状态、置信度、来源顺序、过期时间与会话范围不变，不再额外查一遍总数。主配置携带遗漏元数据；仅在记忆／学习开启且确实有遗漏时，Main 看到工作视图不完整，相关时才调用现有只读记忆工具，无法回查则说明限制。没有扩大初始记录数、增加摘要节点或后端服务，也没有把三家的存储格式带进项目。原 Vec 读器只在测试编译中给既有记录回归／控制组使用，生产入口只能取得带遗漏信息的投影。

定向离线证据 `logs/preference-projection-20261029-focused.log`：15条符合条件记录只投影12条并报告3条遗漏；其它会话、暂定、过期、禁用记录不计入；恰好12条／0条时遗漏为0；数据库异常返回错误，不冒充空库。提示回归验证开关关闭时不展示此元数据，且提示不是扩权、强制检索或自动操作。`logs/preference-projection-20261029-receipt.log` 的Python28例通过，新增验收拒绝计数与真实模型输入不符的回执。

一次真实 Main 接线回归 `logs/explicit-preference-6yaml7jk/acceptance.json`：沿用此前真实学过的合成原句，复制隔离库并加入61条人工噪声，不重学、不改题，固定 `closing_lookup` 在变更后的遗漏合同下运行一次。**16项合同通过**，当前12条不含目标、遗漏50条明确进入模型输入；Main 调用现有工具读回原句后答C，源学习库哈希未变、不同会话、空原历史、无业务动作，逐请求统计一致。模型 gpt-6-luna，2请求、输入14497／输出70。它是已知用例的新合同回归，不是新holdout或相对旧实现的准确率提升，也没有证明时间／更正语义和自然多日可靠性。

最终 `logs/preference-projection-20261029-final-full.log` **full9/9、退出0**：Python28／前端745／Rust1298通过，16 ignored另计。实现与源码／研究复核后执行，之后仅补记文档及diff／引用检查，没有重复模型试跑。未更新普通版或个人配置，未切换模型／计费、添加后台模型循环或扩大权限；不部署三家的完整运行时。外部机制说明和本地门禁均不替代自然多日使用验收。
