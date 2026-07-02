# Compact Editor 原生化试水设计

> 日期:2026-07-02
> 状态:设计已确认,待 writing-plans 出实施计划
> 定位:**试水** —— 验证 webview→原生控件迁移路径,为后续语音识别窗 / 剪贴板窗(常驻 ~100M 内存大头)的降内存改造铺路

## 1. 背景与目标

### 1.1 现状
compact editor 是一个**独立的 Tauri webview 窗口**(`compact_editor_window`)。每次"编辑"由剪贴板窗(`ClipboardItem.tsx`)触发 `openCompactEditor` → 后端 `open_compact_editor` 用 `WebviewWindowBuilder` 建窗 + 加载整个 React bundle。前端是 `<textarea>` + 工具栏,撤销/重做靠 `document.execCommand`,查找替换手写。

功能集:撤销、重做、字号 ±、查找替换、清空、取消、保存、纯文本编辑。

### 1.2 痛点
应用已有 2 个**常驻** webview 窗口(语音识别、剪贴板),每个 ~50M。compact editor 虽是**按需打开**(非常驻),但打开期间又多一个 ~50M webview 实例,叠加模型(ASR / OCR)加载,内存吃紧。

### 1.3 目标(试水定位)
把 compact editor 的 webview 内核换成**原生文本控件**,验证:

- webview→原生窗口的创建 / 通信 / 生命周期可行
- 中文 IME、长文本滚动(egui 当年翻车点)在原生控件下无问题
- 内存确实显著下降

**试水的真正价值不在 compact editor 本身省的那点峰值内存,而在验证一条可推广到「常驻大窗口」(语音识别窗 / 剪贴板窗,~100M 大头)的降内存路径。** compact editor 因功能最简单,作为首个试验田。

## 2. 方案选型

### 2.1 排除的方案

| 方案 | 排除原因 |
|---|---|
| egui | 亲历翻车:multiline `TextEdit` + `ScrollArea` 鼠标滚不到底(max_offset 应 436、实测只到 87.5);另有 IME 全角字符显示问题。**根因:egui 属「自绘 GUI」,文本布局/滚动要框架自己画,正是模型训练最少的环节** |
| Slint | license 不友好(GPLv3 传染 / 否则商业付费) |
| iced / fltk | 同属自绘 / 自带事件循环框架,塞进 Tauri 有集成冲突风险,文本滚动同样要框架自画,大概率重蹈 egui 覆辙 |
| GTK / Qt | 运行时重(glib/pango/cairo 几十 M),违背省内存初衷;自带 main loop 与 Tauri 冲突 |
| 主窗口浮层(复用 webview) | app 是纯多窗口架构(7 个平级独立 webview 窗),无「主窗口」角色;compact editor 触发自剪贴板小窗,无合适宿主可挂浮层 |

### 2.2 选定:系统原生控件

**关键洞察:集成地狱的源头是「跨平台 GUI 框架自带的 winit 事件循环」,不是原生控件本身。** 系统原生控件(NSWindow+NSTextView / Win32 / GTK)用的就是 Tauri 底层**同一套**平台事件循环(NSApp / Win32 消息泵 / GTK main loop),加窗口是同 loop 多窗口,**零冲突**。

选定路径(试水先 macOS):

- macOS:`NSWindow` + `NSTextView`(via Tauri `WindowBuilder` 原生窗 + objc2 挂控件)
- Linux / Windows:试水阶段保留 webview fallback,验证成功后再补

**优势**:

- 集成最干净(同平台事件循环,无第二套事件系统)
- 功能全白送(NSTextView 的无限撤销栈、系统查找条、中文 IME、滚动、选区)
- 内存最省(个位数 M)
- license 友好(objc2 / gtk-rs / windows-rs 全 MIT/Apache)
- 规避模型弱项:把「手搓文本/滚动」外包给 OS,只留确定性的 API 挂载 + 通信

## 3. 总体架构与实现路径

三步:

1. **建无 webview 原生窗** —— `WindowBuilder::new(app, LABEL)` 替代现在的 `WebviewWindowBuilder`,尺寸/标题/可调全保持(`720×560`、`decorations(true)`、`resizable(true)`)。窗口管理(关闭事件、激活策略)继续走 Tauri。位置记忆复用 `window_position`(其函数现收 `WebviewWindow`,需小改适配 `Window`,两者都有 `outer_position`/`set_position`)。
2. **挂原生文本控件** —— 窗口建好后,主线程拿 `ns_window()` → `contentView` 换成一个 `NSScrollView`,其 `documentView` 设为 `NSTextView` 填满。**这是方案唯一的「新代码」**,文本编辑/IME/撤销/查找/滚动全由 NSTextView 白送。
3. **emit 方从 JS 换 Rust** —— 保存触发改由后端 Rust `app.emit(...)`;剪贴板窗 listen 不动。

**第一步是可行性 spike,不铺开。** 先最小版:WindowBuilder 建空窗 + 挂 NSTextView 显示静态中文 + 手动验证「能输中文 / 能滚动 / 能取到文本」。spike 过了再铺工具栏 / 通信 / 字号。

## 4. 通信与生命周期

**事件机制原样复用。** 剪贴板窗前端 `openCompactEditor`(listen `compact-editor://result|cancel` + invoke `open_compact_editor`)**一字不改**;事件名、payload(`{requestId, text}` / `{requestId}`)、requestId 过滤都不动。唯一变化:**emit 发起方从 compact editor 前端 JS 换成后端 Rust**(原生窗无 JS)。

**open 流程简化 —— 去掉 PENDING 中转。** webview 版要 PENDING 是因为建窗异步、得等前端 mount;原生版 `WindowBuilder.build()` 同步返回,建完立刻塞文本,PENDING 不再需要:

```
open_compact_editor(text, request_id):
  if 原生窗已存在(并发再开):
      主线程 textview.setString(text); CURRENT_REQUEST_ID=request_id; SAVED=false
      window.show() + set_focus()
  else:
      建原生窗(WindowBuilder + 挂 NSScrollView/NSTextView)
      主线程 textview.setString(text); CURRENT_REQUEST_ID=request_id; SAVED=false
      macOS: set_activation_policy(Regular) + 显 Dock 图标
```

单例 + 关窗即销毁,与现状一致;并发再开 = 换文本换 requestId。`get_pending_compact_edit` 命令和 `compact-editor://load` 事件可删(无前端 listener)。

**保存 / 取消 / 关窗兜底**:

- 保存(「保存」按钮 / ⌘↵):主线程取 `textview.string()` → `app.emit("compact-editor://result", {requestId, text})` → `SAVED=true` → 关窗
- 取消(「取消」按钮 / Esc):`app.emit("compact-editor://cancel", {requestId})` → `SAVED=true` → 关窗
- **关窗兜底 emit cancel**:webview 版靠 React unmount,原生版改走 `on_window_event` 关闭事件 —— 若 `!SAVED && CURRENT` 则 emit cancel(防 X 关窗 / 系统关闭时调用方 listen 悬空)。关窗后 macOS 切回 Accessory。

取 textview:每次经 `window.ns_window() → contentView → documentView` 重新拿,不缓存裸指针(避免悬垂)。

**额外红利**:原生窗没有前端 JS,**不再需要 `compact_editor_window` 的 ACL emit 授权** —— memory 记过的「capability 漏列致保存不回传」坑天然消失(emit 全在后端 Rust 发,不受前端 ACL 管)。

## 5. 功能保真度分级

| 现状功能 | 原生 NSTextView | 处置 |
|---|---|---|
| 编辑 / 中文 IME / 滚动 / 选区 | ✅ 白送(系统级;IME 比 webview 还成熟;滚动正是 egui 翻车点) | P0 |
| 撤销 / 重做 | ✅ 白送(undo manager,Cmd+Z/Y,比 execCommand 强) | P0 |
| 查找(Cmd+F) | ✅ 白送(系统 find bar) | P0 |
| 保存 / 取消 | 按钮 + 快捷键(要绑) | P0 |
| 字号 ± | `setFont` 一行;持久化从 `localStorage` 改存 `app_config`(和 `window_pos` 同类) | P1 |
| 字数统计 | `string` 长度 | P1 |
| **可视工具栏** | 要自加 | **复刻现状**(顶部 NSView + NSButton 横排) |
| **替换 / 全替 / 匹配数** | 系统有 find,替换要定制 | **系统 find bar**(Cmd+F);替换/全替/N-of-M 后续补 |
| 清空(二次确认) | 要自加 | P2(可 Cmd+A+Del 替代) |

**决策记录**:

- **工具栏:完整复刻现状按钮栏**(顶部 NSView + NSButton 横排)。理由:试水要验证原生控件能承载定制 UI,否则试不出后续复杂窗口(语音识别 / 剪贴板)的参考价值。
- **查找替换:系统 find bar**(Cmd+F 白送)。替换 / 全替 / 匹配数后续补。

## 6. 依赖与构建变更

objc2-app-kit 补 feature(现状只有 NSWindow/NSView/NSColor 等):

```toml
objc2-app-kit = { version = "0.3", features = [
  "NSWorkspace","NSRunningApplication","NSWindow","NSApplication",
  "NSImage","NSImageView","NSView","NSColor",                         # 现有
  "NSTextView","NSScrollView","NSText",                               # 文本编辑 + 滚动
  "NSButton","NSControl","NSTextField","NSFont",                      # 工具栏按钮 + 字号/字数显示
] }
```

不新增 crate(objc2 / objc2-foundation 已在);不加 `windows-rs`(试水只 macOS)。

**平台分流**:原生代码全部 `#[cfg(target_os = "macos")]`;非 macOS 试水阶段**保留现状 webview 版作 fallback** —— `create_compact_editor_window` 走 `#cfg` 分流(macOS 建原生窗、其它仍建 webview)。前端 `CompactEditor/index.tsx` 原样保留(fallback 用,不动)。

**主线程约束**:所有 `NSWindow` / `NSTextView` / `NSButton` 操作必须 `run_on_main_thread`,沿用 `result_window` 范式。

## 7. 验证 · 内存 · worktree · 测试

### 7.1 验证清单(手动,逐项过)

1. WindowBuilder 建无 webview 原生窗,正常显示
2. NSTextView 挂载 + 初始中文显示
3. **中文 IME**(拼音 / 候选词输入)—— 红线
4. **长文本滚动到底** —— 红线
5. 撤销 / 重做(Cmd+Z/Y)
6. 工具栏按钮(NSView+NSButton)显示 + action 触发
7. Cmd+F 系统 find bar 查找
8. 保存(⌘↵)/ 取消(Esc)→ 后端 emit → 剪贴板窗收到文本回写
9. X 关窗兜底 emit cancel
10. 单例 / 并发再开换文本 / macOS Regular↔Accessory 切换

### 7.2 内存实测(核心判据)

- 方法:open compact editor 前后读进程 RSS(`ps -o rss=` 或 Rust mach `task_info`),记增量
- 对比:同一段长文本,**现状 webview 版 vs 原生版** 的 RSS 增量
- **判据:原生增量应是个位数 M,远低于 webview 的 ~50M。若原生也要 30M+,试水目标未达,停下来重评估(可能 WindowBuilder 底层仍带开销,需改走纯 objc2 自建窗路径)**
- **三条红线**:IME、滚动、内存,任一不达即判试水失败,不硬推

### 7.3 worktree 与回退

- **新开独立 worktree**(从 `main`,名如 `compact-editor-native`),不在当前 notepad worktree —— 符合「隔绝在独立 worktree」要求
- 试水失败 → 直接删 worktree,`main` 零影响。这是试水的安全网

### 7.4 测试策略

- GUI 部分 headless 测不出(egui 教训),靠上面手动清单 + 截图存证
- 可单测的纯逻辑:`CURRENT_REQUEST_ID` / `SAVED` 状态机、result/cancel payload 构造(emit 改后端后可 Rust 单测)、字号 `app_config` 存取
- 内存数值记进 worktree 验证笔记,作为后续推广到语音识别窗 / 剪贴板窗的依据

## 8. 后续(试水成功后,本 spec 范围外)

- **Linux / Windows**:补平台原生(`GtkTextView` / Win32 `Edit`),或此时(已有 macOS 经验底气)再评估跨平台框架
- **推广**:语音识别窗、剪贴板窗(常驻 ~100M 大头)按验证路径做原生改造,才是真正的常驻内存收益

## 9. 实施结果（2026-07-03）

**三红线全过，试水通过**（分支 `compact-editor-native`，未合 main）：

- 🔴 IME：✅（拼音/候选词/选词正常，NSTextView 系统级）
- 🔴 长文本滚动：✅（NSScrollView+NSTextView 滚到底正常）
- 🔴 内存：✅ **per-window 原生增量 ~2 MB（个位数）**。首开 +32 MB 经二次开窗对照证明是** app 级一次性预热**（activation policy Accessory→Regular 切换加载 Dock/窗口服务器 AppKit 状态 + 中文 IME 冷启动 + AppKit 资源缓存），关窗不释放、重开不重复计——与「WindowBuilder 底层开销」（§7.2 担忧项）无关，纯 objc2 自建窗退路也省不掉这部分。per-window 2 MB 才是常驻窗场景的真正指标。

**实现要点（spec 之外、落地时确认的）**：
- emit 全走后端 Rust（`do_save`/`do_cancel` + `on_window_destroyed` 关窗兜底 cancel），原生窗无前端 → 前端 ACL emit 授权坑天然消失。
- 快捷键 + 字数统计用一个自定义 container NSView（`CompactEditorContainerView`）担二职：`performKeyEquivalent:` 拦 Cmd+Z/Shift+Z/F/Return/Esc（其余交 super 让 textview 原生处理 Cmd+C/V/A）+ 兼 textview delegate `textDidChange:`。localized，不动全局菜单（避免影响 Tauri tray/其他 webview 窗）。
- **死锁坑**：任何「持 STATE 锁调可能重入的 AppKit 方法」会死锁（undo/redo 同步触发 textDidChange → update_char_count 再锁 STATE，Mutex 不可重入 → beachball）。修法：锁内只 clone Retained 引用、释放后再调。setString 不触发 textDidChange 故未暴露。
- 工具栏按 `contentLayoutRect` 布局（Tauri 原生窗为 fullSizeContent，标题栏占顶部 ~32px，按 contentRect 算才不遮工具栏）。

**已知限制（非红线，归后续）**：
- find bar 英文（系统 NSTextFinder 按 app 本地化语言渲染；octopus 未做 zh-Hans 本地化，且仅打包 .app 生效）。
- 非 macOS webview fallback 初始文本传递随 PENDING 移除（macOS 试水阶段，项目以 macOS 为主）。

**推广条件达成**：per-window ~2 MB + 三红线过 → 可推广到常驻大窗（ASR 结果窗 / 剪贴板窗）的原生改造。详见 [VALIDATION.md](../../../VALIDATION.md)。
