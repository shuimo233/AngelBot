# AngelBot 架构决策

> 合并原 3 份 ADR，保留编号、决定、备选项与后果。accepted 是设计承诺，不是实现或真实业务验收证明；当前状态见 [统一路线图](current-roadmap.md)。主对话整体权限和第三方服务信任不等于子代理的能力 lease，产品授权边界见 [设计原则](design-philosophy.md)。

## ADR-0001

**Workspace supervision is the control plane**

**Status: accepted.** AngelBot exposes one Main Agent conversation per Workspace, while a durable Workspace Supervisor alone owns delegation, state transitions, and project-write decisions. This rejects user-addressable child agents and a general-purpose shared queue: workers produce bounded evidence or candidates, and the Supervisor serializes the only authority that can change Workspace state.

### Considered Options

- A single global agent queue would blur Personal and Project isolation and make recovery ownership ambiguous.
- User-visible subagent conversations would expose implementation detail and split the relationship with AngelBot.

### Consequences

Per-workspace input steering follows a Pi-style queue, but durable Work Packages, Attempts, leases, and candidates form a separate recoverable state machine. SQLite persists the control plane; in-flight work is inspected after restart rather than blindly resumed.

## ADR-0002

**Project mutation uses a Git-backed Workspace Repository**

**Status: accepted.** Every Project Workspace that AngelBot may mutate uses a Git-backed Workspace Repository, including non-code projects. Existing Git projects retain their own workflow; a non-Git project can opt into an application-managed Git base whose metadata, worktrees, checkpoints, and AngelBot records live outside the project root except for the required standard `.git` pointer.

### Considered Options

- Direct writes in a selected project directory are simpler but cannot isolate concurrent candidates or provide reliable rollback.
- AngelBot-specific files in every project root would make the project noisy and leak product internals into user material.

### Consequences

Candidate work occurs in isolated worktrees from private checkpoint refs. Materialization checks a State Anchor and is serialized per Workspace; drift triggers re-read, rebase, and review rather than a silent overwrite.

## ADR-0003

**Capabilities are leased and mediated**

**Status: accepted.** Tool, MCP, skill, filesystem, network, and automation authority is issued as the minimum expiring Capability Lease needed by a Work Package or fixed Automation Definition. A Tool Capability Gateway constructs the allowed surface and normalizes untrusted tool output; neither worker text nor tool output may expand authority or trigger new delegation.

### Considered Options

- A persistent `full_access` switch makes scope drift and MCP updates unsafe.
- Treating MCP registration as authorization confuses availability with permission.

### Consequences

Workspace enablement and task leases are separate. Tool manifest or command changes revoke the affected workspace grant until renewed approval; all project mutations, including MCP-originated ones, enter the Candidate Change path.

解释边界：本条约束 Work Package 和宿主可中介的候选流；Main 的整体权限不能直接传给 worker。对用户完全信任的第三方 MCP 进程，不承诺其所有外部副作用都能被宿主拦截或强制转换为候选；MCP 不是 OS 沙箱。实现与验收差距仍以路线图为准。

## ADR-0004

**Context is a bounded projection, not the source of authority**

**Status: accepted; implementation scope below.** 长期可维护的 harness 保留模型无关的身份、授权、任务状态、证据和恢复协议。模型负责理解与选择，不用某一代模型的提示词或“脑式记忆”黑盒保存权威状态。上下文变化和换模型不改变执行门禁、工作包或交付协议。

### Considered Options

- 每轮复制完整历史会放大噪音，也不能可靠保留约束；相似度召回不能决定一条明确规则是否仍然存在。
- 由模型生成摘要并把它作为唯一记忆，可能把未完成、已撤销或有例外条件的事项改写；增加层级索引或网状记忆库不直接解决这个问题。
- 新建通用上下文框架或另一套队列会复制现有职责；先加深已有 `ForegroundContextAssembler` 的准备接口、领域读取器与 `PromptBuilder`，保留真实差异所在的 Adapter。

### Consequences

运行时区分当前请求／显式设置、持久化约束及任务事实投影、可替换的摘要／相关记忆／学习偏好；这些输入不构成权限。工具依旧在执行时校验作用域和批准，交付与独立审校依旧由原状态机持有。worker 只获取明确工作包背景和授权证据，不自动继承 Main 的完整记忆或人格。

原生协议压缩也遵守同一边界：公开 OpenAI Responses 的服务端 `compaction` 项由原 Adapter 校验、绑定当前模型和凭据后回放；只缩减发送窗口，不删除持久分支或复制不透明状态到另一个账号／模型。当前宿主指令仍独立组装，工具调用仍须与显式调用字段一致并经过原权限门禁。私有账本与恢复继续使用现有事务，不增加第二套摘要权威、队列或付费回退；兼容端点不被推定支持该请求参数。验收和限制统一记录在路线图。

本次落实的是已保存的全局 `constraint`：Main 每轮准备和 `check_constraint` 复用同一个读取器，不依赖当前用词、学习开关或摘要。关闭长期记忆时不注入；排除被替代、遗忘及无法归属的会话记录，后者仍保留在数据库／备份中。投影保留来源 ID 和完整条目，最多 10 条、JSON 最多 8192 字节，超量或超长不截断成另一条规则，而以 `omitted` 明示，按任务补查或澄清；查询失败报错，不伪装成无约束。普通相关记忆不重复注入约束，也不召回已被替代的记录。

长期记忆读取关闭时，Main 的 `recall_memories`、`traverse_graph` 和 `check_constraint` 同时退出本回合的工具目录与执行注册表，不能通过显式工具调用绕过关闭状态。设置页查看／清理和用户明确批准的记忆写入仍沿原入口；操作权限、作用域和当前任务条件不依赖长期记忆开关，也不因此停用。

模型失败的原文是可能含敏感信息的不可信诊断，不属于会话或协议历史。Main 运行循环通过既有后端私有结果单独返回该值，投影与落盘模块将带步骤结果的诊断沿原受保护证据存储、与回复和任务状态同事务保存；公开正文、工具结果和生命周期事件只保留安全提示。用户停止不记为模型故障，不新增公共诊断读取入口、重试器或第二套日志系统。

这并非“永不忘记”的承诺：自由文本约束仍需模型理解，不能被当作安全策略执行；普通记忆／图谱的完整空间归属与预算、工作包按任务选择哪些规则，以及长会话质量仍需独立核验。本次不把全局规则自动复制给所有 worker。今后只在真实任务暴露缺口时调整选取／渲染策略，不改权威数据合同来追逐模型版本。
