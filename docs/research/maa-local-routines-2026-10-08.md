# MAA 本地日常：按需启动与既有任务队列

日期：2026-10-08（Asia/Shanghai）。本调研以官方文档和源码为依据；未启动 MAA/MuMu，不作为本机业务验收。文档不包含本机安装路径、用户名或真实任务配置。

## 结论

用户追加的约束是 **辅助软件不得为了等待定时而开机启动或空闲常驻**。因此首阶段方向改为 AngelBot 的既有自动化控制面独占触发：到点按需启动 MAA，通过明确的任务接口沿用当前 GUI 队列，完成后只回收本次流程拥有的实例。不每天调用模型，不再建一份 maa-cli 任务配置或游戏执行引擎。这是待实现、待实机验收的方案，不是已启用的每日流程。

MAA 的原生定时要求 MAA 在定时时刻运行，不等于 Windows 任务计划程序，也没有离线补跑保证；它不满足本次“按需启动”的要求，故不再采用自启动＋原生定时的组合。已临时设置的自启动、18:00 定时及连接失败启动选项均已撤销，回读与修改前备份一致。简单设置 `RunDirectly=true` 虽能在启动时开跑，但也改变用户手动打开 MAA 的行为；不默认采用。AngelBot 自身仍须运行，关机、休眠、未登录与助手退出后的调度另行验收。[WPF 定时检查](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/ViewModels/UI/TaskQueueViewModel.cs#L1092)、[启动设置](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/ViewModels/UserControl/Settings/StartSettingsUserControlModel.cs)

调查基线是 MAA **v6.18.0**，而非 `dev-v2`。官方文档站为滚动版本，遇到差异以该版本源码为准。maa-cli 单独检查了提交 `aebb5e9e064a18c81f5241b8e83baa5ed10bc385`，不据此声称本机已安装或验证 CLI。[MAA v6.18.0 发布](https://github.com/MaaAssistantArknights/MaaAssistantArknights/releases/tag/v6.18.0)、[maa-cli 固定提交](https://github.com/MaaAssistantArknights/maa-cli/tree/aebb5e9e064a18c81f5241b8e83baa5ed10bc385)

## 启动 MAA 不是完成日常

| 接口/状态 | 官方行为与边界 |
| --- | --- |
| 启动 `MAA.exe` | 只证明软件启动。是否立即执行 GUI 队列取决于当前配置的 `RunDirectly`，不能直接报“日常完成”。 |
| `MAA.exe --config <配置名>` | 文档声明为切换配置，不是“开始任务”命令。v6.18.0 实现还检查旧 `config/gui.json` 存在；切换后可能重启。不能将参数当稳定的实时控制 IPC。 |
| 同安装目录第二次启动 | 路径派生的 Mutex 拒绝第二实例；通过 `MAA_SHOW_...` 事件仅唤起已有窗口，未转发配置或任务指令。 |
| `RunDirectly=true` | MAA 初始化时经过倒计时调用 `LinkStart`。会影响普通手动打开软件的行为，与每日定时并用还可能产生两条触发路径；按需控制方案默认保持关闭。 |
| `--skip-startup-auto-run` | v6.18.0 支持跳过启动后的自动任务/模拟器启动；它不是停任务参数，也不替代定时设置。 |

依据：[官方启动参数说明](https://docs.maa.plus/zh-cn/manual/introduction/others.html#启动时自动切换配置)、[参数处理与单实例激活](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Main/Bootstrapper.cs#L725)、[配置参数实现](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Main/Bootstrapper.cs#L1432)、[初始化自动运行](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Main/AsstProxy.cs#L725)。未发现可据此承诺的 WPF `--start`/`--stop` 命令行接口，不应自行发明。

`StartSelf` 调用 `AutoStart.CheckStart/SetStart`，不是 `gui.new.json` 内的 `StartSelf` 开关。本文只核实其 UI 属性调用，不承诺自启动的 Windows 注册实现、登录/启动时机或睡眠唤醒能力；这些须在用户授权后实机验收。[StartSelf 源码](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/ViewModels/UserControl/Settings/StartSettingsUserControlModel.cs)

## MuMu 12：两个“启动模拟器”语义不能混淆

MAA 官方支持 MuMu 连接，自动检测适用于正在运行的单个模拟器。常见地址不能替代实际实例端口，也不能根据可执行文件路径推断 ADB 路径、端口或多开编号。保留既有可用连接与 MuMu 增强设置，首阶段不重写它们。[官方连接设置](https://docs.maa.plus/zh-cn/manual/connection.html#自动检测)、[MuMu12 连接类型](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/ViewModels/UserControl/Settings/ConnectSettingsUserControlModel.cs)

- `Gui.StartUpSettings.StartEmulator` 对应 `OpenEmulatorAfterLaunch`：仅控制 **MAA 软件初始化**的 `TryToStartEmulator(true)`。它不是每次定时触发都启动 MuMu 的开关。
- 定时调用现有 `LinkStart`。任务启动先尝试连接；失败且 `RetryOnDisconnected` 开启时才调用无参 `TryToStartEmulator()`，再连接。该分支不受 `StartEmulator=false` 阻止。
- `RetryOnDisconnected` 精确映射 `Gui.StartUpSettings.RestartEmulatorWhenAdbFailed`。若用户希望 MuMu 已关闭时也能执行日常，需授权启用此项，并核对有效 `EmulatorPath`、启动参数、等待时长与目标实例；如果 MuMu 始终已经运行，则不必为正常连接而打开它。它不等于任意错误都重试，更不应自动扩大为杀进程/重启 ADB 权限。

依据：[启动模拟器守卫](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/ViewModels/UserControl/Settings/StartSettingsUserControlModel.cs)、[启动时调用](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Main/AsstProxy.cs)、[断连配置映射](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/ViewModels/UserControl/Settings/ConnectSettingsUserControlModel.cs#L278)、[任务连接失败后的模拟器启动](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/ViewModels/UI/TaskQueueViewModel.cs#L2090)。

## 原生定时事实与未采用的配置候选

v6.18.0 按本地 `DateTime.Now` 检查每日小时/分钟，调度代码读取列表前八槽。`IsEnabled=true` 为持续启用，`null` 为本次触发后置为 `false` 的一次性状态。繁忙时，未启用强制启动会跳过本次处理；强制模式可能停止当前任务并关闭游戏。指定配置模式会在计划时间前两分钟尝试切换配置。因此不能承诺迟启动、睡眠恢复、当时繁忙后自动补跑。[定时器模型](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Configuration/Global/Timer.cs)、[全局定时设置](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Configuration/Global/TimerSettings.cs)、[定时实现](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/ViewModels/UI/TaskQueueViewModel.cs#L914)

持久化入口是 `<MAA安装目录>/config/gui.new.json`。根结构为 `Configurations`、`Timers`、`ConfigVersion`、`Current`、`Update`、`AnnouncementInfo`、`Gui`；大小写必须保留。`CurrentConfig` 是计算属性，标记为不序列化，不能人工增加该字段。全局 `Gui` 与 `Configurations[配置名].Gui` 是不同层级。[Root](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Configuration/Root.cs)、[SpecificConfig](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Configuration/Single/SpecificConfig.cs)、[单配置 Gui](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Configuration/Single/Settings/Gui.cs)

| 最小修改位置 | 安全选择 |
| --- | --- |
| `Current` / `Configurations` | 保留当前名称、全部已有配置与 `TaskQueue` 顺序、任务参数；不创建替代日常引擎。 |
| `Configurations[现有配置].Gui.StartUpSettings.RunDirectly` | `false`；原生定时候选曾由内部定时独占触发，现改为等待显式任务接口。 |
| 同层 `StartEmulator` | 保持原设置；不误当定时启动开关。 |
| 同层 `RestartEmulatorWhenAdbFailed` | 仅用户同意“断连时启动已选模拟器”后启用。保留路径、参数、等待时间。 |
| `Timers.ForceScheduledStart` | `false`；不得强制打断当前任务。 |
| `Timers.ShowWindowBeforeForceScheduledStart` | 保留或关闭；不借它实现执行控制。 |
| `Timers.CustomConfig` | 沿用当前配置的首场景可保持 `false`。若用户要求固定某一配置，则需明确启用并验证两分钟前切换、重启与既有启动设置的关系。 |
| `Timers.List[选定空槽]` | 只改该槽 `IsEnabled`、`Config`、`Hour`、`Minute`。`Config` 必须是已有精确名称；时间由用户确认。保留 `Id`、八槽顺序和其他槽，不清空整个列表。 |

字段依据：[StartUpSettings](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Configuration/Single/Settings/StartUpSettings.cs)、[TimerSettings](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Configuration/Global/TimerSettings.cs)、[Timer](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Configuration/Global/Timer.cs)。这些是补丁位置，不是可以覆盖整份文件的配置模板。

外部修改建议是工程推断：先确认目标安装目录和实际版本，等待 MAA 完全退出，保存独立可恢复备份，解析并验证结构，只改授权字段，检查原文件未并发变化，原子替换后回读核对。禁止覆盖未知版本、损坏文件或运行中的文件。`ConfigFactory` 为 Lazy 内存加载，属性变更约 200ms 后保存并整文件序列化；运行中外改可能被后续保存覆盖。不要借“退出到托盘”假装软件已退出，也不要为了修改配置杀掉正在运行的任务。[加载/保存实现](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Configuration/Factory/ConfigFactory.cs#L70)

保留旧 `gui.json` 及迁移状态，不同时向新旧文件重复写设置。MAA 自带迁移代码会读旧文件并根据条件转换任务与配置；不应另写一套迁移器。没有确认名为 `GuiLoad` 的外部热加载接口或持久化开关，不把它当集成契约。[官方迁移器](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Helper/ConfigConverter.cs)

## 完成、失败与取消的可观察边界

MaaCore 的回调区分 `TaskChainStart`、`TaskChainCompleted`、`TaskChainError`、`TaskChainStopped`、`AllTasksCompleted`；`AsstStop` 与运行状态查询只面向调用方拥有的 Core handle，不是任意附着现有 WPF 进程的 API。直接在 AngelBot 内另建 Core handle 会成为另一执行主体，本阶段不做。[回调协议](https://docs.maa.plus/zh-cn/protocol/callback-schema.html)、[集成协议](https://docs.maa.plus/zh-cn/protocol/integration.html)

WPF 在 `AllTasksCompleted` 收口时还检查失败任务名单，能显示“结束但有错误”；这证明“队列结束”不等于“每个任务成功”。官方开始前/结束后脚本可提供低成本收尾通知，但结束脚本也可在手动停止时按开关运行，且自然完成有错误仍会执行。它没有自动提供完整业务状态和 AngelBot runId，不能仅靠一个结束 marker 判成功。首阶段允许只报告“定时已配置/MAA 已运行”，结果不明时保持未知，不自动重跑。[WPF 完成判定](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Main/AsstProxy.cs#L1509)、[脚本设置模型](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Configuration/Single/Settings/RuntimeSettings.cs)、[脚本手册](https://docs.maa.plus/zh-cn/manual/introduction/others.html#开始前-结束后脚本)

取消须区分“请求取消”“确认停止”和“游戏副作用已撤销”。WPF 停止具有异步回调与超时收口；不能把 GUI 空闲或结束脚本当成 Core 已可靠停止，也不能承诺已完成游戏操作回滚。[WPF 手动停止](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/ViewModels/UI/TaskQueueViewModel.cs#L2522)

## 按需启动候选：仅 loopback 的薄 RemoteControl 桥

MAA 已有官方远控协议，但 **WPF 是 HTTP(S) 轮询客户端，不是监听本地 REST 服务**。用户配置获取任务、汇报状态两个端点，MAA 默认每秒 POST 拉取，身份字段是 `user/device`。协议允许自建服务，理论可接仅绑定 loopback 的 AngelBot 薄桥，无需云服务；实机兼容与 HTTP 警告仍待验收。[远控协议](https://docs.maa.plus/zh-cn/protocol/remote-control-schema.html)、[RemoteControl 配置](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Configuration/Single/Settings/RemoteControl.cs)

`LinkStart` 复用当前 GUI 队列；`StopTask` 异步请求手动停止；`HeartBeat` 仅返回当前远控顺序任务 ID，不是所有 Core/原生定时任务的运行状态。v6.18.0 的 `LinkStart` 分支等待 `UntilIdleAsync` 后汇报，`status` 默认是 `SUCCESS`，没有检查失败任务名单。因此 report `SUCCESS` 不等于游戏任务全部成功；停止请求返回也不证明已停止。[顺序任务与汇报](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Services/RemoteControl/RemoteControlService.cs#L332)、[心跳与停止](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Services/RemoteControl/RemoteControlService.cs#L475)

薄桥的建议边界：仅 loopback、显式授权绑定一个安装实例、最小 `LinkStart/StopTask/HeartBeat` 白名单、唯一请求 ID、去重、单次在途任务、请求/回执与业务成功分离；不暴露抽卡、截图、任意设置或通用命令能力。不建设第二个 GUI 队列或 MAA 引擎，也不与原生定时争夺触发权。身份字符串并非天然安全凭证，本地端点也要考虑其他本机进程；这些是设计建议，不是现成 MAA 安全保证。

还不能仅靠该协议实现完整生命周期：`LinkStart` 可能等待空闲，而 `HeartBeat` 不反映手动任务是否繁忙；领取前的 TTL 不能撤销 MAA 已内部排队的指令。官方接口也未确认提供退出 MAA 的命令。是否忙碌、任务确已结束及正常退出都须另有可验证契约，不能仅凭默认 `SUCCESS` 回报关闭窗口或回收进程树。

## maa-cli：现成替代，但不是现有 GUI 的控制面

官方 CLI 支持 `maa run <task>`，使用自己的 profiles/tasks 配置与 MaaCore。Windows 有官方安装方式，同时仓库 README 明示 Windows 支持维护能力有限。本轮未确认导入 `gui.new.json` 或控制既有 WPF 实例的支持，因此不为当前场景复制一套 CLI 配置。[使用文档](https://docs.maa.plus/zh-cn/manual/cli/usage.html)、[CLI 配置](https://docs.maa.plus/zh-cn/manual/cli/config.html)、[Windows 安装](https://docs.maa.plus/zh-cn/manual/cli/install.html)、[官方 README](https://github.com/MaaAssistantArknights/maa-cli/blob/aebb5e9e064a18c81f5241b8e83baa5ed10bc385/README.md)

已核对的 CLI 源码等待 Core 运行结束、显示 summary，并在回调错误标记存在时返回错误；任务链错误与停止不是同一状态。它注册终止信号，检测用户中断；**Windows 隐藏子进程怎样送出可确认的优雅取消，本轮未实测**，不能把 POSIX SIGTERM 或强制结束进程直接当 Windows 取消契约。CLI 的正常分支有 `asst.stop()`，但中断分支提前返回，清理语义还需核对，不能承诺所有终止均走相同流程。[固定版本执行循环](https://github.com/MaaAssistantArknights/maa-cli/blob/aebb5e9e064a18c81f5241b8e83baa5ed10bc385/crates/maa-cli/src/run/mod.rs)、[回调错误处理](https://github.com/MaaAssistantArknights/maa-cli/blob/aebb5e9e064a18c81f5241b8e83baa5ed10bc385/crates/maa-cli/src/run/callback/mod.rs)

## 最小集成与待验收

建议沿用既有自动化持久化与调度，补一个薄的辅助软件任务执行接口：检查目标实例与授权 → 到点按需启动 → 等待明确的控制面就绪 → 发起当前队列 → 跟踪结果 → 正常关闭本次启动的实例。优先评估上述官方本机 RemoteControl，不把打开窗口当成任务开始；GUI 已开、正在执行或结果未知时不得重新启动或盲目重试。配置连接前仍须备份、最小修改并回读；不改任务选择、消耗设置或手动启动行为。

复用现有受信任应用发现和进程所有权边界，但不能直接把 GUI 路径塞入等待脚本退出的 runner：其父脚本退出后会回收自有进程树，而 MAA 原生任务尚未完成。执行接口应区分启动、任务执行与业务结果，不另建调度器。MAA 的识别、游戏动作、内部重试和任务顺序仍由 MAA 实现；旧有用户实例不由本次流程关闭。

后续实机验收应覆盖：MAA 已开/未开、MuMu 已开/未开、连接失败、计划时刻繁忙、睡眠跨过计划时刻、当前配置被手动切换、手动取消、结束但部分任务失败，以及配置备份恢复。特别核实指定 MuMu 12 实例启动参数、ADB 连接、等待时长。2026-10-08 调研时按需接口尚未实现；2026-10-09 已实现单软件控制并通过不派发任务的本机初始化／连接／退出／恢复预检，最新事实以[统一路线图](../current-roadmap.md)为准。每日北京时间 18:00 与沿用当前任务已确认，但新定时仍未启用，不能把预检称为日常任务成功。

## 收尾与危险动作边界（2026-10-09）

Core 的 `TaskChainStart`、`AllTasksCompleted.finished_tasks` 与 `TaskChainError` 联合核验本轮队列，完成列表也可能包含失败任务。Core 析构可能在完整完成之后追加同任务的 `TaskChainStopped`；这种收尾只在自有进程自然退出 0 时接受，终结前停止、不同 ID／设备及后续新队列仍为未知。[Core 回调与析构](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaCore/Assistant.cpp)

`Gui.PostActions` 的 `ExitEmulator/Hibernate/Shutdown/Sleep` 可能关闭原有模拟器或触发整机电源操作，不能以本次启动和进程清理授权覆盖。首版只读拒绝这些 flags 及未知格式；保留非危险原有动作，`ExitSelf` 须结合完整 Core 证据和进程退出，不能只看退出码。[结束动作执行](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/ViewModels/UI/TaskQueueViewModel.cs#L571)、[flags 定义](https://github.com/MaaAssistantArknights/MaaAssistantArknights/blob/v6.18.0/src/MaaWpfGui/Models/PostActionSetting.cs#L418)
