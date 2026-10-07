# 受控单窗口观察：实现选择与架构边界

日期：2026-10-07（Asia/Shanghai）。前半记录是源码与官方文档调研，后半记录本轮落地与验证。编译和合成测试不等于真实窗口捕获验收；本轮不抓取用户窗口、不修改真实授权配置、不调用真实模型。

## 结论

建议在已有 `DesktopAdapter` 平台接缝内使用 Windows.Graphics.Capture（WGC）的单 HWND 捕获，优先复用 `windows-capture =2.0.1`，不将 PrintWindow 当作通用截图后端。捕获是一次有预算的只读工具操作；图像继续使用现有瞬时 `ToolImage` 路径进入 Main Agent，而不是建立视觉 Agent、独立路由、截图历史库或坐标点击兜底。

严格的取消与清理需要一次性、宿主拥有的捕获 helper 进程：该库的初始化和停止包含无超时的等待，不能只在父任务中包一层 timeout 然后遗留捕获线程。可以复用现有最小子进程环境、平台串行 gate 和取消/回收原则；不应继续扩大固定 PowerShell 脚本去手写整套 WGC/D3D11。[库捕获实现](https://docs.rs/crate/windows-capture/2.0.1/source/src/capture.rs)

以下框架不变：用户只与 Main Agent 对话；`ForegroundToolSurface` 仍是工具组装入口；应用范围与整体操作权限仍是授权依据；截图判断不是点击授权，图片内文字不是新的用户指令。日常/项目工作区、委派子代理、既有执行未知结果暂停机制不需要重写。[现有工具入口](D:/Projects/desktop/AngelBot/src-tauri/src/commands/foreground_tool_surface.rs)、[桌面适配器](D:/Projects/desktop/AngelBot/src-tauri/src/desktop_control.rs)、[上一阶段图像桥](D:/Projects/desktop/AngelBot/docs/research/tool-image-bridge-2026-10-07.md)

## Codex 可以借鉴什么

继续核对与上一份报告相同的公开提交 `5a3140176e668a2f72f3c098490eb7f7052d9d85`，没有改用 main 分支，也没有把未公开的桌面 helper 当成可移植源码。这里借鉴接口与生命周期，不复制源码。

- `FunctionToolOutput` 的模型输出为 typed content，日志输出只做文本投影；`FunctionCallOutputBody` 可表达字符串或内容数组，文本投影明确会丢弃图像。AngelBot 现有瞬时图像链应继续承担这个分离，不让截图 base64 进入 UI、摘要或持久化对话。[工具输出](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/core/src/tools/context.rs#L238)、[协议投影](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/protocol/src/models.rs#L2067)
- 工具终态由宿主持有；`notify_tool_finish_if_unclaimed` 使用原子交换防止重复终态。不能让取消后的迟到图像成为新的成功结果，也不能因为拒绝已经执行完成的结果而重做外部动作。[宿主状态](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/core/src/tools/context.rs#L44)、[唯一终态与后置 hook](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/core/src/tools/registry.rs#L723)
- Codex 区分结束进程、取消输出任务、确认终止。AngelBot 应同样区分“请求取消”与“helper 已退出并回收”，不把 future 被丢弃等同资源结束，也不推导远端副作用已撤销。[进程终止实现](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/core/src/unified_exec/process.rs#L220)

该提交 MCP 有非空 `structuredContent` 时可以优先输出文本；它不是所有图像无损回传的保证。本轮仍沿用 AngelBot 自己已验证的有界 PNG 契约。[MCP 结果转换](https://github.com/openai/codex/blob/5a3140176e668a2f72f3c098490eb7f7052d9d85/codex-rs/protocol/src/models.rs#L2172)

## WGC 与 PrintWindow 的选择

| 方案 | 官方/源码事实 | 对 AngelBot 的取舍 |
| --- | --- | --- |
| WGC `CreateForWindow` | 明确目标为一个 HWND；该互操作接口最低 Windows 10 1903。`IsSupported` 仍需运行时检查。 | 使用已经核对可执行路径、PID 与唯一窗口的 HWND；不调用 monitor 捕获，不按标题模糊查找。 |
| WGC 帧 | 帧池 surface 大小不等于有效 `ContentSize`；超出有效区域的内容可能未定义，改变窗口大小需要重建帧池。 | 只返回有效帧内容，先检查尺寸/像素预算；不发送未初始化填充区。HDR 的色彩准确性需要实机验证。 |
| PrintWindow | 目标进程处理 WM_PRINT/WM_PRINTCLIENT 并绘制；调用同步且可能长时间阻塞。 | 不作为默认通用后端。返回非零不证明浏览器/GPU 内容一定完整或新鲜；失败不能改为桌面 BitBlt。 |

以上分别来自 [CreateForWindow](https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nf-windows-graphics-capture-interop-igraphicscaptureiteminterop-createforwindow)、[微软截图与帧生命周期指南](https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/screen-capture)、[PrintWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-printwindow)。选择 WGC 的覆盖性更合适是工程推断，不是微软保证所有 Windows 程序均可捕获。

## 最小成熟库接入

候选是 [NiiightmareXD/windows-capture](https://github.com/NiiightmareXD/windows-capture)，核对固定 crates.io 版本 `2.0.1`，发布源码记录的提交为 `c7d106448eb9d9b251345c39047711e1cd408ae2`。许可证 MIT，分发时需保留相应版权与许可；本次只引用依赖接口，没有复制实现。[版本清单](https://docs.rs/crate/windows-capture/2.0.1/source/Cargo.toml)、[发布提交](https://docs.rs/crate/windows-capture/2.0.1/source/.cargo_vcs_info.json)、[许可证](https://docs.rs/crate/windows-capture/2.0.1/source/LICENCE)

可复用的接口：

1. `Window::from_raw_hwnd(*mut c_void)`；转换为 `GraphicsCaptureItemType::Window` 实际调用 WGC `CreateForWindow`。它接收句柄，不验证该句柄是否属于 AngelBot 获准应用，授权与重核仍在宿主。[window.rs](https://docs.rs/crate/windows-capture/2.0.1/source/src/window.rs)
2. `GraphicsCaptureApiHandler` 的 `on_frame_arrived(&mut Frame, InternalCaptureControl)` 获得第一帧；复制到自有 CPU 数据后请求停止。`Frame::buffer()` 与 `as_nopadding_buffer()` 处理 mapped staging texture 与行填充，可交给仓库已有 `png` 编码器，不使用 `save_as_image()` 落盘。[capture.rs](https://docs.rs/crate/windows-capture/2.0.1/source/src/capture.rs)、[frame.rs](https://docs.rs/crate/windows-capture/2.0.1/source/src/frame.rs)
3. 维持系统默认捕获边框；不请求无边框权限。明确不启用 secondary windows。`SecondaryWindowSettings::Exclude` 在属性不可用时会报错，不能无条件设置。微软说明 `IncludeSecondaryWindows` 从 Windows 11 24H2 提供且默认 false，因此旧系统必须按能力检测使用 Default，支持时使用 Exclude，绝不使用 Include。[settings.rs](https://docs.rs/crate/windows-capture/2.0.1/source/src/settings.rs)、[库能力检测](https://docs.rs/crate/windows-capture/2.0.1/source/src/graphics_capture_api.rs)、[微软 secondary windows 属性](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.includesecondarywindows)
4. `GraphicsCaptureApi` 的 Drop 清除事件订阅并关闭 frame pool/session；mapped staging texture Drop 执行 Unmap。外部 `CaptureControl::stop()` 仍会 join，初始化的 `recv()` 也无期限；这些正常资源回收路径不构成硬超时承诺。helper 隔离后，父进程可在到期/取消时 kill 并 wait，丢弃尚未接受的图像。[WGC cleanup](https://docs.rs/crate/windows-capture/2.0.1/source/src/graphics_capture_api.rs)、[D3D RAII](https://docs.rs/crate/windows-capture/2.0.1/source/src/d3d11.rs)、[捕获控制](https://docs.rs/crate/windows-capture/2.0.1/source/src/capture.rs)

这不是“零成本已有依赖”：2.0.1 使用 edition 2024、`windows 0.62.2`、`windows-future 0.3.2`、parking_lot 与 rayon。AngelBot 当前锁文件存在 `windows 0.61.3` 与多个 windows-core/windows-sys 版本，但没有 windows-capture；直接依赖需要核对新增包、离线锁定、许可证与发布产物。当前本机 rustc 为 1.96.1，只证明本机 edition 支持，不能替代 CI 编译验收。[固定版本 manifest](https://docs.rs/crate/windows-capture/2.0.1/source/Cargo.toml)、[AngelBot manifest](D:/Projects/desktop/AngelBot/src-tauri/Cargo.toml)

## 安全与兼容迁移

现有设置明确承诺“不截图”，已有 `observe` 授权不能静默扩大为可上传截图。建议复用现有“展示精确应用 id/exe → 用户确认 → 事务重核 → 更新 revision”的确认流程；使用内部观察范围语义版本区分旧范围与已确认新范围，而不是新增用户逐应用截图开关。旧控件观察继续可用，image 模式要求新版范围。只改说明文字、或因 FullAccess 跳过确认，不构成旧范围扩展的证据。[设置说明与迁移提示](D:/Projects/desktop/AngelBot/src/components/Settings/pages/DesktopControlSettings.tsx)、[现有范围确认事务](D:/Projects/desktop/AngelBot/src-tauri/src/commands/desktop.rs)

最小工具形状可以是现有观察工具增加 `mode = controls | image`，默认 controls；使用同一 CAP_OBSERVE 应用范围和统一执行权限策略。RequestApproval 下展示含“单窗口内容发送给当前模型”的原确认卡；FullAccess 按用户既有选择执行，但仍要求完成范围升级，仍禁止越权与已知敏感界面。这是接口建议，由主任务决定最终契约，不是新增架构层。

保护与锁屏需要保守判定：

- 已知 display affinity 不为 WDA_NONE 时拒绝。`GetWindowDisplayAffinity` 文档说明只有 layered window 且 DWM 工作时查询成功；查询 FALSE 不能当成“无保护”。查询受限与不支持必须明确处理，不能绕过捕获保护提高成功率。微软也不把 affinity 视为完整 DRM 安全保证。[GetWindowDisplayAffinity](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowdisplayaffinity)、[SetWindowDisplayAffinity](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity)
- 仅 `OpenInputDesktop` 成功不足以证明未锁屏：它在断连会话中可能返回重连后才激活的桌面。建议捕获前后核对当前 session 活跃且 `WTSINFOEX_LEVEL1.SessionFlags` 为解锁、输入 desktop 为 Default；状态未知/查询失败拒绝，并释放 WTS 查询 buffer 和 desktop handle。不切换到 Winlogon/UAC 安全桌面、不提升权限。[OpenInputDesktop](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-openinputdesktop)、[会话查询与释放](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsquerysessioninformationw)、[锁屏状态](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/ns-wtsapi32-wtsinfoex_level1_w)、[连接状态](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/ne-wtsapi32-wts_connectstate_class)
- UIA `IsPassword` 表示控件是否为密码字段；它不是任意窗口像素的敏感信息检测器。已知密码控件应拒绝或采用确定的区域屏蔽策略，但 UIA 缺失不能推导图像不含密码、账号或私人信息。不能对用户承诺“截图不会读取密码”；截图范围升级必须明确整个获准窗口可能含私人内容。[IsPasswordProperty](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.automationelement.ispasswordproperty)

这些检查与实际捕获之间仍有竞态；捕获后需要再次核对取消、范围 revision、exe/PID/HWND 和会话状态后才发布图像。系统保护应继续由 WGC 遵从，不使用第二种 API 绕过。工程不能证明所有自绘密码、第三方 DRM 或动态秘密都被识别。

## 最小实施与验收顺序

1. 先定好观察 mode、范围升级、预检/执行/回执契约，保证控制层不调用两套权限代码；MockDesktopAdapter 注入合成 PNG，覆盖拒绝、撤权、取消、无视觉 provider、历史/缓存去图。
2. 一次性 helper 只接收宿主核验的目标标识，禁用自选 monitor、URL、任意路径和模型脚本；用现有最小环境，不继承模型凭据。捕获与 PNG 编码只在内存，输出设严格字节上限并复用 ToolImage 验证。
3. 图像 helper 必须并发、有界排空 stdout/stderr。当前 UIA 8 KiB 观察代码先 wait 再 wait_with_output；直接改成 MiB 图像会产生管道写满→等待超时的死锁风险。正常、拒绝、超时、取消、父进程退出都应回收 helper/捕获资源；不复用已取消帧。
4. 离线 full 只用合成帧/临时 fixture，验证边界，不依赖图形设备、模型账号或用户窗口。另做明确授权、无私密内容的 Win10/Win11 窗口实机验收，覆盖普通 Win32、Chromium/Electron、遮挡/最小化、窗口关闭/替换、大小/DPI变化、锁屏与受保护窗口。只能在实机证据后声明对应兼容性。

本轮不能保证：所有 Windows 程序都可观察；隐藏/最小化/受保护内容可用；HDR 色彩一致；库初始化/GPU Map 永不阻塞；截图能判断业务目标完成；Main Agent 已可按坐标操控任意网页。捕获成功只证明一次有界窗口图像获得，后续网页动作仍应深化现有语义/UIA/专用连接能力。

## 本轮落地

- 只扩展既有 `observe_trusted_app_window`：`mode` 为 `controls | image`，默认仍是 controls。图像由 `DesktopAdapter` 返回非序列化的 `DesktopWindowImage`，再进入现有 `ToolImage` / Main Agent / Responses 瞬时链；不签发控件引用，不产生点击或输入授权，也不引入新 Agent、调度器、权限页面或数据库迁移。
- 新观察范围使用内部 `observeImage` 版本标记。旧记录只加载不扩权；按精确 ID/exe 事务确认，或阅读新说明后明确保存才升级。重复确认不重复推进 revision。非 FullAccess 的图像查看使用现有确认流程；FullAccess 仍必须已经取得新范围，受保护动作的专项确认不变。
- Windows 只走固定、唯一可见无 owner 顶层窗口 → PID/exe/UIA 安全预检 → WGC 单 HWND、首帧 → 停止/回收 → 再次目标和会话核对。捕获专用枚举覆盖同一 PID 多窗口歧义；旧控件观察路径保持不变。会话必须相同、Active、Unlocked、Default desktop，未知拒绝。已知 affinity 受保护和已知密码界面拒绝，affinity 无法查询不被宣称为“无保护”，仅由 WGC 遵从系统保护；没有其他捕获 API 兜底。
- helper 是同一程序的隐藏模式，在 Tauri、数据库、凭据和日志初始化之前分派。复用从 MCP 提取的 `child_process_tree`；删除旧模块而非保留兼容副本。Job 绑定后才写入宿主目标，stdout/stderr 并发有界排空，取消、超时、输出超限或父进程退出会终止进程树。普通释放与 kill/reap 有独立边界。
- 图像不落盘：源帧边长上限 8192、像素上限 16 Mi；只复制有效 RGBA 行，不包含 row padding。必要时用固定 `image 0.25.8` 缩放至边长 2048；最终静态 PNG 上限 2 MiB，超过直接失败。WGC 库可在 resize 回调前重建 GPU 帧池，因此这些是源尺寸预检、CPU/编码预算，不是所有 GPU 分配的硬上限；helper 的期限与进程清理兜底。
- `bounded_png` 是模型桥和本地捕获共享的纯格式校验，完整解码、CRC 和尾部动画拒绝只有一份策略。取消、超时、撤权、配置 revision/exe 改变、无效图像均不得发布载荷；PNG 验证后再核对配置。adapter 错误只回固定分类文案，不显示原始 stderr、控件文本或像素。

实现入口：[捕获宿主](D:/Projects/desktop/AngelBot/src-tauri/src/window_capture.rs)、[WGC 实现](D:/Projects/desktop/AngelBot/src-tauri/src/window_capture/native.rs)、[共享 PNG 校验](D:/Projects/desktop/AngelBot/src-tauri/src/bounded_png.rs)、[共享进程树](D:/Projects/desktop/AngelBot/src-tauri/src/child_process_tree.rs)。

## 验证记录与剩余验收

最终 `python scripts/verify.py quick`（5 个检查）和 `full`（7 个检查）均在清除模型凭据、依赖离线解析的环境通过：前端 63 个文件、517 项测试和生产构建；Rust 类型检查、1100 项 library tests，0 失败；1 项既有原生交互测试按设计忽略，不计入通过数。全量使用模型/传输替身、临时数据库和本地进程夹具，不依赖真实凭据或外网服务。相较上一阶段净增 4 项前端、17 项 Rust 回归，主要用参数化场景覆盖边界，没有新建大套端到端框架。

验证中修正两个实际问题：WinRT 初始化的导入库应是 `RuntimeObject.lib`，而不是 `combase.lib`；当前授权夹具应含新版观察范围。后者定向测试先复现失败（能力集合不相等），再只更新夹具，保留精确相等、revision 增长和禁止重放的安全断言，定向回归通过。全量结果工具曾在 300 秒超时，改用可持续读取的执行会话取得实际终态，没有把超时或部分绿色计为通过。[微软 RoInitialize 的导入库与生命周期要求](https://learn.microsoft.com/en-us/windows/win32/api/roapi/nf-roapi-roinitialize)

普通桌面版 `npx tauri build --no-bundle` 已成功完成（release 编译 2 分 12 秒，退出码 0），生成 `src-tauri/target/release/angelbot.exe`；这证明普通入口与隐藏捕获入口可共同编译，不计作真实窗口捕获、模型图像理解或安装包验收。此次直接依赖及 13 个新增传递依赖的对应许可选项和版权已纳入 [附加第三方声明](D:/Projects/desktop/AngelBot/THIRD_PARTY_NOTICES.txt)，Tauri 资源配置包含该文件；这是本轮新增链的记录，不宣称整个项目已完成发布合规审查。

新增回归使用合成 PNG、虚构窗口候选、纯 WTS 结构、临时数据库及 Node pipe fixture，覆盖旧范围不扩权、确认幂等、严格 mode、权限矩阵、捕获前后撤权/取消/过期、无效 PNG、无引用与无持久化图像、多窗口歧义、尺寸/padding、并发管道上限和取消/超时回收。保留既有 MCP 孙进程 Job 回收测试，没有要求图形设备或用户窗口。

尚无本轮 WGC 实机证据：普通 Win32、Chromium/Electron、遮挡、窗口关闭/替换、大小/DPI 变化、Win10/Win11、锁屏、系统保护与 HDR 均需受控非私人窗口验收；真实 Luna 图像理解也尚未执行。此前 Agent 自动化偶发凭据故障和 UIA Expand 未知结果未由这一轮修复。下一步先做独立原生捕获夹具，再接用户明确授权应用的完整“观察 → 已授权动作 → 回读 → 主对话结果”旅程，不引入坐标静默兜底。
