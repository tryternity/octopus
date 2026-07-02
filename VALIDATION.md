# Compact Editor 原生化试水 —— 验证记录

> 分支：`compact-editor-native`（worktree，未合 main）。日期：2026-07-03。
> 三条红线 + webview 对比，作为推广到常驻大窗（ASR/剪贴板）的依据。

## 红线 1：中文 IME —— ✅ PASS

切拼音输入、候选词、选词均正常（NSTextView 原生 IME 支持，spike + 联调验证）。

## 红线 2：长文本滚动 —— ✅ PASS

粘几百行滚到底正常（NSScrollView + NSTextView 原生，spike + 联调验证）。

## 红线 3：内存 —— ✅ PASS（per-window 个位数 M）

实测进程 RSS（release 裸二进制，PID 93411，含已预热的 ASR 模型等，基线 ~444 MB）：

| 事件 | RSS | delta |
|---|---|---|
| 编辑窗未开（基线） | 444 MB | — |
| 首次开编辑窗（稳定） | 476 MB | **+32 MB** |
| 关窗后重开（稳定） | 482 MB | **+~2 MB**（相对首开后） |

**解读**：
- 首开 +32 MB 里绝大部分是**一次性 AppKit 预热**：activation policy Accessory→Regular 切换（加载 Dock / 窗口服务器 AppKit 状态）+ 中文 IME 冷启动 + AppKit 资源缓存。关窗**不释放**（macOS 保留 + AppKit 缓存 Regular 态），**重开不重复计**。
- **per-window 原生增量 ≈ 2 MB（个位数 M）** = NSWindow + NSScrollView + NSTextView + 工具栏(8 NSButton + 2 NSTextField) 本身。
- 这是未来常驻窗口（ASR / 剪贴板）真正关心的指标：它们运行在**已 warm 的 Regular app** 里，每个原生窗增量 ≈ 2 MB。

**webview 参考**：迁移动机预估 webview 窗 ~50 MB（本次 macOS 已全程原生，无 webview compact editor 可直接对照测；非 macOS fallback 代码保留但未实测）。

**结论**：原生窗口本身极轻（~2 MB/窗）。首开 32 MB 是 app 级一次性预热，与「原生 vs webview」选择无关（webview 在同一 Accessory→Regular 场景也会付这部分 + 自身 ~50M）。**红线 3 在 per-window 维度通过**，试水达成推广条件。

## 已知限制（非红线）

- **find bar 英文**：系统 NSTextFinder 按钮文字随 app 本地化语言。octopus 未做 zh-Hans 本地化 → 系统组件回退英文。属 app 级本地化（影响所有系统 UI），且仅在打包 .app 生效（裸二进制无 bundle）。归「app 中文本地化」后续任务。
- **非 macOS fallback 初始文本**：webview 回退路径的初始文本随 PENDING 一并移除（macOS 试水阶段，项目以 macOS 为主）。

## 推广条件

三红线全过 + per-window 内存 ~2 MB → **可推广到常驻大窗**（ASR 结果窗 / 剪贴板窗）从 webview 迁原生。注意：那些窗若需保持 Regular app 态常驻，首开一次性预热 ~30 MB 会付一次（非每窗）。
