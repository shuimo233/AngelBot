# Muse 的工具调用与电脑操作：AngelBot 可借鉴点（2026-09-30）

范围：核对公开的一手产品资料和上游仓库文档，提炼适合 AngelBot 本地 Windows 工具的实现原则。下文把上游已公开能力与本项目工程建议分开；没有运行 Muse、连接真实账户或验证其模型质量。

## 身份与产品边界

结合时间和个人 Agent 语境，本次参考对象采用 **Meta 于 2026-09-08 发布的 Muse**。其核心环境是每人独立的云端 Muse Secure VM 与浏览器，而非将本机 Windows 桌面直接交给模型。用户选择连接哪些服务和读写范围。[Meta 发布公告](https://about.fb.com/news/2026/09/introducing-muse-personal-ai-agent/)

Meta 的产品页另介绍了 Mac 客户端对桌面文件、应用和浏览器标签页的接入，但该页未给出 Windows 执行器实现，因此不能据此声称 Muse 已提供同等 Windows 能力。[Muse 产品页](https://ai.meta.com/muse/)

同名项目应区分：Google Research 的 2023 Muse 是文生图 Transformer；Muse Spark 是 Meta 的模型系列；**CopilotKit/OpenMuse 是另一个组织的开源个人 Agent 应用**，不是 Meta Muse 的公开源码。[Google Muse 论文项目](https://muse-model.github.io/)、[Muse Spark 1.3](https://research.meta.ai/blog/introducing-muse-spark-1-3)、[OpenMuse README](https://github.com/CopilotKit/openmuse/blob/main/README.md)

## Meta 公开的可操作机制

Muse 有文件系统、终端和完整浏览器，可编写任务所需代码并生成文档等结果；目标能够按计划或事件继续执行。用户可中断并发送多项任务；后台通知优先展示有意义的变化。活动与目标界面提供进度，记忆文件可直接编辑。[Muse 产品设计](https://introducing.muse.ai/)

安全技术文章公开了以下具体边界：运行环境与宿主权限服务隔离；动作请求由独立 Sentinel 按对象、方法和范围决定允许、拒绝或询问；凭证在边界注入。浏览器子 Agent 通过独立 CDP broker 读取可访问性快照，无页面脚本执行接口。用户接管或填写凭证时 Agent 暂停；授权可限于一次、任务、会话或时限。[Muse 技术说明](https://research.meta.ai/blog/security-and-safety-for-ai-agents-our-approach-with-muse)

这些是公开架构描述，不是可直接导入 AngelBot 的 Windows 库。Linux 的隔离与网络控制也不能仅靠同名 Python 类在 Windows 上获得。

## OpenMuse 的实际复用范围

OpenMuse 是自托管 alpha：独立 API、任务 worker、持久 Chromium，以及可选 Docker Linux 工作区。它提供计划、检查点、暂停/恢复/取消、动作回执；桌面 GUI 和自动结账仍属后续工作。[README](https://github.com/CopilotKit/openmuse/blob/main/README.md)、[ROADMAP](https://github.com/CopilotKit/openmuse/blob/main/ROADMAP.md)

上游验证文档记录了租约竞争与过期恢复、审批对象/版本绑定、取消、不确定写入、重启后不重放已中断命令等用例。模型与真实 Google 账户的现场验收有单独限制；这些是上游报告，非本项目重跑结果。[VERIFICATION](https://github.com/CopilotKit/openmuse/blob/main/docs/VERIFICATION.md)

其 OpenBot 适配说明给出可借鉴的控制契约：快照产生 `snapshotId` 和不透明 `ref`；点击/输入必须携带对应快照；人工接管后拒绝 Agent 动作；不确定变更不自动重试。适配器默认关闭，文档明确没有接通部署，不能把这套接口算成 OpenMuse 已完成的桌面功能。[OPENBOT-INTEGRATION](https://github.com/CopilotKit/openmuse/blob/main/docs/OPENBOT-INTEGRATION.md)

部署当前仅面向单一拥有者；浏览器 worker 的应用级网络检查不是强多租户隔离。可选 Linux 命令不落到宿主 shell，网络关闭、工作区持久化。[SECURITY](https://github.com/CopilotKit/openmuse/blob/main/SECURITY.md)

## AngelBot 工程建议

以下是从上述机制提炼的本项目建议，不代表 Muse 内部实现或 AngelBot 已有完成度。与 [Windows 通用层研究](windows-generic-computer-use-2026.md) 的本地桌面方向一致。

| 优先完善的边界 | 建议的具体行为 | 借鉴依据 |
| --- | --- | --- |
| 可定位的观察 | 观察绑定应用、进程、窗口与一次快照；元素引用只在该快照有效，执行前重新核对目标。观察大小有界，返回明确的可用 patterns。 | Muse 的受限浏览器观察；OpenBot 的 `snapshotId` / `ref` 契约。 |
| 结构化动作 | 用同一入口表达点击、填写、选择、滚动、键盘动作与已授权范围；每项动作从解析到结果都有可追溯状态。 | Sentinel 的类型化请求；OpenMuse 动作回执。 |
| 动作后的真实结果 | 区分拒绝、未执行、已派发、已核验、结果未知。派发后再次观察；缺少业务证据时不写“完成”，不确定变更不自动重放。 | OpenMuse 的不确定写入与中断恢复验证。 |
| 可中断的会话 | 用户接管、停止、目标窗口变化时终止后续动作；释放执行器占用。恢复先重新观察，先前元素引用不得沿用。 | Muse 接管暂停；OpenBot 控制权边界。 |
| 任务跨步骤继续 | 保存计划、当前步骤、动作/结果关联与可恢复检查点。恢复从已证实结果继续；读取可重试，可能已经产生后果的变更先核实。 | OpenMuse 的 server-owned jobs、SQL leases、检查点与回执。 |

本轮最有直接收益的是完成“观察 → 定位 → 执行 → 再观察 → 回执”的本地闭环，随后让任务引擎消费这些明确状态。动态生成工具、云端 VM 或新增账户集成需要独立实现与验收，不能用演示或工具名称代替可用能力。

## 许可与验收界限

OpenMuse 仓库采用 MIT；若复用代码或实质性片段，须保留版权与许可声明。[LICENSE](https://github.com/CopilotKit/openmuse/blob/main/LICENSE) CopilotKit Intelligence 是另行配置的服务，不随这个仓库的 MIT 许可提供。[RICH-THREADS](https://github.com/CopilotKit/openmuse/blob/main/docs/RICH-THREADS.md) 本研究只借鉴接口和状态设计，不引入上游代码、品牌素材或托管服务依赖。

资料按 2026-09-30 查阅的公开页面记录；未固定 OpenMuse 仓库提交，后续实际复制代码应先固定版本并核对依赖许可。本地任何实现变更仍需通过 AngelBot 的 `python scripts/verify.py full`；上游测试与公开能力叙述不能替代 Windows 实机操作验收。
