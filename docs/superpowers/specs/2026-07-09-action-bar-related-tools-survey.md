# Action Bar 相关工具综合调研（补充）

> **调研日期**：2026-07-09
> **调研目的**：在 [`2026-07-08-popclip-survey.md`](./2026-07-08-popclip-survey.md)（PopClip/SnipDo/OnText/Click to Do）的基础上，扫描 `~/.tolaria/` 知识库，补充与 octopus action-bar 设计直接相关的工具，提炼**未覆盖的可借鉴点**。
> **范围**：与"选中即操作 / 浮窗 / 截图+OCR fallback / 扩展机制 / 窗口焦点策略"主题相关的 11 份工具笔记。
> **关联文档**：[`2026-07-08-action-bar-design.md`](./2026-07-08-action-bar-design.md) §9 后续演进、[`2026-07-09-action-bar-menu-db-design.md`](./2026-07-09-action-bar-menu-db-design.md)

---

## 0. 调研对象一览

| 类别 | 工具 | 已有调研覆盖？ | 本文档补充价值 |
|------|------|:----:|----------------|
| 语音+OCR+剪贴板编排层 | **VoxFlow** | ❌ | octopus 最像的竞品，定位置换可借鉴 |
| 截图+OCR+翻译全能工具 | **eSearch** | ❌ | 二期"截图+OCR fallback"的成熟参照 |
| 桌面侧边栏+插件生态 | **KoBar** | ❌ | Ghost Window / 插件注册中心范式 |
| 截图+OCR（Tauri 同栈） | **Snow Shot** | 部分 | octopus 已借鉴，仍有未用思路 |
| 屏幕标注（Tauri 同栈） | **MarkerOn** | ❌ | 修饰键绘图 / 点击穿透标注 |
| 剪贴板+OCR+翻译（Qt） | **Paster** | ❌ | 双引擎 OCR / 离线翻译 / 智能卡片 |
| macOS 截图+AX 文本树 | **Moly Appshots** | ❌ | 二期"Accessibility 直读"路径 |
| Tauri NSPanel 插件 | **tauri-nspanel** | ❌ | 浮窗焦点抢夺根治方案 |
| 拖拽中转站 | **DropPoint** | ❌ | 鼠标定位弹窗 / 中转站范式 |
| 启动器调研 | **Wox/Raycast/Alfred** | ❌ | Snippet / Universal Actions / MCP |
| PopClip 补充 | **PopClip** | ✅ | 7 种 Action 类型 / Package 格式 |

---

## 1. VoxFlow（码上写）— octopus 最直接的形态对照

> 仓库：[xingbofeng/VoxFlow](https://github.com/xingbofeng/VoxFlow) ｜ Swift 6 原生 macOS 菜单栏 App ｜ GPL-3.0 ｜ v1.10.0（2026-06-28）

**定位金句**：「**语音键盘**，不是语音助手」——不接管窗口、不抢焦点、不自动发送内容，按住快捷键说话、松开把文字输入到当前光标位置。这与 octopus 的"Accessory 浮窗 + Run And Paste"决策**完全同源**。

### 1.1 与 action-bar 直接对位的「划词动作卡」

VoxFlow 的 `⌘⇧F` 是和 octopus action-bar 完全对位的功能：

| 维度 | VoxFlow 划词动作卡 | octopus action-bar |
|------|---------------------|---------------------|
| 触发 | `⌘⇧F` 全局热键 | `⌘⇧Space`（可配） |
| 直接动作 | 翻译 / 总结 / 任务助手 / 问 AI | 翻译 / 搜索 / 网页 |
| 一键直达 | `⌘⇧J/K/L/P` 直接翻译/总结/发任务助手/问 AI | ❌ 暂无"动作直接绑定快捷键" |

### 1.2 关键可借鉴点

1. **「一键直达」热键**（`⌘⇧J = 翻译`、`⌘⇧K = 总结`）——绕过菜单层级，对最高频动作直接绑快捷键。octopus 二期可在 `action_bar_items` 表加 `shortcut` 字段（如 PopClip/OnText 已有，启动器也有），让用户给自定义动作绑快捷键。

2. **agentCompose 模式（生成可复制 prompt，不注入不发送）**——读窗口上下文（Accessibility 文本 + 窗口标题，**不足时临时截图兜底**）+ 口述意图，用 LLM 生成一段**可复制提示词**。**只复制，不注入、不发送**。这是 octopus "上下文增强"二期的成熟范式：截图是文本兜底而非主入口，安全边界更清晰。

3. **OCR 临时上下文增强（`VoxFlowContextBoostKit`）**——从当前窗口 OCR 文本里抽 Top-K（≤5）术语，作为本次 prompt 的**临时热词**。**标注「仅本次使用，不进入学习」**，不写进热词表。octopus 做 AI 动作时可借鉴：选中文本 + 当前窗口 OCR 的高频术语做临时 context boost，避免污染全局词表。

4. **文本注入切换输入源三段式**——CJK 输入法最容易在"程序粘贴"时出问题。VoxFlow 方案：**粘贴前临时切换输入源 → 模拟 ⌘V → 完成后恢复输入源和剪贴板**（`VoxFlowTextInsertion` 模块：`Clipboard` / `FastPaste` / `SimulatedTyping` / `Coordinator`）。octopus Run And Paste 在某些中文输入法开启的 app 中可能粘贴出乱码——这套三段式是工程答案。

5. **上下文管线安全边界**（写在 `docs/PRIVACY.md`）——**检测并屏蔽密码框**、不自动滚动/修改 UI、截图临时不落盘、**采集超时 500ms**、所有上下文标记来源。octopus 二期"上下文增强"和"截图 OCR fallback"必须把这套安全边界照搬：密码框检测、超时控制、来源标记。

6. **Provider 热词能力矩阵（`ASRHotwordCapability`）**——显式声明每个 Provider 的热词支持模式（`nativeHotword` / `promptContext` / `configuredVocabulary` / `unsupported`）。**只有真有热词 API 的 Provider 才在 UI 显示"热词"标签**。octopus 已在 ASR 层面临类似的"引擎能力不一"问题（不同 ASR 引擎对热词的支持差异），可借鉴这种"诚实标注能力边界"的 UI 设计。

7. **`agentDispatch` 模糊匹配 Agent**（router.rs）——把口述解析成 `ParsedIntent { target_phrase, message }`，在注册表里模糊匹配目标 Agent，返回 `ResolveOutcome`（`Direct` / `Ambiguous` / `NotFound` / `Unavailable`）。octopus 二期 script 动作如果想"模糊匹配目标 app"做"问豆包/问 ChatGPT"类动作，这套 resolver 设计可直接参考。

### 1.3 警示：GPL-3.0

VoxFlow 是 **GPL-3.0-or-later**（同赛道多数为 MIT/Apache）。**只读源码学习架构思想、不抄代码**——避免 copyleft 传染。

---

## 2. eSearch（识屏·搜索）— 截图+OCR fallback 的成熟参照

> 仓库：[xushengfeng/eSearch](https://github.com/xushengfeng/eSearch) ｜ Electron 40 + TypeScript ｜ GPL-3.0 ｜ v15.3.3 ｜ 跨平台

**核心理念**：「截屏是入口，OCR 是桥梁，搜索/翻译/贴图是目标」——一个快捷键（`Alt+C`）触发框选，框选后立即分流到 OCR / 以图搜图 / 二维码扫描 / 贴图 / 录屏 / 屏幕翻译。

### 2.1 关键可借鉴点

1. **「截屏即入口」交互模型**——以"选区"为核心而非以"应用"为核心。octopus 二期截图+OCR fallback 应该把"框选"作为统一入口：框选 → 自动 OCR → 把识别文本喂给 action-bar（不强制走"先有选中文本"路径）。这对禁制复制的页面、PDF、视频字幕是刚需。

2. **离线优先 + 在线增强 OCR 策略**——开箱即用 PaddleOCR（离线），百度/有道在线 OCR 作为补充。octopus 已有 PaddleOCR 本地链路，二期可加"在线 OCR"作为低置信度时的兜底（用户可选启用）。

3. **屏幕翻译（生成贴图窗口，将图片文字替换为翻译文本）**——这是 eSearch 独有的范式：不打开翻译窗口，直接在屏幕贴图上把原文"涂掉"显示译文。octopus pin_window 已有贴图基础，加"OCR→翻译→覆盖原文显示"是高价值差异化（适合视频字幕翻译）。

4. **多引擎并行翻译**（Google 免费 / DeepL / 百度 / ChatGPT / 自定义本地 AI）——octopus 翻译目前固定走 LLM。可借鉴多引擎：默认 LLM，用户可配置 DeepL/Google 作为备选。

5. **段落识别 + 标点分段算法 + 竖排文本识别**——octopus OCR 目前偏纯文本。截图 OCR fallback 时如果识别到表格/段落，保留段落结构能显著提升"截图→AI"动作的输入质量。

6. **异形透明窗口 + 鼠标穿透**（贴图/广截屏/屏幕翻译使用）——CSS 区域接收事件、其他区域仅显示。octopus 浮窗在某些场景（如悬浮提示）可借鉴这种"部分穿透"模式。

7. **WebCodecs 帧级视频编辑**——和 action-bar 关系不大，但 octopus 已有录屏功能，eSearch 这套帧级编辑（自动运镜 / GOP 级优化）是录屏工具进阶的现成思路。

---

## 3. tauri-nspanel — action_bar_window 焦点抢夺的根治方案

> 仓库：[tauri-apps/tauri-nspanel]（macOS NSPanel 插件）｜ Rust + Obj-C FFI

octopus 当前 action_bar_window 用透明悬浮窗 + Accessory 策略 + `before_floating_window_show` 视觉协调方案（详见 architecture.md 第 9 条）。**tauri-nspanel 提供了更原生的根治方案**：

### 3.1 关键技术

1. **isa-swizzling 把 NSWindow 运行时改装为 NSPanel**——`object_setClass` 把窗口类改成 NSPanel 子类，零侵入获得 `nonactivatingPanel` / `hidesOnDeactivate` / `becomesKeyOnlyIfNeeded` 等面板语义。这是 macOS "不抢焦点的浮窗"原生答案，**比当前"激活前隐藏其他 Regular 窗口"的协调方案更彻底**。

2. **`no_activate` 在建窗层堵焦点抢夺**——建窗前临时把 `activationPolicy` 设为 `Prohibited`，建完再复原。action_bar 热键唤出时若有一帧"偷焦点闪烁"，这是根治方案。

3. **`canJoinAllSpaces` + `CollectionBehavior::stationary`**——面板可加入所有虚拟桌面、全屏 app 之上也显示。octopus action_bar 需要全局随时唤出，这是**跨 Space / 全屏应用场景的必备配置**。

4. **`panel_event!` 宏 + 类型安全 FFI**——编译期生成 Obj-C selector 和类型转换，`window_did_become_key` / `window_will_resize` 等回调直接收强类型参数，比手写 `unsafe msg_send` 安全。

5. **`set_released_when_closed(true)` 防 NSPanel 内存泄漏**——NSPanel 默认 close 不释放，是 Obj-C 内存暗坑。octopus 若改用 NSPanel 方案必须注意。

### 3.2 权衡

- **`macos-private-api` feature 影响 MAS 上架**——透明/无装饰窗口需要私有 API，App Store 审核受限。octopus 当前分发渠道（GitHub Releases / DMG）不受影响，但若未来上 MAS 需评估。
- **改造工作量**——当前协调方案已能工作，改 NSPanel 是"锦上添花"而非"刚需"。建议作为 action-bar **三期**或"焦点问题再现时的根治方案"。

---

## 4. Moly Appshots — 二期「Accessibility 直读」路径参考

> 仓库：[moly-appshots]（macOS 截图 + 无障碍文本树捕获，AI Agent 用）

### 4.1 关键可借鉴点

1. **AX 元素树遍历得到结构化文本**——Moly 截图时获取前台窗口 PID，遍历其 Accessibility 元素树得到结构化文本。**这正是 octopus 二期"Accessibility API 直读选中文本"所需的核心技术路径**，可直接参考其 Swift daemon 的 AX 遍历实现（octopus 用 Rust + `objc2` 调 Accessibility API，思路一致）。

2. **双重捕获：截图 + AX 文本树互补**——纯截图让 Agent 视觉分析耗 ~70K tokens，AX 文本树只需 ~2K tokens。octopus 上下文增强时，**选中文本 + 周边 AX 结构比截屏更高效 35 倍**——印证了"直读选中文本"vs"截图 OCR"的 token 经济性差距。这是"Accessibility 直读为优先，截图 OCR 为 fallback"的有力论据。

3. **页面 URL 从地址栏 AX 提取**——浏览器截图时从地址栏 AX 元素自动提取 URL 写入文件。octopus 上下文增强可借鉴：不仅读选中文本，还从 AX 树提取当前 App / 窗口标题 / URL 等上下文元数据。

4. **Electron / Chrome AX 桥接需额外配置**——多进程架构导致 AX 树不默认暴露，需 `--force-renderer-accessibility` 启动参数。octopus 直读选中文本若要支持 Chrome/VS Code/Discord 等，**必须处理同样的 AX 权限和桥接问题**。建议在 README/文档中给出"在 Electron 应用中开启 AX"指引。

5. **`~/.moly/latest.txt` 零 API 文件快路径**——原子写入最新结果路径，Agent 直接 `cat` 本地 JSON，比走 IPC/MCP 快 100 倍。octopus action_bar 与主进程间传选中文本目前用 Tauri command 通道，**高频场景可考虑文件原子写做低延迟通道**（但当前通道已经足够快，未必需要）。

6. **CGEvent Tap 全局热键监听**——daemon 内置 `⌃⌥⌘Space`，不依赖 Shortcuts.app。octopus 热键触发 action_bar 当前用 `tauri-plugin-global-shortcut`，若要做更复杂的热键组合（如双击、长按），可参考原生 CGEvent Tap 实现。

7. **二进制哈希绑定 TCC 机制**——每次重编译需重新 `--setup` 重新授权辅助功能权限。octopus 开发阶段频繁编译会遇到同样问题，需有 TCC 重置流程预案（可在开发文档中加 troubleshooting 章节）。

---

## 5. KoBar（模块化桌面侧边栏）— 浮窗+插件生态范式

> 仓库：[kobar-app]（Electron + React）｜ 模块化桌面侧边栏

### 5.1 关键可借鉴点

1. **Ghost Window 透明覆盖层模式**——超大透明窗口（6000×4000px）+ 鼠标悬停检测动态转发/忽略事件，实现自由浮动 + 始终置顶。**与 octopus action-bar 透明悬浮窗实现方案高度一致**，技术细节可直接参考。

2. **边缘吸附 + 多显示器 + 迷你模式**——拖拽吸附屏幕边缘、收缩为浮动小图标、跨显示器检测。octopus action-bar 当前定位在鼠标上方，可加**位置记忆 + 边缘吸附**体验优化（用户拖过一次就记住位置）。

3. **`kobar.json` 插件清单文件 + `registry.json`**——每个插件仓库根目录放清单文件，GitHub Actions 每日抓取最新版本生成 registry.json。octopus 二期若开放社区自定义动作生态，**清单 + 注册中心机制是低成本方案**：用户在 GitHub 仓库放清单，注册中心聚合为可发现目录。

4. **iframe / 沙箱插件运行时**——插件在独立 iframe 沙箱中运行，动态加载。octopus 二期若做"HTML/JS 自定义动作"，沙箱隔离是安全前提（脚本类已用 `$OCTOPUS_TEXT` 环境变量隔离，但 UI 类动作需要 iframe 沙箱）。

5. **`for-agents/` Vibe Coding 支持**——提供 Agent Skills（SKILL.md）含架构规则、API 约束、UI 指南，可用 AI 工具直接生成插件代码。octopus 若开放社区扩展，可提供类似的自定义动作开发指南，**降低扩展开发门槛**。

6. **Snippet Vault 插件**——独立插件保存文本模板、代码片段、AI 提示词。octopus 二期 Snippet 功能可参考其存储 + 检索 + 快速插入交互。

---

## 6. Snow Shot（截图工具，Tauri 同栈）— octopus 已借鉴，仍有未用思路

> octopus 截图功能已大量借鉴 Snow Shot。这里只列**与 action-bar 二期相关、尚未借鉴**的点。

### 6.1 关键可借鉴点

1. **OCR 表格/数学公式提取（ONNX Runtime 静态编译）**——octopus OCR 仅纯文本。表格结构化输出对截文档场景价值高，可作为 OCR 高级模式（action-bar 二期"截图→AI"时若用户截图是表格，结构化输入质量更高）。

2. **智能窗口识别（区域截图自动吸附窗口/元素边界）**——octopus 截图为纯手动框选。加窗口识别可减少选区操作，尤其 OCR fallback 时精准取窗口内容（鼠标悬停的元素直接 OCR）。

3. **截图→AI 对话（OCR 文本 + 原图一起喂 LLM）**——action-bar 现 AI 动作仅吃选中文本。可扩展"截图→AI"分支，**OCR 文本 + 原图一起喂多模态 LLM**（视觉理解），突破"必须先选中"的限制。

4. **截图→翻译组合动作**——action-bar 可加"截图翻译"组合动作（OCR → 翻译 → CompactEditor），适合图片/PDF/视频字幕场景。

5. **二维码识别 / 颜色拾取 作为独立插件**——action-bar 二期可把"取色""二维码识别"作为低频但高价值的小工具动作（用户截图后直接出结果）。

6. **焦点窗口截图模式**——一键截当前活动窗口，OCR fallback 的常用快捷入口。

7. **插件化架构（核心 13MB + 51MB 插件按需）**——印证"核心 + 可装卸插件"架构是 Tauri 截图工具的成熟模式。action-bar 已是 DB 菜单驱动，可进一步把 OCR/翻译/AI/二维码/取色做成可装卸插件（action_type 扩展点）。

---

## 7. MarkerOn（屏幕标注）— 修饰键绘图 + 点击穿透标注

> 仓库：[markeron]（Rust + Tauri + Vue）｜ 极轻量（~1.5MB）｜ Canvas API

### 7.1 关键可借鉴点

1. **修改键组合绘图**——`Alt=直线 / Ctrl=矩形 / Shift=椭圆 / 组合=正方形圆箭头`。一次拖拽 + 修饰键出 5 种图形，效率倍增。**octopus 截图工具栏目前靠点按钮切换工具**，OCR fallback 场景常需快速标注（遮挡敏感信息后再识别），全快捷键覆盖降低摩擦。

2. **点击穿透模式（标注留存但鼠标事件穿透到底层应用）**——octopus pin_window 是纯图片；可做"标注层 + 穿透"——在任意应用上标注重点后直接对底层内容 OCR，省去截图步骤。这是"屏幕实时标注 + OCR"的差异化方向。

3. **全屏覆盖标注（在任意应用之上绘制，非截图）**——action-bar 二期 OCR fallback 可增加"标注后框选 OCR"路径，而非先截图。比"先框选 → OCR"多一层灵活度（先标注重点 → 再框选 → OCR）。

4. **绘制内容持久化（退出后保留，下次恢复）**——octopus 截图标注一次性；持久化支持跨会话继续编辑/再 OCR。

5. **白板模式（独立白板，复制为图片）**——可作为临时草稿/OCR 结果暂存区，复用 CompactEditor 之外的新场景。

6. **极小体积印证方向正确**——标注层可用纯 Canvas 实现而不必引入 Excalidraw，octopus 现有 SVG/Canvas 方案方向正确。

---

## 8. Paster（剪贴板+OCR+翻译）— 双引擎 OCR + 智能卡片

> Paster（Qt + C++）｜ 国产剪贴板增强工具

### 8.1 关键可借鉴点

1. **双引擎 OCR（Tesseract 多语言 + PaddleOCR 中文）**——octopus 仅 PaddleOCR 单引擎，对英文/多语种弱。**Tesseract 可作轻量多语种 fallback 或"快速 OCR"模式**（区别于高精度中文 OCR）。

2. **OCR 三模式**（截图 OCR / 图片 OCR / 快速 OCR）——分层"快速识别"与"高精度识别"两档，对应 action-bar 二期的 OCR fallback——**无选区时快速 OCR 整屏 vs 选区后高精度 OCR**。

3. **离线翻译（Argos Translate，中⇄英本地）**——action-bar 现有 AI 动作走在线 LLM，离线翻译可作为**无网 fallback 或新增 action_type**（翻译结果直接入剪贴板）。

4. **截图标注补全**（马赛克 / 高亮笔 / 橡皮擦 / 气泡 / 图层管理 / 裁剪）——octopus 截图工具栏仅矩形/箭头/文字/序号/撤销；**马赛克与高亮笔**对截图 OCR 场景尤其实用（遮挡敏感信息后再识别/分享）。

5. **图像调整**（亮度/对比度/灰度/反色/透明度）——截图 OCR fallback 前可对低对比度区域预处理，提升识别率。

6. **桌面贴图可旋转/翻转/继续编辑标注**——octopus pin_window 只能拖拽缩放，不能二次标注/变换；截图→贴图→OCR 可串成闭环。

7. **智能卡片（自动识别表达式/颜色码/时间戳/单位换算）**——OCR 出文本后智能结构化，action-bar 可在 AI 之外加**轻量本地"智能解析"动作类型**（不需要 LLM 调用，本地正则即可）：选中 `#ff5500` → 显示颜色预览；选中 `2+2*3` → 显示 8；选中 `2026-07-09` → 显示距今天数。

---

## 9. DropPoint（拖拽中转站）— 鼠标定位 + 中转站范式

> 仓库：[DropPoint]（Electron）｜ 拖拽中转辅助

### 9.1 关键可借鉴点

1. **拖拽中转站模式：拖入 → 切换位置 → 拖出**——action-bar 若做文件/内容拖拽，可借鉴"悬浮窗作为临时中转站"——用户先拖入 action-bar 缓存，再拖到目标位置，**解决跨窗口/跨桌面拖放痛点**。

2. **全局快捷键在鼠标位置创建实例**——`Shift+Tab` 在当前鼠标位置弹出悬浮窗。octopus action-bar 热键唤出时**跟随鼠标定位是更自然的交互**（已实现）——比固定位置弹出更符合"上下文就近"原则。DropPoint 是同一思路的验证。

3. **关闭窗口最小化到托盘**——action-bar 关闭时不应销毁窗口、而是隐藏到托盘/菜单栏待唤出，降低重新创建开销（octopus 已实现单例 show/hide）。

4. **跨虚拟桌面置顶**——macOS 默认支持跨 Space 显示。对应 NSPanel 的 `canJoinAllSpaces`——DropPoint 用普通窗口 + 置顶实现，效果不如 NSPanel 原生面板语义完整（呼应 §3 tauri-nspanel）。

---

## 10. 启动器调研（Wox / Raycast / Alfred）— Snippet / Universal Actions / MCP

### 10.1 关键可借鉴点

1. **Raycast Universal Actions：选中即操作**——对选中内容（文件/文本/URL）展示可用操作列表，是**上下文感知的成熟实现**。octopus action-bar 两级菜单可借鉴：第一级展示内容类型，第二级展示该类型可用动作（与 PopClip 的"按文本类型自动决定显示动作"一致）。

2. **Raycast Snippets：关键词展开 + 动态变量**——关键词触发片段展开，支持动态变量占位符。octopus Snippet 功能可直接参考此交互模式：输入关键词 → 自动展开带变量的模板。

3. **Alfred Snippets：集合 + 富文本 + 自动展开 + 占位符 + 安全字段**——片段支持集合分组、**安全字段（密码等不回显）**，自动展开机制。octopus 可参考：Snippet 分组管理 + 敏感字段处理。

4. **Alfred Workflow 画布编辑器（零代码编排）**——拖拽式可视化编排、Prefabs 预制组件、条件分支、变量系统。octopus 二期若做**多步骤自定义动作**，可视化编排比纯代码配置更友好（但工作量大，建议作为远期方向）。

5. **Wox 插件进程隔离 + JSON-RPC over WebSocket**——每个插件独立宿主进程隔离。octopus 扩展机制的安全隔离参考：**脚本类自定义动作可独立进程运行**（当前 script 动作直接 `sh -c` / `osascript -e`，安全沙箱是后续考虑）。

6. **Wox AI Command 模板 + silent query hotkey + Run And Paste**——AI 命令模板（翻译/摘要）、静默模式热键、执行后自动粘贴结果。octopus Snippet + AI 动作可参考：**动作执行结果直接回写/粘贴**（已实现 CompactEditor 展示 + 剪贴板，可加"直接粘贴"快捷模式）。

7. **Wox MCP 集成（STDIO + StreamableHTTP）**——启动器既是 MCP client 也是 server。**octopus 二期扩展机制若对接 MCP 协议**，可直接复用 Wox 的双端实现方案——action-bar 的 script 动作可演化为"MCP tool 调用"，对接任意 MCP server。

8. **缩写匹配（Alfred `gc`→Chrome）**——输入缩写快速匹配目标。octopus 动作搜索/菜单导航可加**模糊匹配 + 缩写快捷选择**（菜单项目数多时尤其有用）。

9. **Wox Plugin / Theme / AI Command 三类商店**——分离式商店管理。octopus 可将内置动作、自定义动作、主题分开管理（当前主题已独立，动作可加"动作包"概念）。

---

## 11. PopClip 补充（与原调研对照）

原 [`2026-07-08-popclip-survey.md`](./2026-07-08-popclip-survey.md) 已覆盖 PopClip 主体。`~/.tolaria/桌面工具/popclip-*.md` 笔记补充以下未覆盖点：

1. **7 种 Action 类型抽象**（在原调研"Snippet YAML"基础上更完整）：URL / Key Press / Service / Shortcut / Shell Script / AppleScript / **JS-TS**（！）。octopus script 动作目前覆盖 shell/osascript/powershell/python，**JS-TS 是缺失项**——可考虑加 `#node` / `#deno` magic comment（用户量大，运行时易得）。

2. **`.popclipext` Package 格式**——文件夹含 Config、图标、脚本、Readme，适合**多步骤复合功能**。octopus 复杂自定义动作可用文件夹/多文件结构（当前 action_data 是单字段，复杂脚本只能内联）。

3. **多维度过滤规则**（上下文感知的核心）——按当前应用 include/exclude、按选中文本正则匹配、按剪贴板操作可用性（cut/paste/format）、按文本内容类型（URL/Email/文件路径）、按扩展自身配置选项。**这正是 octopus 二期「上下文感知 + 正则匹配」的最佳参考模型**——单个动作可配多条规则。

4. **数字签名 + 未签名警告的安全机制**——扩展生态安全策略。octopus 若开放社区扩展生态可参考（脚本类动作尤其需要）。

5. **App/域名排除 + 默认排除模式**——可排除特定 App 或反向配置「仅启用选择的 App」。octopus action-bar 可加 **per-app 启用/禁用配置**（用户在密码管理器/终端中禁用 action-bar）。

6. **外观自定义（vibrancy 毛玻璃）**——菜单样式 + 透明度 + 背景效果可调。octopus 透明悬浮窗可参考其 vibrancy 效果（NSVisualEffectView）。

---

## 12. 综合结论：octopus action-bar 二期可借鉴点汇总

按优先级 / 实现成本排序：

### P0 — 与现有二期计划直接对位、价值最高

| 借鉴点 | 来源 | 对应 octopus 二期项 |
|--------|------|---------------------|
| **Accessibility API 直读选中文本**（AXSelectedText） | Moly / VoxFlow / OnText | action-bar-design §9 二期项 |
| **Electron/Chrome AX 桥接（`--force-renderer-accessibility`）** | Moly | Accessibility 直读的兼容性补丁 |
| **上下文增强（窗口标题 + URL + App 名）** | Moly / VoxFlow | action-bar-design §9 二期项 |
| **截图+OCR fallback 串联（已有截图+OCR 链路）** | eSearch / Snow Shot | action-bar-design §9 二期项 |
| **Snippet 自定义动作格式**（`#octopus` YAML） | PopClip / OnText / Raycast | action-bar-design §9 二期项 |
| **正则上下文规则**（动作可配正则仅匹配时显示） | PopClip / OnText | action-bar-design §9 二期项 |

### P1 — 新发现的可借鉴点（建议纳入二期）

| 借鉴点 | 来源 | 价值 |
|--------|------|------|
| **一键直达热键**（动作直接绑快捷键） | VoxFlow（⌘⇧J/K/L）/ OnText / 启动器 | 高频动作绕过菜单，DB 加 `shortcut` 字段 |
| **密码框检测 + 采集超时（500ms）+ 不落盘** | VoxFlow `PRIVACY.md` | 上下文增强的安全边界必备 |
| **OCR 临时上下文 boost**（Top-K 术语作临时热词，不进词表） | VoxFlow `VoxFlowContextBoostKit` | 提升 AI 动作质量而不污染词表 |
| **文本注入切换输入源三段式**（切输入源→⌘V→恢复）✅ 已实现 | VoxFlow `VoxFlowTextInsertion` | 解决中文输入法下 Run And Paste 乱码（`crates/desktop/src/input_source.rs`，config `switch_input_source_on_paste`） |
| **Per-app 启用/禁用 action-bar** | PopClip / SnipDo | 密码管理器/终端场景刚需 |
| **多引擎翻译**（默认 LLM，可配 DeepL/Google） | eSearch / Paster | 用户偏好选择 |
| **智能解析动作类型**（正则识别颜色/表达式/时间戳） | Paster | 不需 LLM 调用的本地轻量动作 |

### P2 — 工程优化与扩展生态

| 借鉴点 | 来源 | 价值 |
|--------|------|------|
| **tauri-nspanel NSPanel 方案**（根治焦点抢夺） | tauri-nspanel | 三期或"焦点问题再现时的根治方案" |
| **`canJoinAllSpaces` + `stationary`**（跨 Space/全屏） | tauri-nspanel / DropPoint | 全局唤出必备配置 |
| **`kobar.json` 插件清单 + `registry.json`** | KoBar | 社区扩展生态的低成本注册中心 |
| **iframe / 沙箱插件运行时** | KoBar / Wox | HTML/JS 类自定义动作的安全前提 |
| **Wox MCP 集成（STDIO + StreamableHTTP）** | Wox | action-bar script 动作演化为 MCP tool 调用 |
| **修改键组合绘图 + 马赛克/高亮笔** | MarkerOn / Paster | 截图 OCR fallback 前的快速标注 |
| **点击穿透标注 + 实时标注后 OCR** | MarkerOn | 差异化方向：屏幕实时标注 |
| **屏幕翻译（贴图覆盖原文显示译文）** | eSearch | 视频字幕翻译的差异化方向 |

### P3 — 警示与边界

| 警示 | 来源 | 含义 |
|------|------|------|
| **GPL-3.0 传染风险** | VoxFlow / eSearch | 只学架构思想、不抄代码 |
| **TCC 重编译重授权** | Moly | 开发阶段频繁编译需有 TCC 重置流程预案 |
| **NSPanel 内存暗坑**（`released_when_closed`） | tauri-nspanel | 若改 NSPanel 方案必须注意 |
| **`macos-private-api` 影响 MAS 上架** | tauri-nspanel | 未来分发渠道决策的考虑项 |

---

## 13. 信息来源

- `~/.tolaria/ai-agent/voxflow-码上写-macos-语音截图剪切板-agent-工作台.md`
- `~/.tolaria/esearch-识屏-搜索-截屏-OCR-翻译-贴图-录屏.md`
- `~/.tolaria/tauri-桌面应用/tauri-nspanel-把-tauri-窗口动态子类化为-macos-nspanel-面板的插件.md`
- `~/.tolaria/ai-agent/computer-use/moly-open-appshots-面向-ai-agent-的-macos-屏幕截图与无障碍文本树捕获工具.md`
- `~/.tolaria/桌面工具/kobar-模块化桌面工具侧边栏-多插件生态-electron-react.md`
- `~/.tolaria/tauri-桌面应用/snow-shot-超好用的截图工具-tauri-rust-nextjs.md`
- `~/.tolaria/tauri-桌面应用/markeron-轻量开源屏幕标注工具-rust-tauri-vue.md`
- `~/.tolaria/剪贴板工具/paster-剪贴板增强工具-截图标注-离线ocr翻译-录屏-桌面便签-qt-cpp.md`
- `~/.tolaria/droppoint-拖拽中转站-跨窗口-跨虚拟桌面-文件拖放辅助工具-electron.md`
- `~/.tolaria/桌面工具/启动器功能特性调研-wox-raycast-alfred.md`
- `~/.tolaria/桌面工具/popclip-选中复制后的无限可能-macos文本操作扩展平台.md`
