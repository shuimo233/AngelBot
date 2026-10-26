# AngelBot 通用 Windows Computer Use：可复用底座与边界（2026-09-30）

范围：面向个人使用、当前已解锁 Windows 交互会话中的桌面应用。本文区分「官方资料说明」与「AngelBot 工程建议」；不是 Codex 内部实现说明，也不是对“绝大多数程序可可靠操作”的实测结论。已有窄能力和实机缺口见 [现有核验](windows-computer-use-2026-evaluation.md)。

## 能从 Codex 借鉴什么

公开的 OpenAI 文档确认：ChatGPT/Codex 的 Computer Use 可看见并操作受用户允许的桌面应用；Windows 版本占用当前活动桌面的前台键鼠，按应用请求访问许可，并可对敏感或干扰性动作再次请求确认。文档还建议优先使用已有的插件/MCP 结构化集成。**公开资料未说明 Codex 的 Windows 底层如何组合 UI Automation、视觉识别或输入注入，不能宣称复制其内部实现。**[Computer Use 产品说明](https://learn.chatgpt.com/docs/computer-use)

OpenAI 的 API 指南提供可借鉴的**集成模式**而非现成的 Windows 执行器：调用方提供持续存在的浏览器/桌面环境，执行模型请求，回传观察结果；示例包含隔离代码执行和结构化鼠标键盘动作两种路径，也明确允许沿用自有 function/MCP UI 工具。安全要求包括隔离/动作白名单、将屏幕内容视为不可信、确认购买/传输/删除等高后果动作、限制步骤和时长、支持取消并核验真实结果。[Computer use API 指南](https://developers.openai.com/api/docs/guides/tools-computer-use)

## Windows 的通用层并非单一 API

| 层 | 官方依据 | AngelBot 工程判断 |
| --- | --- | --- |
| 应用专用 API/MCP | OpenAI 建议在可用时优先使用结构化集成。[Computer Use 产品说明](https://learn.chatgpt.com/docs/computer-use) | 数据读取/事务操作优先走可验证的专用接口；不要用 UI 点击重新实现已有能力。 |
| 浏览器页面语义 | Microsoft 将浏览器外壳的桌面元素和网页元素分开；Playwright locator 在每次动作时重新定位 DOM，且提醒避免用位置序号消除歧义。[Power Automate UI 元素](https://learn.microsoft.com/en-us/power-automate/desktop-flows/ui-elements)、[Playwright locators](https://playwright.dev/docs/locators) | 浏览器内容走 DOM/可访问名称/角色，不把网页当 Win32 控件树；会话与站点许可独立于桌面程序许可。 |
| Windows UI Automation | UIA 的控件树、属性、control pattern 可跨 Win32、WinForms、WPF 等框架提供共同语义，但能力取决于目标应用的 provider；自绘控件若无 provider 可能大体不透明。[UIA 客户端](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-clientsoverview)、[UIA provider](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-providersoverview) | 作为桌面主路径：有界观察、按 control pattern 执行、回读状态。不要假定所有 `Edit`、按钮、表格都可操作。 |
| UIA Raw / MSAA | Microsoft 的 Power Automate 使用 UIA 为默认；Raw view 用于非标准层级，MSAA 面向未提供合适 UIA selector 的旧应用。[Power Automate UI 元素](https://learn.microsoft.com/en-us/power-automate/desktop-flows/ui-elements) | 作为可诊断兼容层，不对每次动作自动盲目回退。Raw 节点更杂，MSAA 语义较弱，必须仍满足唯一目标和核验。 |
| 窗口视觉与前台输入 | `Windows.Graphics.Capture` 的常规流程由系统 picker 让用户选择窗口并显示捕获边框；`SendInput` 可模拟键鼠，但受 UIPI 限制，不能输入到更高完整性级别的应用，错误也未必说明 UIPI 阻止。[屏幕捕获](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)、[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput) | 只对语义层不可用的已授权、可见窗口开放“视觉辅助操作”模式。逐步抓取窗口、定位、输入、再观察；坐标不跨帧/窗口复用，用户切窗或目标失焦即停。不可静默升级为任意程序或全桌面权限。 |

UIA 的 `Name` 可本地化且不唯一，`AutomationId` 也仅应视为局部线索、可能随版本改变；目标身份至少应结合会话中的进程/窗口、容器路径、control type、pattern 和唯一匹配，动作前重新解析。[UIA 测试定位属性](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-usefortesting)

## 建议的最小公共契约（工程判断）

产品决策（2026-09-30）：当 UIA 无法提供足够语义时，用户接受**按任务授权的单窗口截图**交给当前配置的视觉模型；不采集全桌面，截图许可不包含点击或键入许可，不能静默回退。截图真正接入前，还需补齐多模态消息类型、窗口选取/失焦处理、敏感区域提示与用后清理；当前实现不具备视觉操作能力。

不暴露“运行任意脚本/PowerShell/全桌面点击”给 Main Agent，而把通用能力收在独立的 `ComputerUseSession` 边界：

1. `open_scope`：绑定用户允许的具体应用、进程与窗口（或指定浏览器站点），声明只读/填写/提交等权限和时限。不同应用、窗口、站点不能借用一次许可。
2. `observe`：返回有界 UIA/DOM 结构、当前窗口身份；仅在用户选择视觉模式后返回单窗口短时截图。密码、令牌、无关窗口和原始全桌面画面默认排除。
3. `resolve_and_act`：动作是结构化的 `invoke/select/set_value/scroll` 等，带一次观察产生的目标引用；执行前检查作用域、唯一性、窗口仍前台及所需 pattern。视觉键鼠是显式标记的另一执行级别，不是失败时静默回退。
4. `verify`：返回 `verified / dispatched / result_unknown / failed` 和可复核的状态差异；“API 调用成功”不等于业务完成。后果性动作展示确切对象、内容、去向并按权限规则确认；未知结果立即停下，不能自动重试。
5. `close`：取消、超时、失焦、用户接管时终止动作队列，释放捕获与辅助进程，丢弃临时截图、元素引用和敏感值。屏幕上的文字始终是不可信任务数据，不能改变权限。[OpenAI 运行安全](https://developers.openai.com/api/docs/guides/tools-computer-use)

这套契约宜作为现有 `DesktopAdapter` 上方的会话/策略层，而不是为每个应用新增一种 Agent tool。浏览器、UIA、MSAA 和视觉执行器共享观察、许可、确认、结果语义；专用 API/MCP 仍保持优先级。能否达到“绝大多数”只能由分类型实机矩阵测量，不能从 UIA 的覆盖叙述推断。

## 从只读观察到可写操作的准入条件

当前观察层在受信任应用范围内返回有界 UIA Control View；新增应用时明确告知窗口结构会交给当前模型，旧版记录须经一次性确认或重新保存才进入观察范围。`observe` 仍是旧记录升级的兼容标记，不是长期逐应用产品开关；全局“执行权限”也不扩大应用范围。只读观察不签发可写控件引用。用户另行启用默认关闭的 `fill` 后，只有完整快照里 AutomationId 唯一、可见、非密码、可写且支持 `ValuePattern` 的普通 Edit 才会得到五分钟内有效的不透明 `fieldRef`；截断快照不签发引用。引用只用于后端重新预检，不是行动许可，不含 UIA RuntimeId 或字段值。既有 `draft` 授权只对应原先保存的草稿输入框，不会自动升级为任意字段可写；按任务授权的单窗口截图仍未实现。

`draft` 与单步 `set_trusted_app_text` 共用后端可信预览：从持久待确认步骤读取精确正文，现场核对应用、窗口和控件，只向确认卡提供可读目标与 60 秒的一次性 `previewId`；模型参数不能充当可信预览。确认后仍须复核进程、窗口、控件身份与字段标识，再由 UIA `SetValue` 写入并回读。预检失败、预览过期或应用授权/参数变更不能批准。目标应用可能自动保存或同步，确认卡会提示；一旦写入可能发生而回读、取消或超时未证实，返回 `RESULT_UNKNOWN` 并停止自动续跑。按钮 `Invoke`、通用键鼠、截图后点击及业务完成判定均不在这条纵切内。

## 明确不能跨越的边界与验收

- 默认不提权、不启用 `uiAccess`。Microsoft 将该能力限定于满足签名、受保护安装位置等条件的辅助技术；普通程序无法访问更高完整性级别 UI，`uiAccess` 也不是 SYSTEM/UAC 安全桌面的通行证。[UIA 安全考虑](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-securityoverview)
- 执行器留在当前用户的交互式桌面，不搬入 Windows 服务的 Session 0；锁屏、UAC/凭据界面交还用户。[Interactive Services](https://learn.microsoft.com/en-us/windows/win32/services/interactive-services)、[Computer Use Windows 前台要求](https://learn.chatgpt.com/docs/computer-use)
- 验收至少覆盖 Win32/WPF、Electron/Chromium、自绘/画布、旧 MSAA、浏览器页面，以及多实例、重名控件、弹窗、切窗、缩放、多显示器、只读/密码、超时/取消、自动保存和不确定结果。按“观察正确、目标唯一、动作成功、业务核验、高风险停顿”分别统计，不把打开应用算作任务完成。

## 2026-10-01 通用界面操作：不以逐家服务适配为前提

讨论口径：这里的目标是**不为每个 SaaS、网站或桌面应用先写专用连接器**，不是排除 LLM，也不是要求离线。浏览器/Windows 的观察与执行可在本机完成；任务理解与视觉模型使用本地或云端，是独立的部署选择。以下是可行性依据与工程建议，不代表 AngelBot 已实现或已完成通用实机验收。

1. **网页主路径可复用浏览器级能力，而非逐家服务 API。** Playwright 按角色、名称、文本、标签定位页面元素，每次动作重新解析当前 DOM；iframe 需要明确进入对应 frame。官方 Playwright MCP 已提供面向 LLM 的结构化可访问性快照及浏览器操作，MCP 在这里是工具接口，不是每家 SaaS 的账号/API 连接器。工程判断：模型从当前页面识别目标，再使用共享的点击、填写、选择、滚动工具；可减少预先编写站点脚本，但不能承诺对所有网站无需任何兼容处理。[Playwright locators](https://playwright.dev/docs/locators)、[Frames](https://playwright.dev/docs/frames)、[Playwright MCP](https://github.com/microsoft/playwright-mcp)

2. **桌面主路径可复用 UIA 的语义与操作模式。** UIA 客户端从控件树读取元素属性，并通过 Invoke、Value、Selection、Scroll 等 control pattern 操作；是否暴露这些行为由目标应用的 provider 决定。工程判断：按控件能力组织执行器，不按软件名称硬编码；没有合适 provider 的自绘界面才进入视觉级别。[UIA 客户端](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-clientsoverview)、[UIA control patterns](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-controlpatternsoverview)

3. **视觉补齐“没有可操作语义”的部分。** Windows.Graphics.Capture 能抓取单个应用窗口，常规 picker 让用户选取并显示捕获边框；Win32 互操作也可从指定 HWND 创建 capture item，最低要求 Windows 10 1903。OmniParser 将截图解析为结构化界面元素，是可复用的 grounding 组件而非业务完成保证。工程判断：DOM/UIA 优先，OCR/截图定位作为显式授权的补充；HWND 接口可用不等于已获用户授权，截图及坐标只绑定本次许可的窗口/标签页、帧与几何状态，不借此采集全桌面。[屏幕捕获](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)、[CreateForWindow](https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nf-windows-graphics-capture-interop-igraphicscaptureiteminterop-createforwindow)、[OmniParser](https://github.com/microsoft/OmniParser)

4. **登录会话与用户接管仍须专门设计，但不必逐服务接 API。** Playwright MCP 支持独立持久 profile、隔离会话及通过浏览器扩展使用现有标签页/登录状态；多个实例不能同时占用同一持久 profile。Playwright 可通过 CDP 接已有 Chromium，但官方说明其连接保真度低于 Playwright 协议。工程判断：优先任务专用可见浏览器，由用户完成登录、MFA/验证码并随时接管；使用现有浏览器时，额外约束获准标签页/站点，不能把“接上一个浏览器”当作全 profile 授权。[Playwright MCP 会话](https://github.com/microsoft/playwright-mcp#user-profile)、[connectOverCDP](https://playwright.dev/docs/api/class-browsertype#browser-type-connect-over-cdp)

5. **共用安全闭环比“让模型连续点很多下”更重要。** Playwright 点击前检查唯一目标、可见、稳定、可接收事件及启用状态；Windows SendInput 受 UIPI 限制，只能向同等或较低完整性级别注入输入，Windows 也限制程序抢占前台。工程判断：一次有界动作后重新观察/核验，页面重渲染、窗口移动/失焦、用户操作或结果不明时使旧引用失效并停下；提交、删除、购买等后果性动作仍须按既有授权规则确认，不为扩大覆盖而默认提权。[Playwright actionability](https://playwright.dev/docs/actionability)、[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)、[SetForegroundWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setforegroundwindow)

6. **通用接口不是安全隔离边界。** Playwright MCP 明确声明自身不是安全边界，其 origin allow/block 设置也不阻止重定向；认证状态文件可能包含能冒用账号的 cookie/header。Chrome 136 起，默认用户数据目录不再接受 remote-debugging-port/pipe，官方建议使用非默认数据目录隔离调试。工程判断：不把用户主 profile 的调试端口整体开放给 agent；本地执行器仍需独立实现作用域、导航/弹窗检查、取消、敏感数据最小化与结果核验。动态 frame、自绘 canvas、语义缺失、登录挑战和系统安全界面，是覆盖率与接管需求，不能靠一次 API 接入消失。[Playwright MCP 安全](https://github.com/microsoft/playwright-mcp#security)、[Authentication](https://playwright.dev/docs/auth)、[Chrome 调试安全变更](https://developer.chrome.com/blog/remote-debugging-port)

7. **可以复用实现，但须按实际版本核对许可。** Playwright MCP 仓库标注 Apache-2.0；OmniParser 仓库 LICENSE 为 CC-BY-4.0，当前 README 说明新 icon_detect_v3 基于 MIT 许可的 YOLOv9 实现、旧 Ultralytics 检测器仍为 AGPL、caption 模型为 MIT。不能将代码、依赖和所有模型权重统称为同一许可；选择具体提交/模型版本后再核对其文件与分发条件。[Playwright MCP LICENSE](https://github.com/microsoft/playwright-mcp/blob/main/LICENSE)、[OmniParser LICENSE](https://github.com/microsoft/OmniParser/blob/master/LICENSE)、[OmniParser 模型许可说明](https://github.com/microsoft/OmniParser#model-weights-license)

结论（工程判断）：**通用浏览器执行器 + 通用 Windows 执行器 + 明确授权的视觉 fallback** 可以成为不依赖逐家 SaaS 适配的底座；真正的难点在跨步骤理解、屏幕目标定位、会话管理、动作后验证与安全接管。通用能力负责覆盖长尾，专用连接器仍可作为高频、高可靠任务的可选优化，二者不必互斥。

## 2026-10-01 UIA Invoke 单步操作的执行约束

以下区分 Microsoft 官方行为约定与 AngelBot 工程策略；支持 InvokePattern 不代表动作无风险，也不等于业务成功。

1. **只调用当前真实支持的 pattern。** Invoke 用于单一动作；选中、开关、展开属于其他 pattern，同一控件也可能有多种能力。工程策略：首批只接入支持 Invoke 的 Button、Hyperlink、MenuItem，动作前重新读取 pattern，不能将控件类型或观察时的能力当作执行时保证。[Control patterns](https://learn.microsoft.com/en-us/dotnet/framework/ui-automation/ui-automation-control-patterns-overview)、[Invoke provider 约定](https://learn.microsoft.com/en-us/dotnet/framework/ui-automation/implementing-the-ui-automation-invoke-control-pattern)

2. **标签不是身份或风险分级。** Name 可本地化且不唯一；AutomationId 仅在兄弟节点中区分且可能随版本改变；RuntimeId 也可能随时间复用，应视为不透明比较值。工程策略：短时引用绑定获准应用、进程、窗口及当前控件；确认预览和执行均重新核对唯一匹配，标签只用于展示，不因“打开”“确认”等字样推断低风险。[UIA 测试定位属性](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-usefortesting)

3. **provider 可以阻塞或拒绝调用。** Invoke 应立即返回，但实现可能因模态对话框而阻塞；不支持、隐藏/阻挡或禁用的控件可能抛出异常。工程策略：调用放在可终止的辅助进程，设置硬截止时间；预检拒绝与可能已发出的动作分别记录，不能将超时直接当作“未点击”。[InvokePattern.Invoke](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.invokepattern.invoke?view=windowsdesktop-9.0)、[原生 Invoke API](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationinvokepattern-invoke)

4. **UIA 调用线程遵循 COM MTA 指引。** Microsoft 建议在不拥有窗口的独立非 UI MTA 线程中查找元素及使用 pattern；创建 apartment 退出后，跨线程元素可能失效。工程策略：不跨辅助进程保留 COM 元素，只保存短时身份元数据并现场重找；Windows PowerShell 3.0 起默认 STA，辅助进程应显式传入 `-Mta`。[UIA threading](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading)、[PowerShell.exe 参数](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_powershell_exe?view=powershell-5.1)

5. **调用后目标消失可能是正常结果。** Microsoft 明确说明控件可在 Invoke 后立即离开 UIA 树，事件回调再读取它可能失败。工程策略：执行前缓存确认卡/审计需要的名称、角色与身份；调用后从获准窗口重新观察，不能把旧控件属性读取失败判定为动作未发生，更不能自动重试。[Invoke provider 约定](https://learn.microsoft.com/en-us/dotnet/framework/ui-automation/implementing-the-ui-automation-invoke-control-pattern)

6. **返回成功或 InvokedEvent 不是业务完成证据。** 原生 API 的 S_OK 表示调用成功；耗时或需用户参与的动作可以先发 InvokedEvent，之后才完成。工程策略：单步执行只返回 `dispatched`；调用已可能产生副作用而取消、超时、异常或结果收集失败时返回 `RESULT_UNKNOWN`，暂停并交给用户检查，不声称发送、保存或购买已完成。[原生 Invoke API](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationinvokepattern-invoke)、[InvokedEvent 时序](https://learn.microsoft.com/en-us/dotnet/framework/ui-automation/implementing-the-ui-automation-invoke-control-pattern)

7. **明确交互授权与确切单步确认仍不可省略。** Invoke provider 可以暴露重大副作用，HelpText 也只是应用提供的提示。工程策略：观察许可不自动升级为交互许可；默认关闭交互，启用后仍对具体控件使用后端可信预览和一次性确认，全局自动执行偏好不得代替这次决定。[Invoke 副作用说明](https://learn.microsoft.com/en-us/dotnet/framework/ui-automation/implementing-the-ui-automation-invoke-control-pattern)

验收策略（工程判断）：确定性替身覆盖禁用/失踪/重名/引用过期、确认后改名或换窗口、取消/超时及调用后树变化；另用隔离本地 fixture 验证真实 UIA。文档依据不替代实机验收，也不扩展到坐标点击、任意键盘、提权或安全桌面。

## 2026-10-01 UIA Select 与 Expand/Collapse 的有界契约

桌面响应性同样属于操作边界：[Tauri 官方文档](https://v2.tauri.app/develop/calling-rust/#async-commands) 说明同步 command 默认在主线程执行。AngelBot 的原生预检和确认执行均复用 `async_runtime::spawn_blocking`，避免 UIA 或外部工具等待冻结界面；核心审批实现仍共用原有可测试同步入口。

本节仍区分官方 API 语义与工程策略；新增选择/展开能力不扩大应用授权，也不引入逐家 SaaS 适配。下面第 3／4 项记录初期角色和隐私收敛策略；2026-10-20 经用户确认，已对启用控件操作的可信应用开放 ComboBox／ListItem 的短名称与对应动作，当前权限边界及验收以[统一路线图](../current-roadmap.md#32-模块进度与剩余边界)为准，不再把初期限制视作现行全部能力。

1. **Select 不是多选追加。** 对尚未选中的元素，`SelectionItemPattern.Select` 清除已有选择后选中目标；provider 约定目标已选中时不做操作。因此不能承诺重复 Select 会清除其他选项，也不能替代 `AddToSelection`/`RemoveFromSelection`。工程策略：回读同一目标当前 `IsSelected == true` 只记为“UI 选择状态已核验”，不声称业务完成或所有同组项目已核验。[Select API](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationcore/nf-uiautomationcore-iselectionitemprovider-select)、[IsSelected](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.selectionitempattern.selectionitempatterninformation.isselected?view=netframework-4.8.1)

2. **展开/收起可能阻塞，状态不是二值。** .NET 文档明确 `Expand`/`Collapse` 阻塞至操作返回；状态还包括 `PartiallyExpanded` 与 `LeafNode`。工程策略：预检拒绝当前 LeafNode；只把新鲜回读严格等于请求的 Expanded/Collapsed 记作 UI 状态核验，不能将 PartiallyExpanded 强转为 Expanded。对按需加载树节点的特殊 provider，保守拒绝是首批覆盖限制，不宣称 API 永远不可操作。[Expand](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.expandcollapsepattern.expand?view=netframework-4.8.1)、[Collapse](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.expandcollapsepattern.collapse?view=windowsdesktop-9.0)、[状态枚举](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationcore/ne-uiautomationcore-expandcollapsestate)

3. **按当前 pattern 能力选有限角色。** RadioButton 与 TabItem 通过 SelectionItem 选择；MenuItem 可以支持 ExpandCollapse。工程策略：首批 Select 仅支持已获准观察名称的 RadioButton/TabItem，Expand/Collapse 仅支持有名称且现场支持该 pattern 的 MenuItem；不把 Invoke、Toggle 或坐标输入当失败替代。旧 Win32 RadioButton 的 SelectionContainer 可能不可用，不能仅因此拒绝目标本身的合法选择。[RadioButton](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-supportradiobuttoncontroltype)、[TabItem](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-supporttabitemcontroltype)、[MenuItem](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-supportmenuitemcontroltype)

4. **保留观察层的既有隐私收敛。** ComboBox 官方 Name 约定为静态标签、不应携带当前内容；不能据此假定所有实际 provider 都遵约。工程策略：首批仍不开放当前被隐藏名称的 ComboBox/ListItem，不读取 Value/Text 或枚举选择值来补目标名；单个控件的布尔选择状态和展开枚举不等于取得列表内容的许可。未来扩展这两类角色须独立审查标签与内容披露。[ComboBox 属性约定](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-supportcomboboxcontroltype)、[ListItem 属性约定](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-supportlistitemcontroltype)

5. **状态与元素均动态，不能用缓存宣称核验。** 展开状态只描述直接子项的可见性；收起可销毁后代，按需生成的子项可能只在显示时存在。工程策略：动作后重新检查获准窗口和目标身份，再读取 `pattern.Current`，不读取观察时的 Cached 状态；原引用单次消费，下一步重新观察。RuntimeId 仅是可复用的不透明短期身份线索，不能充当永久对象 ID。[ExpandCollapse provider 约定](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-implementingexpandcollapse)、[UIA 身份属性](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-usefortesting)

6. **线程和不确定结果仍遵循同一执行边界。** UIA 查找、pattern 调用和回读都放入无 UI 窗口的 MTA 辅助进程；OS 调用不在数据库/确认预览锁内执行。工程策略：单步已可能产生副作用后，取消、超时、异常、目标/窗口变化或状态回读不符统一为 `RESULT_UNKNOWN`，停止自动续跑和重放；只在动作前确证拒绝时返回“未执行”。选择页签或单选项可能触发应用事件，不能依名称推断低风险，仍须精确操作绑定的单次确认。[UIA threading](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading)、[SelectionItem provider](https://learn.microsoft.com/en-us/dotnet/framework/ui-automation/implementing-the-ui-automation-selectionitem-control-pattern)

验收策略（工程判断）：覆盖同控件多 pattern、已选中/已展开、LeafNode/PartiallyExpanded、回读错态/失踪、禁用/密码/失焦、超时/取消、操作类型替换及确认后配置变更；隔离 WPF fixture 至少实测单选、切页、展开和收起。`verified` 限于目标 UI 状态，不代表发送、保存或最终任务已完成。

## 2026-10-03 UIA 小步纵向滚动：复用现有操作闭环

1. **步长由应用决定，不承诺固定像素或一整页。** `ScrollPattern.Scroll` 接受横向与纵向的 `ScrollAmount`；`SmallIncrement` / `SmallDecrement` 表示控件的小幅增减。没有横向滚动时传 `ScrollAmount.NoAmount`，不能混用仅供 `SetScrollPercent` 使用的 `ScrollPattern.NoScroll` 数值哨兵。工程策略：只暴露 `scrollup` / `scrolldown`，每次一次小幅纵向操作，不接收距离、次数、坐标或任意脚本。[Scroll 方法](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.scrollpattern.scroll?view=windowsdesktop-9.0)、[ScrollAmount 枚举](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.scrollamount?view=windowsdesktop-9.0)、[NoScroll 字段](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.scrollpattern.noscroll?view=windowsdesktop-9.0)

2. **可滚动容器不是扩大读取内容的许可。** 工程策略：在既有有界观察中只为具有有效短名称、可见、启用且现场支持垂直 ScrollPattern 的 Pane / Document / List 提供滚动引用；普通容器不因角色新增就挤占动作元数据预算。名称可能包含页面标题、文件名或应用提供的数据，不能视为无隐私的静态标签或可信指令；仍不读取 Value/Text、不枚举列表内容，也不扩大到未授权窗口。

3. **只核验方向或已触边，不核验业务完成。** `VerticalScrollPercent` 表示该元素内容区的纵向相对位置。工程策略：动作前后读取当前可滚动状态及有限的有效位置，只在请求方向得到有界证据，或动作前已处于该方向边界而无需调用时记作 narrow `verified`；不据此宣称已读完网页、已加载全部内容或完成任务。状态不可用、方向不符、动态内容使结果无法判断、取消或超时等可能越过副作用接缝的情况仍为 `RESULT_UNKNOWN`，不自动补滚或重放。[VerticalScrollPercent](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.scrollpattern.scrollpatterninformation.verticalscrollpercent?view=windowsdesktop-9.0)

4. **沿用同一授权、确认和清理，不再加一套工具。** 工程策略：继续使用 `operate_trusted_app_control`、现有 `interact` 授权与后端可信实时预览；整体完全访问偏好不能替代这次精确单步确认。引用绑定应用、进程、窗口、RuntimeId、名称、角色和操作类型，单次消费并使该应用旧引用失效；下一步必须重新观察。滚动可触发应用懒加载或事件，不能仅凭操作名推断没有后果，不提供键盘/坐标失败回退。

本轮范围与验收限制：仍保留完整快照才签发引用的规则，以及既有控件、字节、节点和深度预算。大量控件的网页、跨进程 provider、多个候选窗口、无有效名称或无 ScrollPattern 的应用可能保守拒绝；这不是通用浏览器内容读取或绝大多数网站任务已完备。确定性验证应覆盖 schema、实时授权撤销、动作替换、单次引用、正常方向、触边 no-op、异常或延迟回读与未知结果；隔离 WPF fixture 再核验真实 ScrollPattern 及独立位置回读，不能以替身通过替代实机证据。

## 2026-10-09 TogglePattern：check/uncheck 单步状态闭环

- **官方：没有直接设定状态的方法。** TogglePattern 只提供状态读取与 `Toggle()`；Microsoft 明确不提供 set-state，以避免绕过三态复选框的状态循环。不能把它当作幂等的 `setChecked(true)`。[Toggle provider 约定](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-implementingtoggle)、[客户端接口](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nn-uiautomationclient-iuiautomationtogglepattern)
- **官方：循环为 On → Off → Indeterminate（若支持）→ On。** 二态控件省略 Indeterminate；不能按枚举数值排序，或假定任意目标状态都只需调用一次。当前 .NET 客户端使用的 `TogglePattern.Toggle()` 也遵循此规则。[.NET Toggle 方法](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.togglepattern.toggle?view=windowsdesktop-9.0)、[Win32 Toggle 方法](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationtogglepattern-toggle)
- **实现策略：复用现有明确动作，不暴露 raw toggle。** `operate_trusted_app_control` 增加 `check` / `uncheck`，仅接受完整观察中启用、有有效名称、具备 `toggle` 的 `checkBox` 引用；仍使用 `interact` 授权和后端可信实时预览，精确绑定本次动作并单次消费，完全访问模式不绕过专项确认。不会借 Invoke、键盘或坐标失败回退。
- **实现策略：请求目标状态，但最多执行一次 Toggle。** 执行前重新读取状态，已经是请求的 On / Off 则不调用；当前为 Indeterminate 时在副作用前拒绝。否则只调用一次 `Toggle()`，随后仅有界回读。三态 provider 仍可能在这一次调用后到达 Indeterminate，此时结果为 `RESULT_UNKNOWN`，不补调用、不反向 Toggle；不声称支持三态状态编排。[官方状态循环约定](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-implementingtoggle)
- **实现策略：回读仅核验控件，不证明任务完成。** `S_OK` 只表示调用返回；可能发生副作用后取消、超时、目标变化、回读不可用或仍不吻合，都沿用未知结果并停下，不自动重放。成功回读只证明该控件的目标状态，后续任务须重新观察并获取新引用，不能宣称提交、购买或其他业务已完成。[Toggle 返回值](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationtogglepattern-toggle)

## 2026-10-09 密集窗口的分区观察：实现与受限验收

- **不增加工具或权限。** `observe_trusted_app_window` 在 controls 模式接受可选的不透明 `region_ref`；image 模式与分区组合必须拒绝。输出最多八个 Pane / Document / List / Group 只读导航建议，字段仅为 role、name、regionRef，不声称穷举分区；未命名布局的 name 可为空，仍绑定准确路径、角色、原始名称及运行时身份，不能猜测选择器。Group 不因此新增任何动作角色。候选不依赖 ScrollPattern，也不挤占动作元数据预算，仍计入原总输出字节预算。
- **分区引用不是写入许可。** 安全访问到的局部分支即使处于不完整的全窗口观察中，也可以给出只读建议；选定分区仍须完整观察其子树才签发 fieldRef / controlRef。不完整子树继续拒绝动作引用，不抬高既有控件、节点、深度和输出预算。选定分区根本身仍可作为合格控件观察，嵌套建议只指向其严格后代。
- **身份与路径留在后端。** 私有短时引用绑定应用、程序、进程、窗口、RuntimeId、名称、角色及真实 Control View 兄弟序号路径；路径不是模型提供的 selector。重新沿路径定位并核对祖先安全性和最终身份，改变或不可读即拒绝，不回退全窗口猜测；RuntimeId 仍只是可复用的短期比较线索。[UIA 身份属性约定](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-usefortesting)
- **范围进入现有动作租约。** FieldRefLease / ControlRefLease 复制私有不可变 scope，待确认参数与一次性不透明引用共同绑定该范围，无需向模型新增身份字段。字段预检与 SetText 必须实际在此范围内查找，不能保留全窗口 FindAll 后补筛选；副作用前和每次回读重新核对当前祖先归属、目标身份及安全属性。
- **失效与未知结果保持统一。** 每次动作使该应用全部字段、控件和分区旧引用失效；副作用后范围、目标或回读失效沿用 `RESULT_UNKNOWN`，不重新定位后自动重放。此处是范围收紧与导航能力，不是通用浏览器、跨进程控件或全部业务流程已经兼容。

确定性矩阵已覆盖 partial→只读分区→完整子树→限定范围动作、错误路径与祖先变化、越界目标、image 组合拒绝、单次引用失效及副作用后的身份丢失。既有隔离原生 WPF 旅程通过生产 adapter 和持久确认路径核验区域根滚动、密集分区的重复 ID 字段填写／勾选、可信区域标签、重排旧路径拒绝及重新观察恢复；独立读数证明其他区域未改变。离线 `quick`、`full` 与独立审查通过，具体门槛及数量见 [当前路线图](../current-roadmap.md)。这些证据不替代真实模型、浏览器或第三方应用的业务验收。

## 2026-10-20 Playwright MCP：复用 stdio 的独立网页会话（调研，非验收）

结论：不必把 Windows UIA 扩成 DOM 执行器。可先复用既有 MCP 配置、工作区准入、确认及未知结果处理，以官方 Playwright MCP 启动专用可见 Edge；这不是接管用户已有浏览器，也不是给第三方服务增加系统沙箱。本节核对已发布 `v0.0.83`，不把持续变化的 `main` 文档当作版本契约。[官方发布](https://github.com/microsoft/playwright-mcp/releases/tag/v0.0.83)

1. **运行要求应写 Node ≥20，不仅是顶层声明的 ≥18。** `@playwright/mcp@0.0.83` 固定依赖 `playwright` / `playwright-core@1.64.0-alpha-1790635538000`；对应源码提交 `e8149b8257d32dcf8f72573ecc43e72439da7080` 的依赖自身要求 ≥20。固定版本有利于发现工具 schema 变化，不建议产品草稿用 `@latest`。[MCP 包定义](https://github.com/microsoft/playwright-mcp/blob/v0.0.83/package.json)、[实际依赖要求](https://github.com/microsoft/playwright/blob/e8149b8257d32dcf8f72573ecc43e72439da7080/packages/playwright-core/package.json)

2. **最小配置建议如下，仍由用户明确添加／准入／连接，不默认启动。** Windows 使用 `npx.cmd`；`msedge` 选择已安装 Edge 的 Chromium channel，而不是下载另一套个人浏览器；Windows 默认为可见窗口。没有安装 Edge、Node 或 npm 获取失败时应报告具体缺项，不能偷偷改装浏览器。[CLI 参数源码](https://github.com/microsoft/playwright/blob/e8149b8257d32dcf8f72573ecc43e72439da7080/packages/playwright-core/src/tools/mcp/program.ts#L34)、[channel 解析与 Windows sandbox 默认值](https://github.com/microsoft/playwright/blob/e8149b8257d32dcf8f72573ecc43e72439da7080/packages/playwright-core/src/tools/mcp/config.ts#L224)

   ```json
   {
     "command": "npx.cmd",
     "args": [
       "--yes", "@playwright/mcp@0.0.83",
       "--browser", "msedge",
       "--isolated", "--no-webmcp",
       "--image-responses", "omit"
     ]
   }
   ```

3. **隔离只针对浏览器会话状态。** `--isolated` 启动自有浏览器并创建非持久 context；不加 `--extension`、`--cdp-endpoint`、`--endpoint` 或个人 `--user-data-dir`，不会以这些接管方式连接已有标签页。`isolated` 与 `user-data-dir` 同时提供会明确报错；关闭／重新连接不能承诺保留登录、标签页及操作进度。若日后需要记住登录，应另行确认专用持久 profile 或 `storage-state` 文件的保存、加密备份、清理策略；不是读取个人 profile。持久 profile 同目录不能并发使用。可见浏览器默认无 idle 自动关闭，故 AngelBot 断连／退出的自有子进程清理仍须实测。[浏览器创建与所有权](https://github.com/microsoft/playwright/blob/e8149b8257d32dcf8f72573ecc43e72439da7080/packages/playwright-core/src/tools/mcp/browserFactory.ts#L53)、[context 与释放](https://github.com/microsoft/playwright/blob/e8149b8257d32dcf8f72573ecc43e72439da7080/packages/playwright-core/src/tools/mcp/program.ts#L101)、[profile 与初始状态说明](https://github.com/microsoft/playwright-mcp/blob/v0.0.83/README.md#user-profile)

4. **不能把预设称为“受限网页沙箱”。** 默认 core 包含 `browser_run_code_unsafe`，能在服务进程执行任意 JavaScript；也包含页面 `browser_evaluate`、上传等能力。该版本 `filteredTools` 无条件包含 core，正式 CLI／config 没有按工具排除它们的选项，`--caps` 只增加能力。`--no-webmcp` 关闭网页自行注册的动态工具，不会移除 unsafe；`--image-responses omit` 不向模型返回图像，但不禁止 DOM 私人内容读取或截图落盘。沿用“完全启用即信任该 MCP 服务”的既有产品边界时，必须在连接前明确其范围；不能用提示词限制冒充执行隔离。[默认工具筛选](https://github.com/microsoft/playwright/blob/e8149b8257d32dcf8f72573ecc43e72439da7080/packages/playwright-core/src/tools/backend/tools.ts#L83)、[unsafe 实现](https://github.com/microsoft/playwright/blob/e8149b8257d32dcf8f72573ecc43e72439da7080/packages/playwright-core/src/tools/backend/runCode.ts#L37)、[配置类型](https://github.com/microsoft/playwright/blob/e8149b8257d32dcf8f72573ecc43e72439da7080/packages/playwright-core/src/tools/mcp/config.d.ts)

5. **origin、host、文件限制不是同一边界。** `allowed-hosts` 检查 HTTP MCP 服务收到的 Host，目的是 DNS rebinding 防护，并非目标网站许可；stdio 不需要开放该 HTTP 端口。`allowed-origins` / `blocked-origins` 是请求便利护栏，官方明确不构成安全边界、不约束重定向。默认文件路径与 `file:` 导航限制也被官方定性为可绕过的便利护栏；不要默认开启 `--allow-unrestricted-file-access`、`--no-sandbox`、忽略 TLS 或远程端口。普通网页网络请求会离开机器，读取到的内容会进入配置的模型，不能把“本地浏览器”描述成“所有数据都在本地”。[HTTP Host 检查](https://github.com/microsoft/playwright/blob/e8149b8257d32dcf8f72573ecc43e72439da7080/packages/playwright-core/src/tools/utils/mcp/http.ts#L61)、[版本化参数与安全说明](https://github.com/microsoft/playwright-mcp/blob/v0.0.83/README.md#configuration)、[文件检查实现](https://github.com/microsoft/playwright/blob/e8149b8257d32dcf8f72573ecc43e72439da7080/packages/playwright-core/src/tools/backend/context.ts#L494)

6. **annotations 只供提示，不证明业务安全或完成。** 官方对 readOnly／assertion 设置 `readOnlyHint=true`，其余设置 `destructiveHint=true`；所有工具 `openWorldHint=true`，未给出 `idempotentHint`。不能据此辨认“填写草稿”与“购买／发送”，仍复用 AngelBot 执行前检查、真实工具身份、确认和结果不明即停的链路；网站内容／工具说明不是授权。初步验收应先用自有网页证明导航→观察→填选→核对→原会话交付及关闭／重启，不做账号登录或真实外部提交，也不由这个结果宣称大多数网站、CAPTCHA、浏览器原生弹窗、下载上传和多代理并发已完备。[annotations 映射](https://github.com/microsoft/playwright/blob/e8149b8257d32dcf8f72573ecc43e72439da7080/packages/playwright-core/src/tools/utils/mcp/tool.ts#L29)

本节未安装软件、未运行浏览器、未修改用户 MCP 配置；连接与真实任务结果以 [当前路线图](../current-roadmap.md) 的后续分层验收为准。需要产品决定的是未来是否保存专用登录状态、是否允许既有会话接管，以及是否要求真正收窄服务进程／工具权限；当前一次性独立会话可以先行，不必为这些可选能力新增并行执行框架。

## 2026-10-20 宿主窗口与控件身份

真实计算器暴露了可见窗口但 `MainWindowHandle=0` 的兼容问题。Windows 8 以后 [`EnumWindows`](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enumwindows) 只枚举桌面应用顶层窗口；[`EnumChildWindows`](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enumchildwindows) 才会递归访问宿主内的子窗口。修复复用已有 Win32 枚举，只为控件保留准确可信进程拥有的最外层子 HWND；不按宿主名称或窗口标题授权，不扩大截图目标。

[`GetWindowThreadProcessId`](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowthreadprocessid) 可重新核对 HWND 的实时所属进程，无效句柄返回零。区域、预览、动作前后均据此核对 PID，再检查路径和 UIA 身份；发现变化时维持拒绝执行／结果未知语义。

用户确认后，结果读取复用原控件观察：仅在明确选择的区域内读取可见 `ControlType.Text` 的短 Name（最多 80 字），不读 Value/Text patterns，不截短后冒充完整结果；静态文字没有动作能力或引用。检查祖先归属、同进程、可见性和密码属性，并排除编辑框及选择字段的后代；不完整观察仍不产生动作引用。整窗结构发现不返回静态正文，不新增点击、填写或截图权限。UIA 元数据由应用提供，文字仍是不可信资料而非指令、授权或业务成功证明；真实验收与剩余缺口仅维护在 [当前路线图](../current-roadmap.md#34-当前有效证据)。

## 2026-10-21 一次确认的下拉选择

用户已允许 interaction 范围内读取 ComboBox／ListItem 的可见短名称，取代前文“首批隐藏这两类名称”的临时限制；无 interact 的应用仍隐藏，字段 Value/Text、密码和隐藏内容仍不读。真实 Main 的分步展开／选择因批准时失焦收起而失败，因此复用 `operate_trusted_app_control` 增加 `choose`，不增加工具或权限层。

一次批准绑定精确 ComboBox 引用及选项（最多 80 个 UTF-16 单元、无控制字符或首尾空白）。执行时复核身份与范围，展开，在该下拉框中以 ordinal 名称匹配唯一可见项，通过 [SelectionContainer](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.selectionitempattern.selectionitempatterninformation.selectioncontainer?view=windowsdesktop-10.0) 及有界祖先身份确认归属，至多 Select 一次，核验实际选择并收起；已选中不重复 Select。后续只比较此前可见选项的 RuntimeId，不读取收起后选项的 Name。导航开始后失败均保留未知结果，不清理性地盲点、猜坐标或重试；既有 `select` 保留给列表、单选和页签。

成功回执的 `choice={controlName,option,expanded:false}` 仅回显已批准且通过原生身份／状态核验的数据，不证明业务完成。获准的可见 ListItem 可附 `state.selectionContainerRole=list|comboBox`，只读容器类型、不读额外名称；不同容器的同名项不能作为选择失败证据。参数变更、授权撤销、旧预检／引用及单次消费沿用原链路，真实模型与原生／离线证据分别记入当前路线图，不从 WPF 一例推断任意应用兼容。

## 2026-10-24 Codex 主源复核：复用接缝，不引入另一套 harness

本节仅调研，不新增工具、启动应用／模型或改变权限。先检查了本机常用参考位置和既有调研，未找到可直接读取的 Codex 源码 checkout；因此在线核对 `openai/codex` 提交 [`398500106707f0a3217c7cf83edc2bf226cc342f`](https://github.com/openai/codex/commit/398500106707f0a3217c7cf83edc2bf226cc342f)（2026-10-24 00:18 北京时间），补充 [10 月 7 日的既有接缝调研](codex-daily-capability-reference-2026-10-07.md)，不把重复调研视为新实现或验收证据。

**公开范围：**官方确认 CLI、app-server 和 SDK 是开源组件。当前仓库也公开了 Windows computer-use 的 AUMID、publisher／product／binary 权限配置及活动展示，但这些配置不是 Windows 控件／截图／键鼠执行器。本次未找到能直接移植的完整 Windows computer-use helper，不能将桌面端实际控制能力统称为开源实现；也不能由配置字段推断它如何强制执行权限。[官方开源 harness 说明](https://developers.openai.com/blog/codex-as-a-platform)、[权限配置源码](https://github.com/openai/codex/blob/398500106707f0a3217c7cf83edc2bf226cc342f/codex-rs/config/src/computer_use.rs#L9)、[活动展示源码](https://github.com/openai/codex/blob/398500106707f0a3217c7cf83edc2bf226cc342f/codex-rs/tui/src/history_cell/computer_activity.rs)

只选择以下三个可用于当前缺口的模式；AngelBot 对应项为工程建议，而非 Codex 行为保证：

1. **稳定工具身份与实际执行器绑定，动作默认串行。** `ToolExecutor` 共同提供 spec、暴露方式和 handle，并默认不允许并行工具调用；宿主可以施加当步策略。`Deferred` 只表示不把完整工具声明放进模型初始清单，仍可后续发现，**不等于按需启动或零后台开销**。AngelBot 应复用既有 registry、权限／预览与 DesktopAdapter；视觉补充、弹窗兼容不另建 runner，同一交互窗口的动作不得因模型支持并行就同时执行。[ToolExecutor 与 Deferred](https://github.com/openai/codex/blob/398500106707f0a3217c7cf83edc2bf226cc342f/codex-rs/tools/src/tool_executor.rs#L49)、[默认串行与 handle](https://github.com/openai/codex/blob/398500106707f0a3217c7cf83edc2bf226cc342f/codex-rs/tools/src/tool_executor.rs#L101)

2. **资源所有权、停止请求与确认结束分开。** Codex 的 MCP shutdown 先将 client 置为 Closed，再终止自己拥有的 stdio 进程；统一进程另行区分 interrupt、terminate 与 terminate_confirmed。它们不证明第三方程序的业务已撤销，也不证明所有 MCP 服务会自动闲置退出。AngelBot 已有工具目录缓存、并发按需启动合并及 stop／stop_all，应在这些接缝上加固退出和迟到响应，而非再加常驻服务；截图／UIA helper 只在获准调用内持有资源，不做空闲轮询截图。停止后原程序是否仍在运行、执行后果是否未知，要分别回报。[MCP 资源释放](https://github.com/openai/codex/blob/398500106707f0a3217c7cf83edc2bf226cc342f/codex-rs/rmcp-client/src/rmcp_client.rs#L1088)、[中断与确认终止](https://github.com/openai/codex/blob/398500106707f0a3217c7cf83edc2bf226cc342f/codex-rs/core/src/unified_exec/process.rs#L243)

3. **单次终态是共享生命周期约束，不是每个附属模块的状态机。** Codex 的 `notify_tool_finish_if_unclaimed` 用原子闸防止同一调用重复发布终态。AngelBot 可沿用既有 task／run 标识和终态收口，让 computer-use 取消、迟到回执、自动化结果及主对话通知引用同一事实；不把迟到成功覆盖取消，也不由“进程已启动”生成业务完成通知。该模式只约束本地收口，不能冒充外部副作用 exactly-once 或可靠投递协议。[单终态源码](https://github.com/openai/codex/blob/398500106707f0a3217c7cf83edc2bf226cc342f/codex-rs/core/src/tools/registry.rs#L985)

精简与性能边界（工程判断）：可选能力保留明确入口，但未启用时不创建辅助进程、捕获会话、模型调用或新轮询器；启用后复用现有调度和权限链，资源随拥有它的调用／会话释放。用“未启用无启动、并发首次调用只启动一次、取消后无新动作、断连清理、迟到结果不重复通知”的最小回归证明边界；没有测量前不宣称零性能影响。当前只参考设计、未复制源码；若日后直接复用 Codex 代码，仍需遵守其 Apache-2.0 及适用 NOTICE，不能因 AngelBot 使用 MIT 而忽略第三方声明。[Codex LICENSE](https://github.com/openai/codex/blob/398500106707f0a3217c7cf83edc2bf226cc342f/LICENSE)、[NOTICE](https://github.com/openai/codex/blob/398500106707f0a3217c7cf83edc2bf226cc342f/NOTICE)

### 已确认的单窗口视觉点击边界

用户允许对 observe＋observeImage＋interact 的可信应用提议一次精确左键点击，所有整体权限下仍须逐次批准。`observe_trusted_app_window(mode=image)` 返回短期 imageRef 与原尺寸，像素仅作当次模型图像输入；`click_trusted_app_image` 不接受桌面坐标或自由键鼠指令。宿主预检重新捕获同一窗口并比对客户区像素，确认卡显示瞬态图像和标点；原 pending step、应用版本、引用、坐标和一次预检共同绑定批准。执行前再次核对，导航／缩放／客户区变化停止。引用只缓存有界元数据／摘要、不缓存像素，旧引用按应用失效，最多 8 个／5 分钟，懒清理而非定时器。没有可核对映射时仍可只读观察，但 imageRef 为 null。

Win32 执行复用原 DesktopAdapter 串行门禁和 WGC 一次性 helper，不新增服务或会话。只接受未缩放图像与 DWM frame 尺寸一致、客户区在 frame 内的情况；[CaptureItem.Size](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscaptureitem.size?view=winrt-26100)／[ContentSize](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframe.contentsize?view=winrt-26100) 与 [DWM 物理边框](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowrect)并不保证同一像素原点，因此“尺寸一致”只是保守准入，不能代替独立色块中心的原生验收。客户区坐标临时使用 Per-Monitor V2 DPI，上下文随后恢复；缩放、超限或不兼容窗口不发输入。

确认后仅允许从 AngelBot 自身或目标窗口交接焦点，[SetForegroundWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setforegroundwindow) 被系统拒绝即停止，不抢占其他应用。焦点变化后再次捕获核对，点击前检查前台、[GUI 线程鼠标捕获／菜单／移动状态](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-guithreadinfo)、用户按键及点下根窗口。[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput) 一次提交 MOVE／DOWN／UP；插入数异常保留 RESULT_UNKNOWN，仅在自身 DOWN 已插入时释放按键，不重试点击。派发不是业务成功；WindowFromPoint 和实时检查不是操作系统强隔离，仍存在检查到使用的竞态与 UIPI／焦点限制。10 月 25 日已完成自有窗口的原生色块中心单击和防重放验收，不是实际 Main／前端确认后的焦点交接或任意应用兼容证明；仍未支持键盘、拖拽、双击或全桌面。详细验收与剩余边界只维护于统一路线图。
