# AngelBot

AngelBot 是面向单一用户的开源 Windows 个人助手。它以一个持续的主 Agent 连接个人日常和项目工作，在用户可控的权限下完成文件、浏览器、自动化以及轻中量编程任务。

AngelBot 不需要 AngelBot 云账户。会话、记忆、任务、项目状态和权限策略默认保存在用户本机；模型与外部服务由用户自行连接。产品方向和已确认边界见 [Personal Assistant Daily-Use Release](docs/wayfinder/personal-assistant/MAP.md)。

## 当前能力

- 单一主 Agent：用户始终与同一个助手对话，委派工作不会变成第二个用户会话。
- 个人与项目空间：日常事务留在个人空间；选中的文本文件可作为快照交给助手，持续的文件工作进入项目的唯一主对话。
- 受控文件工作：浏览、引用、创建、编辑、复制、移动和重命名项目范围内的文件；不覆盖已有目标，并保留可验证、可反向操作的修改边界。
- Windows 快捷操作：主 Agent 可先读取本机当前可用的受信任应用、系统设置页和项目文件能力，再打开应用或设置、在文件管理器中定位项目内容、填写但不发送消息草稿；任意程序、外部路径和发送动作不在该能力面内。
- 项目隔离：支持 Git 的项目可使用短生命周期 worktree 隔离候选修改；非 Git 项目仍可使用通用工作区。
- 可恢复任务：任务、审校结果和需要用户处理的事项使用结构化状态保存。
- 自动化：支持工作区归属、固定权限和可恢复运行的本地自动化。
- 本地记忆与资料：用户显式设定的人格、资料和经过证据筛选的偏好默认留在本机。

AngelBot 的定位不是邮件助手或编程助手，而是可扩展的通用个人助手。邮件、日历只是首批 Connected Service 示例；后续应用与服务沿用同一套能力发现、授权、执行和恢复机制。尚未完成真实闭环的连接能力不会提前宣称可用。

## 会话、记忆与自适应

AngelBot 的数据默认保存在本地 SQLite。人格由用户显式设定，系统不会通过自适应机制改写人格。

会话上下文采用模型窗口感知的预算：保留完整的近期轮次和工具调用，较早内容压缩为结构化 checkpoint（目标、约束、进度、验证、相关文件与下一步）。上下文指示器优先采用模型返回的输入 token，并仅对最新尚未被模型计量的内容补充本地测量。

自适应仅处理重复出现的交互偏好：它必须通过结构化提取、内容过滤、至少两次证据和置信度阈值后，才会作为低优先级默认项参与提示词。显式用户要求、当前消息和用户设定的人格始终优先；未持续被验证的自适应偏好会自然失效。

## 技术栈

- 桌面端：Tauri v2、Rust、WebView2
- 前端：React 18、TypeScript、Vite、Zustand
- 本地存储：SQLite（rusqlite）
- 富文本：react-markdown + remark-gfm

## 安装与启动

### 前置条件

- Node.js 20 或更高版本
- Rust stable（通过 [rustup](https://rustup.rs/) 安装）
- Windows 上的 Microsoft Edge WebView2 Runtime（通常已随 Windows 11 安装）

### 开发运行

```bash
git clone https://github.com/shuimo233/AngelBot.git
cd AngelBot
npm ci
npm run tauri dev
```

首次启动会直接进入个人空间，不会用配置向导阻断界面。需要模型时，可在设置中选择服务商、模型、连接地址并将 API Key 写入系统凭据库。点击左侧“工作区”旁的 `+` 可通过 Windows 文件夹选择器打开项目。

日常对话的附件按钮支持 UTF-8 文本文件：`txt`、`md`、`csv`、`tsv`、`json`、`log`、`yaml`、`yml`。每次最多 4 个、每个 16 KiB、合计 32 KiB；不静默截断或丢弃不支持的文件。发送的是当时选中文件的内容快照，会随当前会话保存在本地并交给配置的模型，不会授予文件夹访问或修改原文件的权限。不要上传不希望交给该模型的私人内容。图片、PDF 与 Office 附件目前明确不支持读取；需要项目内浏览或修改时再打开对应文件夹项目。

### 前端界面开发

```bash
npm run dev
```

此模式只启动 Vite 前端；文件访问、SQLite、系统集成和完整 Agent 能力需要使用 `npm run tauri dev`。

### 构建桌面应用

```bash
npm run tauri build
```

构建产物位于 `src-tauri/target/release/bundle/`。

## 验证

```bash
python scripts/verify.py quick  # 聚焦修改后
python scripts/verify.py full   # 交付、提交或 CI 准备前
```

前端依赖以 `package-lock.json` 为准，Rust 依赖以 `src-tauri/Cargo.lock` 为准；生成物、运行数据和密钥不提交。分支、提交与发布边界见 [仓库与发布流程](.github/RELEASING.md)。

## 项目结构

```text
src/                 React 前端、状态管理和 Tauri 命令封装
src-tauri/src/       Rust 后端、Agent、数据库迁移和桌面命令
src-tauri/src/agent/ Agent 循环、上下文压缩、工具、提示词和自适应约束
src-tauri/src/migrations/
                     SQLite 的前进式迁移
```

## 数据与权限

会话、设置、记忆和自适应约束默认只保存在本地。连接模型服务时，当前会话所需上下文会按你配置的服务商连接方式发送给相应服务。

文件写入受工作目录约束；当需要写入其他位置时，应先切换会话工作目录。工具的副作用仍由应用的权限与确认流程约束。

## 贡献与许可

改动通过短期分支向 `dev` 提交 PR，完整验证与审查后合并；提交与合并约定见 [仓库流程](.github/RELEASING.md#repository-workflow)。

AngelBot 使用 [MIT 许可证](LICENSE)。第三方组件保留各自许可，新增捕获链的声明见 [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt)；项目许可证不替代第三方声明。
