# Windows computer use：官方边界与 AngelBot 最小落地（2026-09-30）

范围：Windows 桌面当前用户的交互式会话；这是对 [Windows 应用控制研究](windows-application-control.md) 的实施核验，不承诺通用 RPA，也不是已完成的实机兼容性报告。以下「官方事实」均来自 Microsoft 文档；「工程判断」是对 AngelBot 当前代码的推论。

## 官方事实

| 边界 | 核验结果 | 对实施的含义（工程判断） |
| --- | --- | --- |
| UI 元素 | UI Automation 通过目标应用的 provider 暴露元素树、属性和 control pattern；某个控件是否可操作取决于它实际提供的 pattern。`AutomationId` 只在兄弟元素间唯一、可随应用版本变更；`Name` 可本地化且不唯一。深搜整个桌面可能遍历大量元素。[UIA client](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-clientsoverview)、[定位属性](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-usefortesting)、[元素获取](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-obtainingelements) | 先绑定用户选定的窗口、进程和具体容器，再做有界 inspect；0 个或多个匹配都失败。不要将 `Name` 或坐标当跨应用稳定 ID。 |
| 执行与验证 | `ValuePattern` 并非所有编辑控件都支持，且控件可能只读；密码控件不能向 UIA client 暴露值。UIA 有事件，但并非所有 provider 都发出每种属性变化事件。多属性读取应使用缓存；涉及自身 UI 的 UIA 调用应放在独立 MTA 线程，避免 UI 线程卡住。[Edit control](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-supporteditcontroltype)、[事件](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-eventsforclients)、[缓存](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-cachingforclients)、[线程](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading) | 每次动作后主动回读目标状态；事件只辅助等待，不把命令返回或事件缺失等同于业务成功/失败。 |
| 输入注入 | `SendInput` 受 UIPI 限制，只能注入同等或更低 integrity level 的目标；失败的返回值和 `GetLastError` 不能明确指出 UIPI 是原因。[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput) | 不以键鼠/坐标作为 UIA 失败时的静默兜底，尤其不能用于发送、购买、删除等不可逆动作。 |
| 高权限与安全桌面 | 普通应用不能读取/控制更高完整性级别的 UI；`uiAccess` 有辅助技术用途、签名和受保护安装位置等约束，即使启用也不能普遍进入 SYSTEM 的 UAC 桌面。Windows 的 Default、ScreenSaver、Winlogon 是不同 desktop，锁屏/UAC 时输入 desktop 可切换。[UIA 安全](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-securityoverview)、[Windows desktops](https://learn.microsoft.com/en-us/windows/win32/winstation/desktops) | AngelBot 应返回「需要用户在系统界面完成」而非提权或绕过；只在已解锁的当前交互式桌面执行。 |
| 进程与会话 | Windows 服务默认使用非交互 window station；Microsoft 明确不建议新代码采用交互式服务，建议用户会话中的单独应用通过 IPC 协作。[Interactive Services](https://learn.microsoft.com/en-us/windows/win32/services/interactive-services) | 将来若拆后台服务，computer use executor 仍必须留在用户交互会话，且 IPC 需鉴权；不能把前台操作搬到 Session 0。 |
| 截屏 | `Windows.Graphics.Capture` 支持窗口/显示器帧；正常流程调用系统 picker 由用户选择捕获对象，系统在活动捕获对象周围显示提示边框。[Screen capture](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture) | 截屏是显式、可停止的辅助观察能力，不等于 UIA 的语义操作权限；默认不常驻、不自动截全屏、不持久保存帧或把帧送给模型。 |

## 当前实现与差距

- 已有合理窄边界：[`DesktopAdapter`](../../src-tauri/src/desktop_control.rs) 只接受结构化 `OpenApp` / `OpenSettings` / `RevealPath` / `PrepareDraft`；[`handlers`](../../src-tauri/src/agent/tools/handlers.rs) 从受信配置解析可执行文件和 selector，而非让模型提供脚本。`PrepareDraft` 对单个非密码 `Edit` 做 `ValuePattern.SetValue` 并回读，不自动发送。设置侧只登记 `launch` / `draft`。[配置校验](../../src-tauri/src/commands/desktop.rs)
- 可用性边界：设置页已能对已保存的受信应用只读查找可填写的普通输入框，只返回名称、不返回输入值；选择后仍需显式保存。运行时只接受唯一的该 EXE 主窗口和唯一名称。它仍不能识别当前聊天对象、邮件收件人、账号或网页语义，也没有持久的窗口/PID/HWND 绑定；用户须先切到正确页面，并在填写后核对。以上是代码边界，不是已完成的跨应用实机兼容性结论。
- 安全边界：桌面辅助进程现在只继承有限的启动环境，避免默认继承 AngelBot 模型密钥；`prepare_draft` 中断、超时以及写入后未能验证时须把结果标为未知并停止自动续跑。草稿正文仍作为专用环境变量交给一次性 PowerShell 辅助进程，目标应用也可能自动保存或同步；环境隔离不是操作系统沙箱，不能据此保证内容不被本机同权限进程或目标应用获取。
- 验证缺口：现有 deterministic mock/前端测试能证明策略与调用链，却不能证明真实目标应用的 UIA provider、多个窗口、弹窗、只读控件、锁屏/UAC 等行为。发布门槛需要独立 Windows 实机/VM smoke 矩阵，且不得用真实个人消息或密钥作测试资料。

## 最小分阶段能力建议（工程判断）

1. **先守住现有路径（已实现最小闭环，待实机验收）**：隔离 PowerShell 环境、限定辅助进程时长和输出；区分 `dispatched / verified / result_unknown`，不要将“已打开/已填写”冒充“任务完成”。保持 `DesktopAdapter` 与现有确认边界，不新增模型可调用的通用 PowerShell、任意 selector、键鼠或截图工具。
2. **只读发现 + 用户选目标（已实现名称选择，窗口身份待深化）**：设置页按受信 EXE 列出当前唯一主窗口内有限数量的非敏感控件名称，不取密码值。后续若需要指定会话或收件人，必须先引入可验证的窗口/对象身份，再考虑绑定 HWND、容器、应用版本/locale 和失效重标定；不能把现有 `Name` 选择误称为收件对象识别。
3. **两种可验证动作**：首先深化 `prepare_draft`；其次只为明确控件增加一个局部 `Invoke/Selection` 动作，默认只用在可撤销的本地步骤。前后抓取目标状态，用户界面展示“准备了什么、下一步需你做什么”。外部发送/购买/删除仍单独确认，不能因草稿权限升级。
4. **再评估视觉后备**：仅当代表应用的 UIA 测试证明缺口不可避免时，加入系统 picker 驱动的单窗口、短时截图；帧默认内存处理并在停止/会话结束清理。视觉只能辅助识别与向用户解释，不自动获得点击/输入权限。锁屏、UAC、提升窗口直接暂停并交还用户。

验收样本建议至少覆盖 Win32/WPF、Electron、浏览器富文本框各一项，并分别验证正常、多个实例、控件改名、只读、密码、弹窗、锁屏/UAC、用户中途切窗、取消和超时。通过率要按场景报告，不能以「可打开应用」推断「可完成任务」。
