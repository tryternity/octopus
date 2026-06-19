# 启动/录音性能优化设计：VAD session 缓存（①），lock-free 音频（③可选）

> Date: 2026-06-19
> 状态：设计中
> 修订：v2。v1 为「实例级 `Arc<SileroVad>` 缓存」，读源码后发现 `compute(&mut self)` 需要 `&mut self`、`Arc<SileroVad>` 给不出可变借用且会污染 h/c。v2 改为「session 级缓存」——**coordinator 零改动**。

## 1. 背景

### 1.1 VAD 重复加载（① 主项）

coordinator 每次 Toggle（开始录音）实时构造 VAD：

- `coordinator.rs:606 / 656`（detection / streaming vad）+ 附近 filter_vad（VadSegmented 场景）调 `octopus_asr::vad::SileroVad::new(&path)`——内部 `Session::commit_from_file` 同步加载 ONNX，百 ms 级。**首次按快捷键 → 录音启动有明显延迟**。
- filter_vad 每个语音段都重新加载一次。
- `main.rs:210-226` preheat 只 preheat ASR model，**不碰 VAD**。

### 1.2 根因

`SileroVad::new()`（`vad.rs:16`）每次都 `Session::builder().commit_from_file(model_path)`——ONNX 加载是慢操作。而 `SileroVad` 的可变状态只有 `h` / `c`（LSTM hidden/cell，`Array3<f32>` 各 128 元素）+ `sr`（标量），zeros 是纳秒级。重加载的真正成本全在 Session。

### 1.3 音频回调锁（③，可选）

`audio.rs` 回调里 `mono.collect()` + 写 `Arc<Mutex<Vec<f32>>>`，coordinator `drain_samples` 持锁读。16kHz 语音，写远多于读，锁竞争低——项目长期稳定，无证据锁是瓶颈。

## 2. 目标

- **①**：VAD 的 ONNX Session 全局缓存（按 path），`SileroVad::new()` 变为廉价（zeros h/c/sr + clone `Arc<Session>`）；`main.rs` preheat 预加载，首次按下不再卡。
- detection / filter 语义完全不变（各自 owned `SileroVad`，h/c 独立）。
- **③**：方案记录但**默认不实现**。
- **#4**：保留。

## 3. 设计：session 级缓存（coordinator 零改动）

关键洞察：`SileroVad::compute(&mut self)` 只更新 `self.h` / `self.c`（自身字段），`self.session.run(...)` 只用不可变借用（`Session::run(&self)`，ort 2.x `Session` 线程安全）。故：

- **Session 共享**：多个 `SileroVad` 实例可共享同一 `Arc<Session>`，各自维护独立的 `h/c/sr`。
- **实例 owned**：detection 与 filter 仍是各自 owned 的 `SileroVad`——h/c 跨 tick 累积（detection）/ 每段 reset（filter）语义天然独立，**无需「两份缓存」**。

对比 v1（实例级 `Arc<SileroVad>` 缓存）的错误：`compute` 要 `&mut self`，`Arc<SileroVad>` 给不出可变借用，且共享实例会污染 h/c。session 级缓存彻底回避此问题，且 coordinator 调用点零改动。

## 4. 方案

### 4.1 vad.rs：session 缓存 + struct 改 Arc<Session>

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
// 其余 use 不变

static VAD_SESSIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<Session>>>> = OnceLock::new();

fn vad_sessions() -> &'static Mutex<HashMap<PathBuf, Arc<Session>>> {
    VAD_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct SileroVad {
    session: Arc<Session>,   // 改：Session → Arc<Session>，跨实例共享
    h: Array3<f32>,
    c: Array3<f32>,
    sr: Array1<i64>,
}

impl SileroVad {
    pub fn new(model_path: &Path) -> Result<Self> {
        // 先查缓存：命中则 clone Arc，避免重复 commit_from_file
        let session = {
            let cache = vad_sessions().lock().unwrap();
            cache.get(model_path).cloned()
        };
        let session = match session {
            Some(s) => s,
            None => {
                let s = Arc::new(
                    Session::builder()
                        .context("Failed to create ORT session builder")?
                        .commit_from_file(model_path)
                        .with_context(|| format!("Failed to load Silero VAD from {:?}", model_path))?
                );
                vad_sessions().lock().unwrap().insert(model_path.to_path_buf(), s.clone());
                s
            }
        };
        Ok(Self {
            session,
            h: Array3::zeros((2, 1, 64)),
            c: Array3::zeros((2, 1, 64)),
            sr: Array1::from_vec(vec![16000i64]),
        })
    }
    // compute / reset 实现不变（self.session.run 经 Arc deref 仍可用）
}
```

要点：
- 缓存键 `PathBuf`——多模型切换各自缓存，切回命中。
- 命中路径只 clone `Arc`（原子 inc，纳秒）+ zeros（128×2 浮点）——比 `commit_from_file` 快约千倍。
- `compute` / `reset` 实现不变（`self.session.run(...)` 经 `Arc` 自动 deref，编译通过）。

### 4.2 coordinator：不改

`SileroVad::new(&path)` 签名与语义不变，coordinator.rs:606 / 656 / 675 附近调用点**零改动**。Stage enum 的 vad 字段类型仍是 `SileroVad`（owned）——detection 跨 tick 累积、filter 每段 reset，行为与现状一致。

### 4.3 main.rs preheat 扩展（可选但推荐）

```rust
if do_preheat {
    // 现有：preheat ASR active_model（不变）
    // 新增：预加载 VAD session 到缓存
    if let Ok(vad_path) = octopus_asr::config::find_silero_vad() {
        if let Err(e) = octopus_asr::vad::SileroVad::new(&vad_path) {
            log::warn!("VAD 预加载失败（不影响启动，首次录音懒加载）: {}", e);
        }
    }
}
```

> 即便不加 preheat，首次 Toggle 的 `new()` 也会填充缓存，**后续 Toggle 全部命中**。preheat 只是把首次延迟从「第一次按快捷键」提前到「启动时」。

### 4.4 ③ lock-free 音频（可选，默认不做）

方案：SPSC ring buffer（`rtrb` crate），audio 回调生产、coordinator 消费，替换 `Arc<Mutex<Vec<f32>>>`。`drain_samples` 改为从 ring 批量 pop。

**标可选**：当前无证据锁是瓶颈；①完成后若 profiling 显示录音热路径有可测延迟再启动。plan 内列为可选 task，附实现草图但不强制。

### 4.5 #4 paste sleep(50ms) — 保留

`paste.rs:89 / 119` 的 `sleep(50ms)`。理由：目标应用（聚焦的输入框）同步读剪贴板，50ms 是 enigo `Cmd+V` 后等待系统粘贴完成的经验下限；过短会漏字/截断。**非性能问题，是时序保证，不动**。

## 5. 文件清单

| 文件 | 改动 |
|---|---|
| `crates/asr/src/vad.rs` | `SileroVad.session: Session → Arc<Session>`；`new()` 加 `VAD_SESSIONS` 缓存；加 `#[cfg(test)]` 单测 |
| `crates/desktop/src/main.rs` | preheat 分支加 VAD 预加载（可选） |
| `crates/desktop/src/coordinator.rs` | **不改**（调用点零改动） |
| `Cargo.toml` | 不动（③ 不启用） |

**不动**：`asr/engine.rs:143`（cli/server 路径，非热路径）。

## 6. 风险

- **低-中**。主要不确定性：ort `2.0.0-rc.12` 的 `Session: Send + Sync`（决定 `Arc<Session>` 能否进全局 `OnceLock<Mutex<HashMap>>`）。ort 2.x `Session` 标准线程安全（项目内各引擎均以 `Mutex<Session>` 跨线程持有，印证 `Session: Send`），但需 Task 1 首步 `cargo check` 验证；若失败回退 `Arc<Mutex<Session>>`（`run` 仍 `&self`，`Mutex` 仅满足 `Sync` 约束，单 VAD 调用无竞争）。
- detection / filter 语义不变（owned + h/c 独立），无需额外单测覆盖 reset 串扰（v1 方案的顾虑在此消失）。
- 多模型切换缓存增长：`HashMap` 按 path，模型数有限（个位数），无泄漏。

## 7. 验证

- `cargo test -p octopus-asr`：新增缓存单测——同 path 两次 `new()` 共享同一 `Session`（同模块 `#[cfg(test)]` 内 `Arc::ptr_eq(&a.session, &b.session)`）、不同 path 不共享。
- `cargo check --workspace`。
- 手动：首次按快捷键 → 录音启动延迟显著降低；连续切换多个 ASR 模型无异常。
