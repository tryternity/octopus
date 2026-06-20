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
| 一1+ | 启用最终润色 → Esc Cancel → 立刻重开+停止触发润色 → 等旧润色返回 | 新会话粘进**新**润色文本（非旧会话）；日志见 `FinalPolishDone session_id mismatch ... 丢弃` |
| 三1+ | 云端停止(CloudClosing) → Esc/Discard → 立刻重开云端+停止 → 等旧 close 返回 | 新会话粘进**新**云端文本（非旧会话）；日志见 `CloudStreamingDone session_id mismatch ... 丢弃` |

**状态**：待用户本地运行。环境无 GUI，本次未跑。

> 一1+ / 三1+ 两条触发苛刻（需卡在润色/close 窗口内 Cancel+重开+再停），难稳定手动复现；主要靠护栏逻辑正确性 + mismatch 日志验证。

---

## 4. FinalPolishDone / CloudStreamingDone 跨会话护栏（✅ 已实现）

**背景**：审查一1 当时仅给中间润色 `PolishDone` 加了 `session_id` 护栏，认为最终润色 `FinalPolishDone` 已被 stage guard 保护（`handle_toggle` 对 `Stage::Polishing` 忽略 Toggle）。**复核发现该推理有漏洞**——它只覆盖「Cancel 后保持 Idle」，漏了「Cancel（→Idle）+ 立刻重开新录音 + 再次停止触发润色 → 新 `Stage::Polishing`」：旧会话迟到的 `FinalPolishDone` 会匹配新 Polishing，用新 id + 旧润色文本 `do_paste` → 跨会话文本污染。`CloudStreamingDone`（审查三1 引入）同理：CloudClosing 期间 Cancel/Discard 清回 Idle（绕过 Toggle 忙保护），重开云端会话 → 新 CloudClosing，旧 close 结果 `set_full` 覆盖新 transcript。

**触发条件**（窄但真实）：润色 1~3s / close 在飞窗口内 Cancel + 重开 + 再次停止，且旧结果恰好落在新会话的同名 stage 窗口内。命中即静默跨会话污染（粘进/落库错会话文本）。

**修复**（对称于既有 `PolishDone` 护栏，`coordinator.rs` 单文件，机械低风险）：
- `Command::FinalPolishDone` / `CloudStreamingDone` 各加 `session_id: i64`（= 发起时的 transcript.id）。
- spawn 处带 id：最终润色 spawn（`start_final_polish_or_paste`，`id` 已在 L1035 取出）、云端 close spawn（`handle_toggle` CloudStreaming arm，`tr.id`）。
- handler 入口校验当前 stage id == session_id，mismatch 则 warn/debug + return（不动当前 stage）：`handle_final_polish_done`（`Polishing.id`）、`handle_cloud_streaming_done`（`CloudClosing.transcript.id`）。

**验证**：`cargo check --workspace --all-targets` 零 warning；`cargo test -p octopus-desktop` 36 passed / 0 failed。无单测（coordinator 全 Tauri 耦合，与一1 session_id 护栏同理 YAGNI）；行为正确性留 GUI e2e（见 §2 一1+/三1+）。

**状态**：✅ 已实现（本 worktree `clipboard-restore-race`）。audit spec §3.1/§4 已同步修正原「FinalPolishDone 已被保护」结论。

---

## 3. 关联（非本文档范围，仅交叉引用）

- **dashscope ASR 真实 key e2e**：云端 WS 引擎是 `#[ignore]` 测试，从未用真实 DashScope key 跑过端到端。属独立 workstream，见 memory `parallel-workstreams`，不在本审查范围。
