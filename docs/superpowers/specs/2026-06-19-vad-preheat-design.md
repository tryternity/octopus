# 启动/录音性能优化设计：VAD 预热 + 单例（①），lock-free 音频（③可选）

> Date: 2026-06-19
> 状态：设计中

## 1. 背景

### 1.1 VAD 重复加载（① 主项）

coordinator 每次 Toggle（开始录音）实时构造 VAD：

- `coordinator.rs:579/629/668`（detection vad，三个 Toggle 分支）+ `:675`（filter_vad，VadSegmented 场景）调 `octopus_asr::vad::SileroVad::new(&path)`——同步 ONNX 加载，百 ms 级。**首次按快捷键 → 录音启动有明显延迟**。
- filter_vad（675）每个语音段都重新加载一次（VadSegmented 按段处理）。
- `main.rs:210-222` 的 preheat 只 preheat ASR model（`active_model`），**不碰 VAD**。

### 1.2 音频回调锁（③，可选）

`audio.rs` 回调里 `mono.collect()` + 写 `Arc<Mutex<Vec<f32>>>`，coordinator `drain_samples` 持锁读。16kHz 语音数据，写频率远高于读，实际锁竞争低——项目长期稳定运行，无证据锁是瓶颈。

## 2. 目标

- **①**：VAD 实例全局缓存（`OnceLock`），Toggle 复用而非重加载；`main.rs` preheat 预加载 detection VAD，首次按下不再卡。
- **③**：方案记录但**默认不实现**（标可选 task），①完成后若仍有可测录音延迟才启动。
- **#4** `paste.rs` 的 `sleep(50ms)`：评估后保留，本文档说明理由。

## 3. 关键约束：两份 VAD 实例不可共享

- **detection vad**（579/629/668）：跨 tick 累积 LSTM hidden state——这是「检测连续语音/静音」所需特性。`OnceLock` 长期持有同一实例，**累积 state 是设计意图，不是 bug**。
- **filter_vad**（675）：每段开始 `reset()` 清空 state。实例可 `OnceLock` 复用 + 每段 `reset()`，等价于每段新实例但省掉 ONNX 重加载。

若误让 filter 与 detection 共用同一实例（或 reset 串了），累积/清空语义互相污染，VAD 检测失常。故**两个独立缓存**。

> filter_vad 复用共享 Arc + 每段 reset 安全的前提：coordinator 是**单线程顺序**处理语音段（一段一段 reset→处理→下一段 reset），同一实例无并发 reset 竞争。

## 4. 方案

### 4.1 asr::vad 缓存层

文件：`crates/asr/src/vad.rs`

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

// detection 实例：跨 tick 复用（不 reset）
static DETECTION_VADS: OnceLock<Mutex<HashMap<PathBuf, Arc<SileroVad>>>> = OnceLock::new();
// filter 实例：每段 reset 复用
static FILTER_VADS: OnceLock<Mutex<HashMap<PathBuf, Arc<SileroVad>>>> = OnceLock::new();

/// 取/加载 detection VAD（跨 tick 复用，调用方不应 reset）。
pub fn detection_vad(path: &Path) -> anyhow::Result<Arc<SileroVad>> { /* get_or_load */ }

/// 取/加载 filter VAD（复用实例，调用方按段 reset）。
pub fn filter_vad(path: &Path) -> anyhow::Result<Arc<SileroVad>> { /* get_or_load */ }
```

设计要点：
- 缓存按 model 目录 `path` 作 key——多模型切换时各自缓存，切回命中。
- 返回 `Arc<SileroVad>`：coordinator 持有 clone 跨 tick/跨段复用。
- filter 的 `reset()` 仍在 coordinator 每段开始调用（**不在缓存层 reset**，避免 detection 误触发）。

### 4.2 coordinator 改造

4 处替换：

| 行 | 原 | 改 |
|---|---|---|
| 579 / 629 / 668 | `SileroVad::new(&path)` | `vad::detection_vad(&path)` |
| 675 | `SileroVad::new(&path)` | `vad::filter_vad(&path)` |

filter 每段 reset 逻辑保持不变。

### 4.3 main.rs preheat 扩展

```rust
if do_preheat {
    // 现有：preheat ASR active_model（不变）
    // 新增：预加载 detection VAD 到缓存（路径同 coordinator 取的 vad_path）
    if let Some(vad_path) = resolve_vad_path(&config) {
        if let Err(e) = octopus_asr::vad::detection_vad(&vad_path) {
            log::warn!("VAD 预加载失败（不影响启动，首次录音时重试）: {}", e);
        }
    }
}
```

说明：VAD 预加载失败不阻断启动（降级为首次 Toggle 懒加载，等同现状）。

### 4.4 ③ lock-free 音频（可选 task，默认不做）

方案：SPSC ring buffer（`rtrb` crate），audio 回调生产、coordinator 消费，替换 `Arc<Mutex<Vec<f32>>>`。`drain_samples` 改为从 ring 批量 pop。

**标可选**：当前无证据锁是瓶颈（项目稳定、写读比悬殊）；①完成后若 profiling 显示录音热路径有可测延迟再启动。plan 内列为可选 task，附实现草图但不强制。

### 4.5 #4 paste sleep(50ms) — 保留

`paste.rs:89/119` 的 `sleep(50ms)`。理由：目标应用（聚焦的输入框）同步读剪贴板，50ms 是 enigo `Cmd+V` 后等待系统粘贴完成的经验下限；过短会漏字/截断。**非性能问题，是时序保证，不动**。

## 5. 文件清单

| 文件 | 改动 |
|---|---|
| `crates/asr/src/vad.rs` | 新增 `DETECTION_VADS` / `FILTER_VADS` 缓存 + `detection_vad` / `filter_vad` |
| `crates/desktop/src/main.rs` | preheat 分支扩预加载 detection VAD |
| `crates/desktop/src/coordinator.rs` | 4 处 `SileroVad::new` 改 `detection_vad` / `filter_vad` |
| `Cargo.toml`（asr / desktop） | 仅启动 ③ 时加 `rtrb`（可选） |

**不动**：`asr/engine.rs:143`（cli/server 路径的 `SileroVad::new`，非热路径，本次不改）。

## 6. 风险

- **中**。filter_vad 共享 Arc + 每段 reset 的单线程安全性需单测覆盖（reset 后 state 清空、detection 不被误 reset 污染）。
- 多模型切换缓存增长：HashMap 按 path 缓存，实际模型数量有限（个位数），无泄漏。
- `Arc<SileroVad>` 若 `SileroVad` 内部含不可 `Send + Sync` 字段（如 `RefCell`）→ 编译期暴露，届时调整。**需在实现首步验证 `SileroVad: Send + Sync` 可行性**。

## 7. 验证

- `cargo test -p octopus-asr`：新增 vad 缓存单测——同 path 返回同一 Arc（`Arc::ptr_eq`）、不同 path 不同、filter 实例 reset 后行为等价新实例。
- `cargo check --workspace`。
- 手动：首次按快捷键 → 录音启动延迟显著降低；连续切换多个 ASR 模型无异常。
