# 工具图像链路实现与验收

日期：2026-10-07（Asia/Shanghai）。本阶段补齐已启用 MCP 工具的 PNG 结果到 Main Agent 和 Responses 的传输链，继续使用当前 Luna／ChatGPT 套餐路径，不新增视觉 Agent、权限档位或调度器。

## 实现范围

工具结果保留文本供界面、步骤和日志使用，经过校验的图像作为独立内存载荷进入下一次模型请求。Responses 按原工具调用 ID 提交文本和图像内容数组；纯文本请求格式保持不变。其他尚未接入工具图像的模型接口返回明确“未发送图像”说明，不声称已经查看。

通用 Message 与 ToolResult 的图像字段均排除序列化；历史 JSON 也不能反序列化注入图像。私有会话记录仅保存去图后的文本，模型请求成功返回后释放当前上下文中的图像；取消、错误结果及调用返回前发现撤权时不交给下一模型回合，工具缓存不保留带图结果。图像仍是未经信任的工具证据，不授予新的应用或操作权限。

| 模块 | 改动 |
| --- | --- |
| ToolImage | 复用已有离线依赖 png 0.17.16 完整解码，拒绝不支持的 MIME、损坏载荷、动画及超预算；Debug 只显示尺寸和字节数 |
| MCP manager 与 handler | typed 内部输出与旧 String 包装共用准入、定义快照、锁和发送路径；返回前再次核对权限；标准图像／资源载荷不进入文本投影 |
| ToolResult 与生命周期 | 当前回合图像与文本分开；失败或终止钩子丢弃图像；带图结果不进入工具缓存 |
| Main Agent runner | 仅支持图像的接口且运行未取消时传图；私有记录去图，消费后释放；有瞬时图像时不运行文本压缩 |
| Responses 与 provider 包装器 | function_call_output 使用 input_text／input_image 数组，保留 call ID、历史顺序检查与套餐 stream／store／namespace 契约；现有动态包装器转发能力标记，不新增包装 |

## 预算与失败行为

只支持 PNG，不支持 JPEG、WebP、GIF 或用户图片附件。单图压缩数据不超过 2 MiB，单边不超过 4096，最多 4 Mi 像素；解码缓冲和解码器均有预算，完整解码后再次拒绝动画标记。单个 MCP 结果最多 2 图；单次 Responses 请求最多 4 图、合计 4 MiB 压缩数据，并保留原 8 MiB 请求体预算。MCP 原有 4 MiB 单行预算不扩大，因此较大的多图返回可能先被 transport 拒绝。

已收到可信成功回执但图片损坏、不支持或超预算时，保留成功文本并明确省略图像，不鼓励为了取图重做已完成操作。没有可靠回执仍使用 RESULT_UNKNOWN；调用后撤权则丢弃回执并复用该人工复核暂停机制，不再进入下一模型回合。

MCP 的普通业务 structuredContent 保持兼容；仅标准显式图像／资源载荷脱敏，遍历限制为 16 层、4096 节点。这个处理不保证任意第三方工具自行放在纯文本中的秘密会被识别；服务信任与用户范围授权仍然必要。

## 格式依据

官方 function calling 文档允许以图像／文件内容数组返回函数结果；当前 Luna 支持图像输入。ChatGPT 套餐入口允许适配模型的图像输入，但不开放托管 native computer use。因此这里是本地工具结果传输，不是接入托管电脑执行器。[Function calling](https://developers.openai.com/api/docs/guides/function-calling)、[Luna](https://developers.openai.com/api/docs/models/gpt-5.6-luna)、[套餐限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

## 验收范围与后续

验收使用合成 PNG、现有 mock provider、临时测试数据及本地 HTTP／stdio fixture，不读取私人窗口、真实凭据或用户数据库。覆盖图像与 call ID 关联、旧文本兼容、损坏／超预算、日志与历史去载荷、取消／撤权、失败暂停及消费后释放。

最终 `python scripts/verify.py quick`（5 项检查）和 `full`（7 项检查）均通过：63 个前端文件、513 项前端测试全部通过；1083 项 Rust library tests 通过，0 失败，1 项既有原生交互测试按设计忽略且不计入通过数。包含前端生产构建、Rust 类型检查及 release preflight。过程中的两个旧测试夹具缺字段已补齐，没有跳过断言。编译仍有既有告警，未将全库告警清理混入本阶段。

新增 23 项针对性回归复用现有夹具并参数化覆盖串行／并行和支持／不支持接口，不新增旁路测试框架。尾部 fcTL 夹具在修复前明确失败于“错误接受图像”，修复后通过最终全量；这项复现与其余最终回归依据分别记录，不将编译迁移失败当作业务缺陷复现。

`npx tauri build --no-bundle` 成功，普通 release `src-tauri/target/release/angelbot.exe` 已启动并返回主对话。桌面核对确认运行配置仍为 `gpt-5.6-luna`、主对话与事项回看可加载；既有验收 Agent 自动化保持暂停，“先启用再执行”禁用，一次性提醒仍为已完成。未发送新模型请求、重做自动化、改动账户或应用授权。本次是启动与只读界面核对，不是安装包／签名发布验收，也不是新的真实 Luna 工具图像验收。

该链路不新增 Windows 窗口截图、坐标点击或浏览器正文读取能力，也不能替代真实 Luna 图像理解验收。接下来应在原 DesktopAdapter 与受信任应用范围内接入有界单窗口观察，再验证观察、预检、动作、回读和结果回流；电脑操作仍需满足既有确认和未知结果暂停规则。
