# Hermes 的长期使用与学习闭环：对 AngelBot 的最小借鉴

日期：2026-10-15。研究对象按用户语境识别为 NousResearch 的 **Hermes Agent**，不是 Hermes 模型权重。源码固定到本次查询所得提交 `300228d24c593f779055204753921fbe9a2163c5`；官网是滚动文档。本次只检查公开官方资料和源码，没有安装 Hermes、执行它的测试、调用模型或读取个人运行数据。

## 结论

Hermes 的“自我优化”不是自动修改核心程序或在线训练模型，而是**在任务之后检查经验，调用受限工具持久化记忆或技能，下次再加载**。后台工作有真实执行链，但“写入成功”仍不等于“业务能力已提高”。AngelBot 不应把偏好抽取、对话压缩、程序性技能学习和权限策略混为一谈。[后台复盘实现](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/agent/background_review.py)

“KEPA / Knowledge-Enhanced Prompt Adaptation”未在本次检查的官方 README、`run_agent.py`、`background_review.py`、`prompt_builder.py`及官网定向检索中找到。不能把它写成 Hermes 的官方机制名称；这也不构成对全部历史源码的不存在证明。AngelBot 应使用自己的准确名称，例如“后台记忆复盘”和“重复偏好适配”。[官方仓库](https://github.com/NousResearch/hermes-agent/tree/300228d24c593f779055204753921fbe9a2163c5)、[官方提示构建](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/agent/prompt_builder.py#L192)

## 1. 三种知识分工，不把所有历史塞进上下文

| 类型 | Hermes 实际机制 | 对 AngelBot 的意义 |
| --- | --- | --- |
| 声明性记忆 | `MEMORY.md`、`USER.md` 有硬字符预算，在新会话开始时作为冻结快照注入 | 保存长期、跨任务适用的小事实；不保存完整日志 |
| 程序性技能 | `skill_manage` 创建/修改本地技能；目录、正文、参考文件分级按需读取 | 保存可重复的步骤、适用条件、坑点；不是所有任务完成后都新建技能 |
| 历史回溯 | SQLite 会话历史和 FTS5 `session_search`，按需返回真实消息 | 历史是证据库，不必持续注入所有历史 |

以上存储与加载由实际工具支持，不是“模型说记住了”就算持久化。Hermes 的 `build_memory_guidance` 特别要求：任务专属流程和坑点归技能；即使暂时不能写技能，也不要因此扩大常驻记忆范围。[记忆文档](https://hermes-agent.nousresearch.com/docs/user-guide/features/memory)、[技能文档](https://hermes-agent.nousresearch.com/docs/user-guide/features/skills)、[`build_memory_guidance`](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/agent/prompt_builder.py#L192)

长期连续会话也有代价：Hermes 文档明确指出冻结记忆快照不会在会话中不断重建，过长会话会反复压缩；推荐自然任务边界开始新会话。AngelBot 可以维持“一个助手”的用户体验，但内部应有有界上下文和可回溯的任务边界，不等于无限增长的单条消息上下文。[会话边界说明](https://hermes-agent.nousresearch.com/docs/user-guide/features/memory#memory-needs-session-boundaries)

## 2. 自动复盘确有调用链，但不是无限自主运行

1. `agent/turn_context.py::_tick_memory_nudge` 按用户 turn 累计记忆复盘间隔，恢复时可从历史用户 turn 数重新定位；不是统计所有消息后取模。技能间隔则由 `agent/turn_iteration_prep.py` 的工具调用迭代累计。
2. `agent/turn_finalizer.py::finalize_turn` 只有最终回复非空、未中断、未禁用复盘且达到记忆/技能间隔时才派发。
3. `run_agent.py::AIAgent._spawn_background_review` 决定立刻执行还是入空闲队列，默认不会从普通子代理继续生成后台复盘。
4. `agent/background_review.py::_run_review_in_thread` / `_run_review_fork` 创建受限 fork，调用真实记忆/技能工具；复盘转录、压缩绑定与主会话持久化分离。

来源：[turn 计数](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/agent/turn_context.py)、[技能计数](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/agent/turn_iteration_prep.py)、[结束触发](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/agent/turn_finalizer.py#L763)、[派发](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/run_agent.py#L753)、[fork](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/agent/background_review.py#L1210)。

两个不宜照搬的点：

- `review_idle_queue.py::ReviewIdleQueue` 合并同一会话的等待项，并等机器空闲；源码明确是**内存 best-effort**，不是持久、恰好一次的学习任务队列。
- 当前 fork 允许最多 16 次迭代；默认累计输入预算按复盘模型上下文的 75%计算，上限 600,000 tokens，未知上下文回退到 120,000。它虽有限制，仍远重于 AngelBot 当前小范围偏好学习所需；不能以“后台”推断便宜或免费。

来源：[空闲队列](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/agent/review_idle_queue.py)、[复盘预算](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/agent/background_review.py#L170)。

## 3. 持久化、授权与效果证据是不同层次

`tools/memory_tool_store.py::MemoryStore._write_file` 在变更锁内使用临时文件和原子替换，避免截断或旧快照覆盖并发结果。技能管理也是真实文件写入，而不是只更新当前模型上下文。[记忆存储](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/tools/memory_tool_store.py#L675)、[技能管理](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/tools/skill_manager_tool.py)

复盘工具执行有白名单，通常是 memory、skills 和用于检查技能的 `read_file` / `search_files`；危险命令确认不能由后台自行批准。官网提供独立的 memory/skills `write_approval`。更具体地，当前源码 `tools/memory_tool.py::_background_delete_gate` 会将无人值守复盘中的记忆 patch/remove 暂存，等待审批，暂存失败则拒绝；因此不能用官网总体的“默认自由写”推断后台能任意改删记忆。技能另有先读后写、固定技能保护和路径检查。[fork 白名单](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/agent/background_review.py#L1080)、[记忆变更边界](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/tools/memory_tool.py#L179)、[技能防护](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/tools/skill_manager_guards.py)

官方有确定性测试检查复盘 scope、输入预算、缓存/会话隔离、记忆/技能操作。例如 `test_skill_only_review_omits_memory_tool` 和 `test_review_input_budget_stops_tool_loop_before_next_provider_call`。这些证明机制有回归约束，**不证明生成的每条经验正确，不证明相似真实任务成功率提高**。本次检查的上述链条中，未发现“自动做业务效果 A/B 验证，通过后才晋级技能”的通用机制；不要把自行复盘当成质量评估器。[scope 测试](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/tests/agent/test_background_review_memory_scope.py)、[预算测试](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/tests/agent/test_background_review_input_budget.py)、[记忆测试](https://github.com/NousResearch/hermes-agent/blob/300228d24c593f779055204753921fbe9a2163c5/tests/tools/test_memory_tool.py)

## 4. AngelBot 应先复用的三件事

以下是针对 AngelBot 的工程建议，不是 Hermes 已实现能力的额外声明。

1. **先闭合有界、增量、持久的学习检查。** 复用现有后台复盘和 SQLite，以已处理的用户消息游标避免重复计算旧对话、重复增加同一证据的权重；重启、失败和并发需要有明确进度语义。只有真实新增用户表达才能形成偏好证据，助手回复、工具输出、引用的第三方文本不能作为用户偏好来源。
2. **记忆与程序性经验分开。** 本轮只修好既有偏好/约束学习，不因为 Hermes 有 skill_manage 就新建另一套技能框架。之后复用既有技能模块，保存已完成并有验证结果的可重复流程；失败事实必须带适用条件与证据，不把一次错误概括成永久规则。
3. **经验可纠正，但永远不能成为新权限。** 明确哪些只是表达偏好，哪些涉及敏感事实、发送范围或操作权限；模型推断不能关闭确认、提升权限或修改核心策略。验证至少覆盖“跨会话恢复、旧证据不重放、用户修正后不复发、上下文/存储有界”，程序性学习另外用同类任务的实际结果证明复用效果。

本研究文件未声称 AngelBot 已完成这些验收；最终实现范围与验证证据仍以当前路线图及对应测试为准。
