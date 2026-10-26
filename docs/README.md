# AngelBot 工程文档入口

先读 [当前路线图](current-roadmap.md)：能力矩阵、有效证据和下一步只维护在那里。日常修复不再追加开发日记或另建计划。

## 哪些文件需要维护

| 入口 | 何时更新 |
| --- | --- |
| [当前路线图](current-roadmap.md) | 能力、缺口或有效验收有变化；替换当前事实，不累计逐轮汇报 |
| [设计原则](design-philosophy.md) | 产品定位、Main 体验或授权原则真的改变；已合并旧日常助手决策票据 |
| [架构决策](architecture-decisions.md) | 稳定架构选择改变；ADR-0001／0002／0003 保留编号，不再各占一份文件 |
| [委派规范](delegated-agent-framework-spec.md)／[网络规范](delegated-network-capability-spec.md) | 对应生产合同改变；独立权限层级仍分开，不为少文件硬合并 |
| [术语表](../CONTEXT.md) | 领域术语或命名改变，不复制模块进度 |
| [桌面验收清单](desktop-beta-checklist.md) | 验收要求改变或有真实证据签收，不用测试存在代替通过 |
| [用户说明](../README.md) | 安装、操作或用户可见限制改变，不重复内部计划 |
| [验证技能](../.codex/skills/angelbot-verify/SKILL.md)／[E2E](../e2e/README.md)／[发布规范](../.github/RELEASING.md) | 相应命令或流程改变，不在普通功能修改时同步重写 |

一次修改通常只涉及路线图中受影响的状态和必要用户说明；不要求把所有文档改一遍。旧 Main 体验、模型合同和发布质量 open 票据已由现有生产入口／合同及验收清单承接，不再维护平行 backlog。

## 参考资料不跟随每轮开发更新

- [固定验收归档](archive/implementation-evidence-2026-10-17.md)：保留原路线图 3.6 起的失败、修复及验收记录，原编号不变；只读、不追加，不作为当前规则或下一步。
- `research/`：按原日期保留一手资料与评估，不覆盖路线图。Windows 平台与连接服务决策分别并入既有 [平台依据](research/windows-daily-use-capabilities.md)、[连接依据](research/connected-services-windows.md)，不另开决策票据。
- 其他被忽略的本地历史设计、`issues/` 和 `archive/historical/` 不作为当前规范，也不应为了整理文档强制加入 Git。
- 本机原有 50 份散落设计／审计／研究和旧工程规则已收敛到 `archive/local-design-notes-2026-10-17.zip`；它仅是本机备份，不随仓库发布、不参与日常文档读取。原文相对路径与 SHA256 清单随包保存，可按需恢复。仍适用的通用 IPC／迁移／工具／回归准则已并入 [AGENTS.md](../AGENTS.md)，过时的强制测试配额和旧架构状态不再维护。

## 代码与验证入口

```text
src/                         Main 工作面、工作台、设置与 typed IPC
src-tauri/src/commands/       前台准入、上下文、持久运行与能力装配
src-tauri/src/agent/          会话循环、Supervisor、委派、审校、候选与隔离
src-tauri/src/llm/            模型协议／授权；Windows、MCP 等沿既有平台模块
src-tauri/src/migrations/     前进式本地状态迁移
scripts/verify.py、e2e/       canonical 离线验证与显式隔离旅程
```

源码按职责组织，不单纯为减少文件数合并不同层级的模块。公开文件以 Git 跟踪状态及 `.gitignore` 明确允许项为准；运行数据、凭据、截图、日志和构建产物不进入提交。
