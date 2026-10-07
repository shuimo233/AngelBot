# AngelBot 前端设计原则调研与优化建议

> 调研日期：2026-09-18
>
> 范围：Windows 桌面个人助手 / 生产力工具；重点覆盖视觉层级、Gestalt 分组与连续性、间距与网格、色彩与对比、动效与减少动态、Windows 桌面交互。
>
> 证据口径：优先采用 W3C/WCAG、Microsoft Windows/Fluent、Apple HIG、Material Design 官方资料；Gestalt 理论引用 Wertheimer 1923 年原始论文的大学档案译本。建议基于当前代码的只读抽查，不等同于完整可访问性审计。

## 结论摘要

AngelBot 当前“低装饰、内容优先”的方向适合生产力工具，不需要改造成高饱和、重卡片或强动效的展示型界面。最值得投入的不是增加装饰，而是把现有三栏界面做得更清楚、更稳、更容易操作：

1. **先修命中区域与字体下限。** 当前常见 `36×36` 图标按钮、`30px` 高操作按钮以及大量 `10–11px` 信息文本，是最明确的可用性风险。Windows 将可触控目标定义为至少 `40×40 epx`，其字体最佳实践给出的下限是 `12px Regular` 或 `14px Semibold`；WCAG 2.2 的网页最低目标虽仅为 `24×24 CSS px`，但这只是合规底线，不是 Windows 桌面产品的推荐体验。[Microsoft：Touch interactions](https://learn.microsoft.com/en-us/windows/apps/develop/input/touch-interactions)；[Microsoft：Typography in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/typography)；[W3C：SC 2.5.8 Target Size (Minimum)](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html)
2. **把“导航—工作内容—辅助工作台”塑造成一条连续任务路径。** 左栏负责定位对象，中栏承担主任务，右栏只呈现当前任务的上下文或工具；三栏的选中态、标题、滚动边界和空状态要互相对应。Material 的 supporting pane 模式同样将主内容与辅助内容区分，并建议主区占多数空间；Microsoft 的 NavigationView 则强调一致导航、适配窗口宽度和在窄窗口收拢侧栏。[Material Design：Canonical layout examples](https://m3.material.io/foundations/layout/canonical-examples/overview)；[Microsoft：NavigationView](https://learn.microsoft.com/en-us/windows/apps/develop/ui/controls/navigationview)
3. **用间距、对齐和字体层级完成分组，少依赖更多边框与卡片。** Wertheimer 的原始实验指出，较小间隔会形成自然分组，方向与“良好连续”也会影响统一知觉；Microsoft 进一步给出 `8 / 12 / 16 epx` 等具体关系间距。对 AngelBot 而言，优先统一对齐线、组内/组间距离和标题层级，比增加阴影、渐变或圆角更有效。[Wertheimer 1923：Laws of Organization in Perceptual Forms](https://www.yorku.ca/pclassic/Wertheimer/Forms/forms.htm)；[Microsoft：Content layout and spacing](https://learn.microsoft.com/en-us/windows/apps/design/basics/content-basics)
4. **保留已有的主题 token、键盘焦点与减少动态支持，但把它们变成可验证的质量门槛。** 文本、图标、边界、选中态和焦点态都应逐色对验证；200% 文本缩放、Windows 对比度主题、纯键盘路径和 `prefers-reduced-motion` 应进入回归测试，而不是只确认 CSS 规则存在。[W3C：SC 1.4.3 Contrast (Minimum)](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html)；[W3C：SC 1.4.11 Non-text Contrast](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html)；[Microsoft：Windows app development best practices](https://learn.microsoft.com/en-us/windows/apps/get-started/best-practices)

## 当前方向：哪些应保持

### 保持 1：低装饰、克制的小圆角

当前大量 `3–5px` 小圆角、轻边界、低装饰表面符合工具型产品的任务导向。不要为了“现代感”把每一组信息都包成浮起卡片，也不必机械照搬 Material 或 Fluent 的视觉皮肤。Apple 的设计原则强调清晰、直接、每个元素都有必要性；这更支持 AngelBot 继续以内容和操作为主，而非增加无功能装饰。[Apple HIG：Design principles](https://developer.apple.com/design/human-interface-guidelines/design-principles)

建议把圆角作为语义 token，而不是继续出现大量近似值：例如小控件、浮层、容器各自一个等级；分区主要通过间距与对齐，边框只在需要明确交互边界或区域边界时使用。

### 保持 2：Light/Dark 语义色 token

保留现有浅色/深色 token 体系，并继续使用“用途命名”而不是“外观命名”，例如 `text-secondary`、`surface-hover`、`border-muted`。Apple 提醒颜色应在浅色、深色和增强对比环境中都能工作，且不应只靠颜色传递状态。[Apple HIG：Color](https://developer.apple.com/design/human-interface-guidelines/color) Windows 也将颜色定位为建立层级和强调重要项的工具，而不是大面积装饰。[Microsoft：Color in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/color)

### 保持 3：`focus-visible` 与 `prefers-reduced-motion`

现有 `:focus-visible` 和 `@media (prefers-reduced-motion: reduce)` 是正确基础。WCAG 要求键盘焦点可见，作者定制的焦点指示还需与邻接颜色达到至少 `3:1` 的非文本对比；作为增强目标，可采用至少 `2px` 实线外轮廓，其面积满足 WCAG 2.2 AAA Focus Appearance 的直接做法（这不是 AA 强制尺寸）。减少动态偏好应让非必要交互动效可被关闭，而不仅仅缩短一点时长。[W3C：SC 2.4.7 Focus Visible](https://www.w3.org/WAI/WCAG22/Understanding/focus-visible)；[W3C：SC 1.4.11 Non-text Contrast](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html)；[W3C：SC 2.4.13 Focus Appearance](https://www.w3.org/WAI/WCAG22/Understanding/focus-appearance)；[W3C：SC 2.3.3 Animation from Interactions](https://www.w3.org/WAI/WCAG22/Understanding/animation-from-interactions.html)

## 优先改进

### P0：命中区域与控件密度

**事实与判断**

- WCAG 2.2 AA 的最低目标为 `24×24 CSS px`，或满足规定的目标间距例外；W3C 同时明确，更大的目标仍会更易用。[W3C：SC 2.5.8](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html)
- Windows 官方把可触控目标定义为至少 `40×40 epx`，触控优化可采用 `44×44 epx` 且目标之间至少有 `4 epx` 可见空隙；视觉图标可以小于命中框。[Microsoft：Touch interactions](https://learn.microsoft.com/en-us/windows/apps/develop/input/touch-interactions)
- 因此，现有 `36×36` 图标按钮虽然通常高于 WCAG 最低线，但没有达到 Windows 的默认触控建议；`30px` 高操作按钮和 `26px` 删除按钮尤其不应作为高频或高风险操作的默认尺寸。

**AngelBot 落地**

- 高频图标按钮、发送/停止、面板开关、侧栏操作：默认命中框改为至少 `40×40`；图标本身可维持 `16–20px`，避免视觉膨胀。
- 紧凑工具条若必须保留 `32–36px` 高度，应确保相邻目标不拥挤、提供清晰 tooltip/accessible name，并提供同功能的正常尺寸入口；不要让“桌面端主要用鼠标”成为所有小目标的豁免理由。
- 删除、停止、授权等高后果操作不应使用最小尺寸；除扩大命中框外，还要用文本或明确图标和状态反馈降低误触。

**边界**：Windows 的 `40×40` 是跨鼠标/触控的产品建议，不是 WCAG 的强制数值；超高密度专家模式可小于 40，但应是显式密度选项，默认界面仍以 40 为目标。

### P0：文字最小值、对比与信息层级

Microsoft 的 Windows 字体最佳实践给出 `12px Regular`、`14px Semibold` 的最小值，并指出更小的尺寸/字重在某些语言中不可读；其 type ramp 使用 `12/16` 作为 caption、`14/20` 作为 body、`14/20 Semibold` 作为 body strong。[Microsoft：Typography in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/typography) 当前代码中广泛存在 `10px` 和 `11px` 的分区标签、元数据、状态、工具说明，不应继续作为承载信息的常规样式。

建议建立精简的桌面字体阶梯：

| 角色 | 建议起点 | 典型用途 |
|---|---:|---|
| Caption / metadata | `12px / 16px`, Regular | 时间、文件大小、辅助说明 |
| Body | `14px / 20px`, Regular | 聊天、设置说明、列表主体 |
| Body strong | `14px / 20px`, Semibold | 列表标题、当前状态、组标题 |
| Section title | `18px / 24px`, Semibold | 页面内主要章节 |
| Page title | `20–28px`, Semibold | 独立页面标题，按空间选择 |

`10–11px` 只可用于真正非必要、丢失也不影响理解的装饰性标记；当前多数 eyebrow、状态、按钮标签和文件信息并不属于这一类，应提升到 12px。不要用颜色变淡和字号变小“双重降级”同一信息；次要信息优先保持可读字号，再通过字重、位置和有限色差降低强调。

WCAG 2.2 AA 要求普通文本至少 `4.5:1`，大文本至少 `3:1`；识别控件、状态和必要图形的视觉信息需与邻接色至少 `3:1`。[W3C：SC 1.4.3](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html)；[W3C：SC 1.4.11](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html) 因而需要逐项测量 `muted` 文本、细边框、图标、进度条、选中态、危险态及 focus ring 在 light/dark 下的实际组合，不能仅凭 token 名称判断。

**边界**：字体阶梯是 Windows 桌面基线，不宜对代码编辑器、日志和稠密数据表做简单全局替换；这些区域可提供独立密度/缩放设置，但仍应通过 200% 文本缩放与不丢功能测试。[W3C：SC 1.4.4 Resize Text](https://www.w3.org/WAI/WCAG22/Understanding/resize-text.html)

### P1：三栏的 Gestalt 分组与连续性

Wertheimer 的原始研究显示，距离更近的元素自然形成组；相似性可以强化或与接近性竞争；连续方向和“良好连续”会使视觉元素被理解为统一结构。[Wertheimer 1923](https://www.yorku.ca/pclassic/Wertheimer/Forms/forms.htm) Apple HIG 也建议通过对齐传达组织和层级，并让控件留有足够空间、按逻辑分组。[Apple HIG：Layout](https://developer.apple.com/design/human-interface-guidelines/layout)

对当前 `248px sidebar + 主区 + 320px workbench` 的建议：

- **左栏 = 导航组。** 顶级入口、会话/任务列表、设置/账户分别形成稳定分区；同组行距紧、组间距明显更大，不要用同一种 8px 间距平铺所有层级。
- **中栏 = 唯一主任务。** 页面标题、当前会话、消息流、composer 沿同一主对齐线组织；composer 与消息流应被知觉为同一工作区，而不是独立卡片。
- **右栏 = 当前任务的 supporting pane。** 顶部标题应明确回答“它辅助的是哪个会话/文件/执行”；切换左栏对象时同步更新或清楚置空，避免右栏残留旧上下文破坏连续性。Material 的 supporting pane 模式把主区作为多数空间、辅助区作为剩余空间，适合此处，但其比例是模式参考而非固定规范。[Material Design：Canonical layout examples](https://m3.material.io/foundations/layout/canonical-examples/overview)
- **建立跨栏连续线。** 左栏当前选中项、中栏标题、右栏上下文标题使用一致命名；顶端基线、分隔线和内容起始线尽量对齐。状态颜色、图标形状和文案在三栏应保持同义，避免同一状态出现三种视觉语言。
- **窄窗口先折叠辅助栏，再压缩导航栏。** Microsoft NavigationView 的自适应模式会随窗口从展开左栏切换到 compact/minimal；AngelBot 不必照抄其固定断点，但应根据中栏最低可用宽度设置内容驱动断点，而非让三栏一直同时缩窄。[Microsoft：NavigationView](https://learn.microsoft.com/en-us/windows/apps/develop/ui/controls/navigationview)

**边界**：Gestalt 原则描述知觉倾向，不是像素规范。不能仅靠“靠近”表达对屏幕阅读器至关重要的关系；DOM/ARIA、标题结构、landmark 和键盘顺序仍要表达相同层级。Microsoft 也要求视觉排列之外提供一致的程序化逻辑层级。[Microsoft：Designing inclusive software for Windows](https://learn.microsoft.com/en-us/windows/apps/design/accessibility/designing-inclusive-software)

### P1：统一间距与网格

Microsoft 指出，一致的间距和 gutter 可在语义上把体验分成组件，并示例：按钮之间 `8 epx`、控件与标题 `8 epx`、控件与标签 `12 epx`、内容区域之间 `12 epx`、表面到内部文本 `16 epx`。[Microsoft：Content layout and spacing](https://learn.microsoft.com/en-us/windows/apps/design/basics/content-basics) Windows 布局文档还建议尺寸、margin、padding 使用 `4 epx` 增量，并在小于 640px 的窗口使用 `12 epx` gutter、较宽窗口使用 `24 epx` gutter。[Microsoft：Alignment, margin, and padding](https://learn.microsoft.com/en-us/windows/apps/develop/ui/alignment-margin-padding)

建议把当前零散值收束为少量 token：

```text
space-1 = 4px   微调（图标内部、紧邻的复合控件）
space-2 = 8px   同组控件、图标与标签
space-3 = 12px  标签与控件、相邻内容块
space-4 = 16px  容器内边距
space-6 = 24px  章节与主要区域
space-8 = 32px  页面级分段
```

其中 `4 epx` 增量以及 `8/12/16/24 epx` 均有 Windows 官方布局实例或建议支撑；`32px` 是 AngelBot 为页面级分段补齐的实现建议。它们是设计基线，不是 WCAG 合规要求。验收重点不是机械套用单一倍数，而是“组内 < 组间 < 区域间”的关系稳定，并让三栏的标题、列表和内容共享对齐基准。

### P1：状态不只靠颜色

WCAG 明确要求颜色不能成为传递信息、动作或状态的唯一视觉手段。[W3C：SC 1.4.1 Use of Color](https://www.w3.org/WAI/WCAG22/Understanding/use-of-color.html) 对 AngelBot 的运行中、成功、失败、需确认、离线等状态，采用“图标/形状 + 简短文本 + 颜色”的组合；危险与成功不要只靠红绿区分。选中项除 accent 色外，还应有背景/边线/字重或选中图标；进度除色条外要有数值或状态文案。

### P2：动效只承担反馈、方向与连续性

Windows 将运动描述为 reactive、direct、context appropriate，用于反馈和强化空间路径；标准时长资源为约 `83 / 167 / 250ms`。[Microsoft：Design principles](https://learn.microsoft.com/en-us/windows/apps/design/design-principles)；[Microsoft：Timing and easing](https://learn.microsoft.com/en-us/windows/apps/design/motion/timing-and-easing) 因此建议：

- hover/press/focus 反馈约 `80–120ms`；小型浮层与面板状态变化约 `160–250ms`。
- 优先 opacity/transform；宽高布局动画只在确实解释空间变化时使用，避免消息流、token 进度或运行状态持续脉动。Microsoft 也建议谨慎使用无限动画，因其持续消耗 CPU。[Microsoft：Optimize animations and media](https://learn.microsoft.com/en-us/windows/apps/develop/performance/optimize-animations-and-media)
- 面板展开、文件预览切换、任务进入/退出可以用方向一致的过渡来维持连续性；不使用视差、大幅缩放或无任务意义的装饰动画。
- 在 `prefers-reduced-motion: reduce` 下移除位移、缩放、视差和自动滚动平滑过渡；保留即时状态变化或极短淡入。自动开始且超过 5 秒的移动/闪烁/滚动内容必须能暂停、停止或隐藏，除非是必要活动。[W3C：SC 2.2.2 Pause, Stop, Hide](https://www.w3.org/WAI/WCAG22/Understanding/pause-stop-hide.html)

## Windows 桌面交互验收清单

- 所有关键路径可仅用键盘完成，焦点顺序与视觉阅读顺序一致；焦点永远可见，不被 sticky 区域或浮层遮住。[Microsoft：Windows app development best practices](https://learn.microsoft.com/en-us/windows/apps/get-started/best-practices)
- light、dark、Windows 对比度主题分别检查文本、图标、边界、focus、selected、error、disabled；不要把 light/dark 当作高对比模式的替代。[Microsoft：Contrast themes](https://learn.microsoft.com/en-us/windows/apps/design/accessibility/high-contrast-themes)
- 在 200% 文本缩放和高 DPI 下检查三栏：内容不重叠、不截断关键操作；优先让右栏覆盖/抽屉化或收起，再压缩主内容。[W3C：SC 1.4.4 Resize Text](https://www.w3.org/WAI/WCAG22/Understanding/resize-text.html)；[W3C：SC 1.4.10 Reflow](https://www.w3.org/WAI/WCAG22/Understanding/reflow.html)
- 图标按钮有可访问名称与 tooltip；危险图标操作有文本确认或可撤销反馈。视觉图标与其命中框分离设计。
- 聊天正文宽度不应由约 `820px` 的容器直接决定。Windows 建议普通拉丁文本每行约 `50–60` 个字符；可给 prose 内层设置约 `60–68ch`，让代码块、表格和图片按需使用更宽区域。中文字符密度与拉丁文字不同，应通过中英文实测调整，不能机械套用 `ch` 数值。[Microsoft：Typography in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/typography)
- 分区之间使用清楚的标题/landmark；视觉三栏结构与可访问性树中的导航、主内容、辅助区域保持一致。

## 建议实施顺序与可验证结果

### 第一批：无争议基础项

1. 将承载信息的 `10–11px` 文本提升到至少 `12px`，正文目标 `14/20`。
2. 将主要图标按钮命中框提升到 `40×40`；30px 文本按钮提升到至少 36px，高频/关键操作目标 40px。
3. 自动扫描并人工复核 light/dark 的文本 `4.5:1`、控件/图标/焦点 `3:1`。
4. 统一焦点样式，并验证键盘完整路径、对比度主题和 200% 文本缩放。

### 第二批：结构与连续性

1. 建立字体、间距、命中框、圆角、运动 token；逐步替换散落常量。
2. 为三栏定义清楚职责和窄窗策略：右栏先抽屉化/收起，左栏再 compact，主区始终保留最低可工作宽度。
3. 对齐左栏选中项、中栏标题、右栏上下文；修复状态与命名不一致。
4. 对设置页、任务页、文件工作台做分组审查：用“组内近、组间远、对齐一致”替代更多卡片。

### 第三批：精修

1. 用 `83/167/250ms` 附近的有限时长收束动效，并完善 reduced-motion 替代状态。
2. 调整聊天 prose 行长，同时保留代码和表格的宽内容能力。
3. 增加视觉回归矩阵：浅/深/对比度主题 × 100%/200% 缩放 × 宽/窄窗口 × 鼠标/键盘。

## 不应做的事

- 不因“艺术化”而增加与任务无关的渐变、玻璃层、阴影、插图和持续动画。
- 不把 Apple 或 Material 的移动端尺寸直接当成 Windows 桌面强制规范；它们适合提供跨平台原则和布局参照，Windows/Fluent 才是默认平台基线。
- 不以全局放大字号、控件或圆角代替信息架构；尺寸修复与层级/分组修复要同时进行。
- 不仅凭肉眼判断对比度、焦点和缩放；这些均应进入自动化或可重复人工验收。
- 不把 WCAG 的最低值理解为最佳值：`24×24` 是目标尺寸的 AA 底线，AngelBot 默认交互仍应优先采用 Windows 的 `40×40` 建议。

## 一手资料索引

- W3C, [WCAG 2.2](https://www.w3.org/TR/WCAG22/)
- W3C, [Understanding SC 1.4.1 Use of Color](https://www.w3.org/WAI/WCAG22/Understanding/use-of-color.html)
- W3C, [Understanding SC 1.4.3 Contrast (Minimum)](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html)
- W3C, [Understanding SC 1.4.11 Non-text Contrast](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html)
- W3C, [Understanding SC 2.4.7 Focus Visible](https://www.w3.org/WAI/WCAG22/Understanding/focus-visible)
- W3C, [Understanding SC 2.5.8 Target Size (Minimum)](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html)
- Microsoft, [Typography in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/typography)
- Microsoft, [Content layout and spacing](https://learn.microsoft.com/en-us/windows/apps/design/basics/content-basics)
- Microsoft, [Alignment, margin, and padding](https://learn.microsoft.com/en-us/windows/apps/develop/ui/alignment-margin-padding)
- Microsoft, [Touch interactions](https://learn.microsoft.com/en-us/windows/apps/develop/input/touch-interactions)
- Microsoft, [NavigationView](https://learn.microsoft.com/en-us/windows/apps/develop/ui/controls/navigationview)
- Microsoft, [Designing inclusive software for Windows](https://learn.microsoft.com/en-us/windows/apps/design/accessibility/designing-inclusive-software)
- Microsoft, [Motion in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/motion)
- Apple, [Human Interface Guidelines — Layout](https://developer.apple.com/design/human-interface-guidelines/layout)
- Apple, [Human Interface Guidelines — Typography](https://developer.apple.com/design/human-interface-guidelines/typography)
- Apple, [Human Interface Guidelines — Color](https://developer.apple.com/design/human-interface-guidelines/color)
- Apple, [Human Interface Guidelines — Motion](https://developer.apple.com/design/human-interface-guidelines/motion)
- Material Design 3, [Canonical layout examples](https://m3.material.io/foundations/layout/canonical-examples/overview)
- Max Wertheimer (1923), [Laws of Organization in Perceptual Forms](https://www.yorku.ca/pclassic/Wertheimer/Forms/forms.htm), York University Classics in the History of Psychology archive
