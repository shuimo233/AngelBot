# 默认吉祥物素材来源

原有参考素材整理日期：2026-10-29，按原始字节归档，没有缩放、裁切、重新编码或重绘。2026-10-31 新增并经用户确认采用应用图标：PNG 保留内置 image_gen 的原始输出，ICO 由项目现有 Tauri CLI 做多尺寸格式转换，例外单独标明。素材来自本对话绘制与已有项目素材，不包含第三方角色图或画风参考截图。

## 图像清单

| 文件 | 尺寸 | 字节数 | 来源 |
| --- | --- | --- | --- |
| [reference/character-sheet.png](reference/character-sheet.png) | 1024 × 1536 | 2336225 | 原生成文件 `exec-e350ece5-71ce-4075-b0e3-bcac4b6f2fd5.png` |
| [sd/default-transparent.png](sd/default-transparent.png) | 1024 × 1536 | 1940605 | 原生成文件 `exec-de400244-7dbe-440c-b00a-4db1d62e0b45.png`；这是 SD，不是正常比例成人立绘 |
| [sd/default-white.png](sd/default-white.png) | 1024 × 1536 | 1594155 | 原生成文件 `exec-e4ededb9-6282-49dd-b945-523fa906142b.png`；比例与画风重绘后的独立 Q 版，不是设定页右下角旧稿 |
| [illustrations/angelbot-moonlit-garden-v1.png](illustrations/angelbot-moonlit-garden-v1.png) | 1536 × 1024 | 3147597 | 原 `artwork/illustrations/angelbot-moonlit-garden-v1.png`，本次迁入 |
| [illustrations/angelbot-seabreeze-daylight-v1.png](illustrations/angelbot-seabreeze-daylight-v1.png) | 1536 × 1024 | 3117118 | 原 `artwork/illustrations/angelbot-seabreeze-daylight-v1.png`，本次迁入 |
| [companion/reading-parts.png](companion/reading-parts.png) | 1536 × 1024 | 2203217 | 此前分层桌宠小样的阅读角色分件 |
| [companion/gamepad-back-hands.png](companion/gamepad-back-hands.png) | 1717 × 916 | 985613 | 原生成文件 `exec-1c47208f-1d49-44e5-a6bc-1871f1795d44.png`；修正后的手柄背面朝外版本 |
| [companion/tea-saucer-hands.png](companion/tea-saucer-hands.png) | 1810 × 869 | 920790 | 原生成文件 `exec-4e6ae74a-2e73-4b89-afb7-8e14c0d8e6fa.png`；一手托碟、一手握柄版本 |
| [icons/angelbot-app-icon-v1.png](icons/angelbot-app-icon-v1.png) | 1254 × 1254，RGBA | 2170146 | 2026-10-31 内置 image_gen；`exec-0d685b73-40bf-48b6-b34f-0496e4f7c3f8.png`，经用户确认的默认应用图标，透明底；构建派生资源在 `src-tauri/icons/` |
| [icons/angelbot-app-icon-v1.ico](icons/angelbot-app-icon-v1.ico) | 16／24／32／48／64／256，多尺寸 ICO | 164003 | 从上述 PNG 使用 `tauri icon` 生成；仅转换规格，未手工修改人物设计 |

## 提示词记录

- [Q 版画风重绘](prompts/sd-style-redesign.prompt.txt)：2026-10-22 在用户指出 Q 版头身比例与画风问题后，最终一轮独立 SD 重绘的原始提示词；对应白底原图。此前 `exec-4b78a0e8-15db-456c-aad2-50f5d95b7c1c.png` 是中间参考稿，不能与最终简化画风混用；透明底图是后续配套素材。
- [月夜庭园](prompts/angelbot-moonlit-garden-v1.prompt.txt)与[海风日光](prompts/angelbot-seabreeze-daylight-v1.prompt.txt)：从原项目插画目录迁入。自然语言生成正文保留，文件引用改成归档后的相对位置；历史画风截图未随包提供。
- [桌宠道具](prompts/companion-props.md)：来自此前多道具小样的素材说明，保留手柄最终生成与杯碟最终编辑提示词；图片链接指向本目录。
- [默认应用图标](prompts/app-icon-v1.prompt.txt)：以内置 image_gen 重绘当前 SD 角色为头像式图标，已获用户确认；用户提供的桌面图标截图只作构图参考，未复制其角色或归档截图。

早期握持错误、按键面朝观众的手柄，以及双手捧杯旧稿均未列入当前素材集。原生成目录和仍被小样使用的文件保留，不改变小样的运行路径。场景插画原来的四个文件已迁入本目录；相关提示词中的本地引用同步更新。

## SHA256 内容校验

```text
343811809f2a16a344cf4c9bfb4fbec669fe613f763804bb4ba43520b27f42cd  reference/character-sheet.png
ebb35a5370dc2bf8cde43373c9064ffb2e74670a2ea2f747a044d0a8d6a654a8  sd/default-transparent.png
9ad807f8ba877600a48368eca3f723427d5211182cbda756d76d0ffcd77ca659  sd/default-white.png
023521ac3f01d66de6622c8a8a5453f3757925cf4f1b8941593479af74462a20  illustrations/angelbot-moonlit-garden-v1.png
69c6449e0cad54722aa9ac95859eb9ffe1268e0f26e8d29acdc676194bce6aea  illustrations/angelbot-seabreeze-daylight-v1.png
b7ddf19080541bd89d8eed840f41dc0f2f155500c398d0f91537e3c49a5fcb03  companion/reading-parts.png
085fe6eb34ea2d3d4cb3e2b33073e2fb80597383863d28d918a65507fa34bbac  companion/gamepad-back-hands.png
279a2e6bc77675068bd22087c4d538a1f0b4240b0bed756578c30321c532838b  companion/tea-saucer-hands.png
383c10337b52e725bd21a37b146ac1a55f0e08707d029b6cee4f04c5d50d8854  icons/angelbot-app-icon-v1.png
217e77e9fc6b73a4ff6a33c78fe38f46127c989c39ed1fd794393ab345ec5018  icons/angelbot-app-icon-v1.ico
```
