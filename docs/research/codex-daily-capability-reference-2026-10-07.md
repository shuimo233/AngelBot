# Codex 公开实现对 AngelBot 日常能力的适配参考

日期：2026-10-07（Asia/Shanghai）。范围：保持当前 `gpt-5.6-luna` / ChatGPT 套餐入口，检查本地工具、computer use 和自动化的可复用实现；不迁移整个编码 harness，不更换模型或计费方式。

这是一份源码与接口调研记录，不是实机验收报告。本次没有读取真实凭据、调用模型、操作桌面或修改生产代码。下文的 AngelBot 状态来自调研时的工作树；后续实施与完整验收由主任务统一执行。

## 1. 结论与公开边界

适合借鉴 Codex 的是**工具执行契约、结果投影和取消收尾**，而不是把 AngelBot 变成另一个编码工具。AngelBot 现有 Main Agent、权限、工作区队列、桌面适配器和自动化应继续作为产品主干。

官方列出的开源组件包括 Codex CLI、SDK、app-server、Skills 和 Plugins，IDE extension 与 Codex cloud 不开源。“Codex 开源”不能推导出桌面应用或其 Windows computer-use helper 全部公开。本次没有找到可以直接移植的完整桌面 computer-use helper，故不把它列为可复用源码。[官方开源组件清单](https://learn.chatgpt.com/docs/open-source)

本次固定核对 `openai/codex` 提交 [`5a3140176e668a2f72f3c098490eb7f7052d9d85`](https://github.com/openai/codex/commit/5a3140176e668a2f72f3c098490eb7f7052d9d85)（提交时间 2026-10-07 04:24:56 UTC）。仓库声明 Apache-2.0；如后续直接复制源码，应核对完整许可证、保留适用归属/NOTICE，并注明修改。本轮只参考设计，没有复制源码。[README](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/README.md#L81)、[LICENSE 第 4 节](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/LICENSE#L89)、[NOTICE](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/NOTICE)

Luna 文档声明支持图像输入、function calling 和 Responses；这不等于当前账号、套餐路线和客户端已具备完整电脑操作。ChatGPT 套餐入口目前支持符合其约束的本地 function/custom 工具与图像输入，但不支持托管 native computer use、hosted MCP/connectors、Responses `tool_search`。因此应保持 **Luna 判断 + AngelBot 本地执行器操作** 的分工，不能悄悄改走付费 API 或托管电脑工具。[Luna 模型文档](https://developers.openai.com/api/docs/models/gpt-5.6-luna)、[套餐入口限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

## 2. 值得参考的具体实现

| 接缝 | 官方源码证据 | 可借鉴的原则 |
| --- | --- | --- |
| 工具描述与执行 | [`ToolExecutor`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/tools/src/tool_executor.rs#L106)、[`ToolRouter`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/core/src/tools/router.rs#L235) | 执行器声明 schema 和是否可并行，宿主施加当次策略；默认不并行。只读查询可并行不意味着同一应用的动作也可并行。 |
| 策略与执行尝试 | [`ToolOrchestrator`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/core/src/tools/orchestrator.rs#L1) | 审批、执行环境和执行尝试集中管理，不让各工具各自实现一套权限。其 shell 沙箱升级重试不能照搬到发送、购买或 GUI 操作。 |
| 文本与图像结果 | [`FunctionToolOutput`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/core/src/tools/context.rs#L258)、[`FunctionCallOutputBody`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/protocol/src/models.rs#L2180)、[`MCP image 转换`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/protocol/src/models.rs#L2399) | 模型可见结果可以是 typed content；文本日志与实际图像载荷是两个投影，不应把图像损失性文本化后当成视觉输入。 |
| 一次终态与取消 | [`ToolCallState`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/core/src/tools/context.rs#L48)、[`notify_tool_finish_if_unclaimed`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/core/src/tools/registry.rs#L823)、[`取消回归测试`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/core/tests/suite/dynamic_tool_cancellation.rs#L299) | 取消不能只隐藏界面；工具、持久化历史与模型上下文都要收尾。迟到的成功响应不得覆盖取消终态或重新进入模型上下文。 |
| 进程与 MCP 资源生命周期 | [`UnifiedExecProcess`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/core/src/unified_exec/process.rs#L239)、[`MCP shutdown`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/rmcp-client/src/rmcp_client.rs#L1061) | 请求取消、关闭 transport、结束宿主拥有的子进程是不同步骤；中止本地进程不证明远端副作用被撤销。 |

特别注意：该版本 Codex 的 MCP 转换在存在非空 `structuredContent` 时可能优先选择其文本形式，而不是始终同时返回所有图像。AngelBot 应借鉴 typed-result 接缝，不应宣称或照抄“所有 MCP 内容都无损回传”。[`as_function_call_output_payload`](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/protocol/src/models.rs#L2304)

app-server 有线程恢复、turn 启动/中断和 item 生命周期事件，可作为未来专业编码执行器的边界参考；本轮没有必要把日常 Main Agent 改为 app-server 客户端，亦不应另建一套用户会话。[app-server 生命周期文档](https://learn.chatgpt.com/docs/app-server)

## 3. AngelBot 当前真实接缝

以下路径为仓库相对路径，行号来自本次检查快照；后续修改会改变行号。

- `src-tauri/src/agent/tools/tool.rs:684`：`ToolResult` 为 `name / success / content:String / terminate_batch`。已有 `RESULT_UNKNOWN` 判断和暂停机制，不需要再创建第二个未知结果框架。
- `src-tauri/src/agent/tools/handlers.rs:1708`：桌面 `RESULT_UNKNOWN` 转为 `terminate_batch`；桌面模型工具由 `register_desktop_handlers` 暴露。
- `src-tauri/src/desktop_control.rs:631`：已有可注入 `DesktopAdapter`、`MockDesktopAdapter`、观察、目标预检与受控执行接口。已支持 UI Automation 的命名控件，不等于任意截图/坐标 computer-use 闭环。
- `src-tauri/src/agent/tools/handlers.rs:2394`、`:2442`：当前桌面观察返回有预算的控件元数据与不透明 ref；操作后 ref 失效，要求再次观察。应保留这一抗过期目标机制。
- `src-tauri/src/mcp_client.rs:713`：MCP 文本与 `structuredContent` 被拼成字符串；图像等非文本只返回计数说明，实际内容不进入模型。
- `src-tauri/src/llm/mod.rs:39`、`src-tauri/src/llm/openai_responses.rs:658`：通用 `Message` 与 Responses 的工具输出仍是文本。仅模型宣称有视觉能力不能弥补这个传输缺口。
- `src-tauri/src/commands/foreground_automation_control.rs`、`foreground_automation_pump.rs`、`automation.rs`：已有会话绑定工具、工作区 Supervisor 派发和持久化运行状态。`automation.rs:1367` 恢复逻辑区分尚未跨越执行边界的任务与已经开始的任务，应复用，不另造 scheduler。

## 4. 三个最小专项适配点

### 4.1 P0：MCP 未知副作用必须触发既有暂停机制

**已确认代码层面的冲突**：`mcp_client.rs:929–934` 把 `tools/call` 超时或断连描述为“外部操作结果未知，不要自动重试”；`McpToolHandler::execute_current`（`handlers.rs:3056`）却将其转成普通 `ToolResult::error`。普通错误的 `format_for_llm`（`tool.rs:750`）又附加可以修正参数重试的提示，而且不停止批次。这是相互矛盾的运行时契约，不只是文案问题。

最小改法：由 transport 返回结构化的执行阶段/未知结果分类，区分**未派发的拒绝**与**派发后无可靠回执**；后者复用现有 `RESULT_UNKNOWN` + `terminate_batch` + `requires_user_review`，而不是通过匹配本地化错误字符串来决定安全性。未知 MCP 操作按用户所选整体权限和服务信任运行，仍不能把丢失回执当成可安全重复的失败。

测试接缝：`mcp_client` 的本地 fake server、`agent/tools/handlers.rs` 的 MCP handler、`agent/conversation/runner.rs` 既有 `unknown_side_effect_pauses_before_another_model_turn`。应断言超时/EOF 后模型不再发起下一轮重复动作、后续批次不执行；未启动拒绝不伪装成已操作；迟到回执不提升为成功终态。借鉴 Codex 的终态幂等与迟到响应测试，但不能推导远端 exactly-once。

### 4.2 P1：打通有预算的图像工具结果，不创建另一套视觉 Agent

**已确认缺口**：MCP screenshot 的非文本结果无法到达 Luna；本次检索 Rust 生产源码也未找到 `input_image` 或窗口截图模型输入路径。当前 computer use 主要依赖 UIA 文本结构。

最小改法：复用 `ToolResult`、通用模型消息和现有 provider 接缝，为模型可见输出增加可选 typed text/image；旧文本调用保持兼容。图像由受控单窗口观察或获准 MCP 产生，设定 MIME、尺寸、解码后字节和当次总量限制；模型不支持图像时明确降级，不能伪造“已经看过”。文本 UI/日志/摘要只保留安全说明或 evidence ref，不把 base64 大块写进会话。

不为视觉单独引入 supervisor、模型配置或权限系统，也不允许图像中文字扩大授权。UIA 目标 ref 和应用范围仍是执行依据；截图判断本身不授权自动点击。Responses/套餐路线的图像工具输出形状需要本地协议 fixture 与最小实机验收确认，不能把 Codex 内部序列化直接当成该路线已经支持的证明。

测试接缝：`mcp_client.rs` 内容解析，`llm/openai_responses_tests.rs` 请求捕获 fixture，`agent/conversation/runner.rs` 工具输出相关测试。使用合成小图，不读取用户桌面。验证 call ID 关联、文本和图像投影分离、非法 MIME/损坏 base64/超预算拒绝、无视觉模型降级、未完成 SSE 不接受执行结果、取消后的图像不进入下一轮。

### 4.3 P1：围绕现有自动化与桌面接口固化日常闭环验收

**已有能力应深化而非重写**：通知提醒、受限脚本和 Main-Agent 自动化已有独立执行类型。到点通知不需要调用 Luna；需要理解上下文的自动化才进入当前工作区 Main Agent。目标应是用户看到“做了什么、是否完成、哪里需要处理”，而不是工具调用计数或完成百分比。

最小改法：在现有 Python 验证流和已有 fixture 上补跨层场景，不创建第二套 runner/scheduler。先覆盖日常工作区对话、项目文件生成/定位、可信应用观察→预检→动作→重新观察、一次提醒/重复任务→派发→完成/需处理、取消→重开→不重放。调用被 Windows 接受（dispatched）只能证明派发，控件值验证只能证明该控件值；均不冒充业务目标完成。

测试接缝：`DesktopAdapter`/`MockDesktopAdapter`、`foreground_tool_surface.rs`、`foreground_automation_pump.rs`、`automation.rs` 临时数据库。现有测试包括 `startup_recovery_requeues_only_unstarted_agent_automations`、`due_scan_claims_only_enabled_scheduled_automations_once`、`cancelled_claimed_reminder_does_not_fire_or_record_a_completed_run`、`unknown_desktop_result_stops_the_remaining_tool_batch`。扩展这些接缝，避免重复建设已经覆盖的纯单元测试。

## 5. 实施顺序与验收边界

1. 优先修复 4.1 的未知副作用分类与暂停冲突，再进行会改变应用状态的实机测试。
2. 使用当前 Luna 路线验证已有文本工具闭环：日常、文件、UIA、提醒及 Agent 自动化；缺失配置或不支持控件应给出可恢复结果，不绕过权限。
3. 图像桥接按 4.2 单独作为明确能力提升；先合成 fixture，再用明确授权、无私人内容的单窗口实机验证，不自动扩大到全桌面或任意 MCP 资源 URL。
4. `python scripts/verify.py quick` 用于聚焦改动；最终统一运行 `python scripts/verify.py full`。完整 profile 必须不依赖真实凭据、外网或用户运行数据；真实 Luna/Windows 验收与离线 full 的证据分别记录。

本报告只完成调研与现有接缝检查。上述适配均为建议，不能据此声称已实现、已经过真实模型测试，或 AngelBot 已能操控绝大多数 Windows 应用。
