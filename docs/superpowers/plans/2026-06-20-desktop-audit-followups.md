# desktop 实现审查 · 后续待办

> 来源：`docs/superpowers/specs/2026-06-20-desktop-implementation-audit.md`（7 条审查的复核 + P0/P1 实施）。
> 状态（2026-06-20）：P0（一1/一2/二1）+ P1（二2/三2/三1）**均已合并 main**（P0 `44b8ab8`、P1 `9a19b6b`）。本文档记录尚未完成、需后续处理的事项。

---

## 1. 一3 剪贴板恢复竞态（✅ 已实现）

**背景**：paste 流程会先备份用户当前剪贴板 → 写入识别文本 → 触发系统粘贴（Cmd+V）→ 恢复原剪贴板。若「恢复」发生在系统粘贴动作完成之前，恢复的旧内容会被粘进目标应用（用户看到的是自己之前的剪贴板，而非识别文本）。

**审查结论**：真实但低危——仅在慢速系统 / 高延迟粘贴路径上偶发，绝大多数场景粘贴是同步完成。详见 audit spec §3.3。

**修法草图**（实施时再细化）：
- `paste::paste` 成功返回后，延迟一个保守时长（~150–300ms，需按平台实测）再 restore 剪贴板；
- 或改为「restore 前 probe 粘贴是否落地」的信号（更复杂，YAGNI，优先纯延迟）。
- 注意 macOS / Windows / Linux 粘贴异步性不同，延迟可能需按平台分档。

**状态**：✅ 已实现（worktree `clipboard-restore-race`，`PASTE_RESTORE_DELAY = 200ms`；spec `2026-06-21-clipboard-restore-race-design.md`）。行为正确性待 GUI e2e（见 §2）。

---

## 2. GUI e2e 验证（P0 + P1 行为正确性）

P0/P1 的修复逻辑均由 `cargo check` + 逻辑审查 + 既有单测保证，但**行为正确性留 GUI e2e**——CI 环境无 GUI / 无真实音频设备 / 无真实 DashScope key，以下项需在本地桌面环境手动验证：

| 来源 | 验证项 | 预期 |
|---|---|---|
| 一1 | 录音中 Esc（Cancel）→ 立即重开新录音 → 旧中间润色结果 | 不污染新会话 transcript / 不写错 DB 行 |
| 一2 | 云端引擎 + 无效 API Key → 触发语音 onset | 结果窗报「⚠️ 云端识别失败」，状态复位，下次 onset 重试 |
| 二1 | 设置窗改 denoise_mode / 硬件加速 → 保存 | **本次生效**（无需重启），asr 缓存已 reload |
| 二2 | 设置窗改麦克风设备 → 保存 → Toggle 新录音 | 用新设备采集（非旧设备） |
| 三2 | 设置窗切 ASR 引擎 → 立即 Toggle 录音 | 首次识别无明显懒加载卡顿（已后台预热） |
| 三1 | 云端录音中连按 Toggle 停止 → 网络模拟慢 close | 主线程不卡（快捷键不堆积），close 完成后自动粘贴 |
| 三1 | 云端 CloudClosing 期间点 Cancel / Discard | Cancel：不粘贴不写库；Discard：写库保历史不粘贴 |
| 一3 | `write_to_clipboard=false` + 慢系统/高负载 → 识别粘贴 | 目标应用粘进识别文本（非之前剪贴板内容） |

**状态**：待用户本地运行。环境无 GUI，本次未跑。

---

## 3. 关联（非本文档范围，仅交叉引用）

- **dashscope ASR 真实 key e2e**：云端 WS 引擎是 `#[ignore]` 测试，从未用真实 DashScope key 跑过端到端。属独立 workstream，见 memory `parallel-workstreams`，不在本审查范围。
