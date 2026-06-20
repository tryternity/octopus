# 剪贴板恢复竞态修复（desktop 审查一3）

**日期**: 2026-06-21
**状态**: ✅ 已实现（commit `e0f1420`，`PASTE_RESTORE_DELAY = 200ms`，详见 §3）
**来源**: desktop 审查一3（`2026-06-20-desktop-implementation-audit.md` §3.3 + `2026-06-20-desktop-audit-followups.md` §1，原 P2 延后项）
**分支**: `worktree-clipboard-restore-race`（隔离实现，main 让给 e2e 测试）

## 1. 背景

paste 流程（`paste_method = "clipboard"`，默认）经剪贴板粘贴识别文本：写识别文本到剪贴板 → Cmd/Ctrl+V → 恢复用户原剪贴板内容。

「恢复」若发生在系统粘贴动作落地之前，目标应用读到的是已恢复的旧剪贴板内容，而非刚写入的识别文本——用户看到的是自己之前的剪贴板，不是识别结果。

**触发面**：仅 `write_to_clipboard = false`（不保留识别结果）路径触发——此时才需要 save/restore 原剪贴板。`write_to_clipboard = true` 不恢复，无竞态。慢系统 / 高负载 / 慢速目标应用粘贴路径上偶发，低概率低影响。

## 2. 根因

`crates/desktop/src/paste.rs::paste_via_clipboard`（L71-129）时序：

```
read saved（L80）→ write_text(text)（L86）→ sleep 50ms（L89）
→ enigo Cmd/Ctrl+V（L109-117）→ sleep 50ms（L119）→ write_text(saved) 恢复（L125）
```

竞态窗口 = L119 的 50ms。该 sleep 旨在等系统粘贴落地，但 50ms 在慢系统/高负载下不足——Cmd+V 触发的粘贴异步未完成，L125 已把原剪贴板写回，目标应用随后读取时拿到旧内容。

**L89（写剪贴板后 50ms）非竞态点**：`write_text` 同步写入，50ms 足等其稳定供 Cmd+V 读取。

## 3. 方案

**纯延迟，固定 200ms，不可配置。**

- `probe「粘贴已落地」信号`：跨平台无可靠实现（系统剪贴板不暴露「已被目标应用读取」状态），YAGNI。
- `可配置 paste_restore_delay_ms`：当前无按机器调优需求，YAGNI；如未来某平台实测仍竞态再加。

### 3.1 改动

`crates/desktop/src/paste.rs`：

1. 顶部（`use` 之后、`PasteMethod` 之前）新增常量 + 注释：

```rust
/// Cmd+V 后等待系统粘贴落地、再恢复原剪贴板的延迟。
/// 审查一3 竞态修复：原 50ms 在慢系统/高负载下不足——粘贴未落地就恢复，
/// 旧内容被粘进目标应用。200ms 为保守估值；跨平台无可靠「已落地」信号，
/// 故纯延迟、固定值（probe / 可配置均判 YAGNI）。
const PASTE_RESTORE_DELAY: Duration = Duration::from_millis(200);
```

2. L119 替换：

```rust
std::thread::sleep(Duration::from_millis(50));
```
→
```rust
std::thread::sleep(PASTE_RESTORE_DELAY);
```

### 3.2 不改

- **L89 sleep 50ms**：语义为等 `write_text` 落地，同步写入下 50ms 足够，非竞态点。
- **L124-126 恢复守卫**：`!saved.is_empty()` 跳过空 saved（保护非文本剪贴板图片/富文本不被空文本覆盖）——已正确。
- **`write_to_clipboard = true` / `paste_direct` / `paste_method = none`**：不恢复原剪贴板，无竞态。

## 4. 测试

无单元测试（系统剪贴板 + enigo GUI 交互，无法离线测；与 connection-test 同理 YAGNI）。

手动 e2e（补入 `2026-06-20-desktop-audit-followups.md` §2 GUI e2e 清单）：
- `write_to_clipboard = false` + 慢系统/高负载（前台跑重任务）→ 识别粘贴 → 确认目标应用粘进的是识别文本（非之前剪贴板内容）。
- 回归：`write_to_clipboard = true` 路径行为不变（结果留剪贴板）。

## 5. 风险

- **paste 总延迟 +150ms**（50→200ms）。paste 后用户本在等粘贴落地，可接受；固定值无法按机器调优，但 200ms 保守覆盖慢系统。
- 仅缓解非根除：极端慢粘贴路径（>200ms）仍可能竞态。属可接受残余风险（触发概率极低，且无可靠 probe 信号）。

## 6. 涉及文件

| 文件 | 变更 |
|---|---|
| `crates/desktop/src/paste.rs` | 新增 `PASTE_RESTORE_DELAY` 常量 + L119 sleep 改用常量 |
| `docs/superpowers/plans/2026-06-20-desktop-audit-followups.md` | §2 GUI e2e 清单补「剪贴板恢复竞态」验证项；§1 P2 状态改为已实现 |
