# 推理 unwrap 优雅降级 + osascript Terminal spawn_blocking spec

> **Status: ✅ 已完成**（2026-07-29，分支 `daily_bugfix_0729`）
>
> **背景**：第二梯队健壮性优化。推理路径 4 处 unwrap 改优雅降级；osascript Terminal 启动阻塞改 spawn_blocking。

## 1. 范围

| 项 | 收益 | 风险 |
|---|---|---|
| **A. 推理 unwrap → 优雅降级** | 避免模型推理时 ndarray 非连续/边界条件致 panic；改为返回错误让上层优雅降级 | 零（4 处都有不变量保证，改 ? 是纯防御） |
| **B. osascript Terminal spawn_blocking** | 避免 async fn 内同步 osascript（200ms-2s）阻塞 Tokio worker | 极低（spawn_blocking 标准范式） |

## 2. 设计

### 2.1 项 A：推理 unwrap 优雅降级

4 处非测试 unwrap（全工程推理路径仅此 4 处，已 grep 确认）：

| # | 位置 | 现状 | 改法 | 不变量 |
|---|---|---|---|---|
| 1 | `whisper.rs:345` `mel.as_slice().unwrap()` | ndarray 非连续即 panic | `ok_or_else(\|\| anyhow!(...))?` | compute_mel 返回标准行主序 Array3，当前连续；防御上游重构 |
| 2 | `paraformer.rs:202` `enc_slice.as_slice().unwrap()` | 同上 | 同上 | from_shape_vec 构造 + 标准切片，当前连续 |
| 3 | `whisper.rs:467` `*tokens.last().unwrap()` | Vec 空即 panic | `last().copied().ok_or_else(\|\| anyhow!(...))?` | 循环进入前已 push ≥3 元素；防御性 |
| 4 | `zipformer.rs:1055` `*ans.last().unwrap()` | 同上 | `if let Some(&last_byte) = ans.last() { ... }` | 已有 `!ans.is_empty()` 守卫；decode_byte_bpe 返回 String 非 Result，不改 ? |

**为什么 #4 不改 ?**：`decode_byte_bpe` 返回 `String`（非 Result），改 ? 需改函数签名 → 波及所有调用方（decode_token_ids 等）。且已有 `!ans.is_empty()` 守卫，`if let Some` 更自然。

### 2.2 项 B：osascript Terminal spawn_blocking

**问题**：`action_bar_commands.rs:1896`（`execute_action_bar_inner` async fn）直接调 `launcher.spawn()`，内部 `osascript -e 'tell application "Terminal"...'` + `.output()` wait 到子进程结束（200ms-2s），阻塞 Tokio worker。

**方案**：不改 TerminalLauncher trait（保持同步），调用点包 `tokio::task::spawn_blocking`。与项目既有范式一致（clipboard_commands / action_bar spawn_script 都这么干）。

```rust
let command = command.clone();
let cwd_path = cwd_path.to_path_buf();
tokio::task::spawn_blocking(move || launcher.spawn(&command, &cwd_path))
    .await
    .map_err(|e| format!("Terminal 启动任务异常: {e}"))??;
```

## 3. 调研发现（async Command 大部分不值得改）

经全量调研，**项目已把该改的都改了**：
- 所有 ffmpeg/ffprobe 长任务（数秒级）已全部用 `tokio::process::Command`（record_audio_probe + export_gif + merge_audio_tracks）
- 剩下的 std::Command 都是毫秒级 fork-only（open/open -R/which）或不在 async 直接路径
- sys_open 改 async 会连锁污染 6 个 async 命令 + 4 个同步命令，成本 > 收益

唯一值得处理的是 osascript Terminal（200ms-2s 真阻塞）。

## 4. 不变量
- 推理数值完全不变（golden 测试守护）
- Terminal 启动行为不变（仅把同步阻塞移到 blocking 池）
- 所有函数签名不变（decode_byte_bpe 仍返回 String）

## 5. 风险
- A：零风险（4 处都有不变量保证，改 ? 是纯防御；golden 测试守护数值）
- B：极低（spawn_blocking 标准范式，osascript 行为不变）
