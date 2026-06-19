# 启动/录音性能优化 Implementation Plan：VAD session 缓存（①），lock-free 音频（③可选）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 按任务实施。步骤用 `- [ ]` 跟踪。

**Goal:** VAD 的 ONNX Session 全局缓存，`SileroVad::new()` 廉价化（zeros h/c/sr + clone `Arc<Session>`），消除首次按快捷键的录音启动延迟；preheat 预加载 VAD。

**Architecture:** `vad.rs` 把 `session: Session` 改 `session: Arc<Session>`，加 `OnceLock<Mutex<HashMap<PathBuf, Arc<Session>>>>` 按 path 缓存已加载 Session。`SileroVad::new()` 命中缓存则 clone Arc + zeros，未命中才 `commit_from_file`。detection/filter 各自 owned `SileroVad`、h/c 独立——**coordinator 调用点零改动**。

**Tech Stack:** Rust, ort `2.0.0-rc.12`（`Session: Send + Sync`）, `std::sync::{Arc, Mutex, OnceLock}`, `std::collections::HashMap`。

**Spec:** `docs/superpowers/specs/2026-06-19-vad-preheat-design.md`（v2：session 级缓存）

---

## File Structure

- `crates/asr/src/vad.rs` — `SileroVad` struct 字段改 `Arc<Session>`；`new()` 加 session 缓存；加 `#[cfg(test)]` 单测。
- `crates/desktop/src/main.rs` — preheat 后台线程追加 VAD 预加载。
- `crates/desktop/src/coordinator.rs` — **不改**（`SileroVad::new(&path)` 签名/语义不变）。

---

### Task 1: vad.rs session 缓存 + struct 改 Arc<Session>

**Files:** Modify `crates/asr/src/vad.rs`

- [ ] **Step 1: 验证 `Session: Send + Sync`（决定 `Arc<Session>` 可行性）**

在 `vad.rs` 的 `use` 段之后加静态断言（永久保留，约束文档化）：

```rust
// 约束：Session 必须 Send + Sync，才能用 Arc<Session> 进全局缓存
// （OnceLock<Mutex<HashMap<PathBuf, Arc<Session>>>>）。ort 升级后此断言若失败，
// 回退方案见 spec §6（Arc<Mutex<Session>>）。
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Session>();
};
```

Run: `cargo check -p octopus-asr`
Expected: PASS（ort 2.x `Session` 线程安全）。

**若 FAIL（回退方案）**：struct 字段改 `session: Arc<Mutex<Session>>`，`new()` 缓存 `Arc::new(Mutex::new(session))`，`compute()` 改 `let session = self.session.lock().unwrap(); session.run(...)`（其余 outputs 提取不变，仍用 session guard）。后续 Task 按此调整。**本 plan 默认主方案（`Arc<Session>`）通过。**

- [ ] **Step 2: 加缓存层 import + static + helper**

`vad.rs` 顶部 import 段补：

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
```

（`use std::path::Path;` 保留；`use anyhow::{Context, Result};` 等不变）

struct 上方加：

```rust
/// 按 model 路径缓存已加载的 ONNX Session。
/// SileroVad 实例各自 owned h/c/sr，但共享底层 Session（Session::run 是 &self，线程安全）。
static VAD_SESSIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<Session>>>> = OnceLock::new();

fn vad_sessions() -> &'static Mutex<HashMap<PathBuf, Arc<Session>>> {
    VAD_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}
```

- [ ] **Step 3: struct 字段 `Session → Arc<Session>`**

```rust
pub struct SileroVad {
    session: Arc<Session>,   // 改：Session → Arc<Session>，跨实例共享
    h: Array3<f32>,          // [2, 1, 64]
    c: Array3<f32>,          // [2, 1, 64]
    sr: Array1<i64>,         // scalar: 16000
}
```

- [ ] **Step 4: 重写 `new()`（命中缓存则 clone Arc + zeros）**

```rust
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
    // compute / reset 实现不变（self.session.run 经 Arc deref 仍可用，无需改）
}
```

Run: `cargo check -p octopus-asr`
Expected: PASS（`self.session.run(...)` 在 `compute` 中经 `Arc` deref 编译通过）。

- [ ] **Step 5: TDD 单测——验证缓存共享**

在 `vad.rs` 末尾加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // 真实 ONNX 模型文件在测试环境可能不存在；find_silero_vad 或 new 失败时跳过，
    // 不 FAIL，避免 CI 强依赖模型文件（与项目其他 ASR 引擎测试一致）。
    fn try_new() -> Option<SileroVad> {
        let path = crate::config::find_silero_vad().ok()?;
        SileroVad::new(&path).ok()
    }

    #[test]
    fn same_path_shares_session() {
        // 核心：同 path 两次 new() 应共享同一 Session Arc（缓存生效）
        let (a, b) = match (try_new(), try_new()) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                println!("skip: 测试环境无 silero_vad 模型文件");
                return;
            }
        };
        assert!(
            Arc::ptr_eq(&a.session, &b.session),
            "同 path 应共享同一 Session Arc（缓存未生效？）"
        );
    }

    #[test]
    fn compute_returns_probability_in_range() {
        let mut v = match try_new() {
            Some(v) => v,
            None => {
                println!("skip: 测试环境无 silero_vad 模型文件");
                return;
            }
        };
        v.reset();
        let samples = vec![0.0f32; 480]; // 30ms @ 16kHz
        let prob = v.compute(&samples).expect("compute should succeed");
        assert!((0.0..=1.0).contains(&prob), "概率应在 [0,1]，实际 {}", prob);
    }
}
```

Run: `cargo test -p octopus-asr vad`
Expected: PASS（或 skip 若无模型文件；本地有模型时 `same_path_shares_session` 验证 `Arc::ptr_eq` 为 true）。

- [ ] **Step 6: commit**

```bash
git add crates/asr/src/vad.rs
git commit -m "perf(asr): SileroVad session 全局缓存，new() 廉价化（Arc<Session>）"
```

---

### Task 2: main.rs preheat 预加载 VAD session

**Files:** Modify `crates/desktop/src/main.rs:219-225`（`std::thread::spawn` 闭包内）

- [ ] **Step 1: 在 preheat 后台线程追加 VAD 预加载**

将现有 preheat 的 spawn 块：

```rust
                std::thread::spawn(move || {
                    if let Err(e) = em.switch_model(&active_model) {
                        log::error!("Failed to preheat active ASR model {}: {}", active_model, e);
                    } else {
                        info!("Active ASR model {} preheated successfully", active_model);
                    }
                });
```

改为（在 ASR preheat 之后追加 VAD 预加载，同一后台线程，不阻塞主线程启动）：

```rust
                std::thread::spawn(move || {
                    if let Err(e) = em.switch_model(&active_model) {
                        log::error!("Failed to preheat active ASR model {}: {}", active_model, e);
                    } else {
                        info!("Active ASR model {} preheated successfully", active_model);
                    }
                    // 预加载 VAD session 到全局缓存：首次 Toggle 命中缓存，消除录音启动延迟。
                    // 失败不影响启动（首次录音时 new() 会懒加载重试）。
                    if let Ok(vad_path) = octopus_asr::config::find_silero_vad() {
                        match octopus_asr::vad::SileroVad::new(&vad_path) {
                            Ok(_) => info!("VAD session preheated"),
                            Err(e) => log::warn!(
                                "VAD 预加载失败（不影响启动，首次录音懒加载）: {}", e
                            ),
                        }
                    }
                });
```

Run: `cargo check -p octopus-desktop --features dashscope`
Expected: PASS

- [ ] **Step 2: commit**

```bash
git add crates/desktop/src/main.rs
git commit -m "perf(desktop): preheat 后台线程预加载 VAD session"
```

---

### Task 3（可选，默认跳过）: ③ lock-free 音频 ring buffer

> **当前不做**。无证据 `Arc<Mutex<Vec>>` 是瓶颈（项目稳定、写读比悬殊）。①完成后若 profiling 显示录音热路径有可测延迟再启动。

草图（供未来参考）：`audio.rs` 回调改 rtrb SPSC 生产、`SharedAudioState` 持 `rtrb::Producer`、coordinator `drain_samples` 持 `rtrb::Consumer` 批量 pop。需加 `rtrb` 依赖 + 重写 `drain_samples` 消费端。风险中（音频热路径）。

- [ ] **评估而非实现**：仅在①完成后、有 profiling 数据支撑时转为正式 task。

---

### Task 4: 验证

- [ ] **Step 1: ASR 单测 + workspace 编译**

Run: `cargo test -p octopus-asr && cargo check --workspace --all-targets`
Expected: PASS，零 warning 回归。

- [ ] **Step 2: 手动验证（需 GUI 环境）**

- 首次按快捷键 → 录音启动延迟显著降低（preheat 已预加载）
- 连续切换多个本地 ASR 模型 → 各自首次 Toggle 后续命中缓存，无卡顿
- detection（流式/云）/ filter（分段）两种 VAD 语义正常——语音边界、静音切断与现状一致

- [ ] **Step 3: 确认 coordinator 未被改动**

Run: `git diff main -- crates/desktop/src/coordinator.rs | grep -c "SileroVad\|vad::" ` 
Expected: `0`（coordinator 调用点零改动，验证方案核心收益）

---

## Self-Review

- **Spec 覆盖**：§4.1 vad session 缓存（Task 1）、§4.3 preheat（Task 2）、§4.4 ③可选（Task 3 评估）、§4.2 coordinator 不改（Task 4 Step 3 验证）、§7 单测（Task 1 Step 5）✓
- **Placeholder 扫描**：无 TBD/TODO；`new()`/struct/preheat 给完整代码；③ 标可选附草图不强制 ✓
- **类型一致**：`Arc<Session>` 贯穿 struct/new/test（`Arc::ptr_eq`）；`compute` 中 `self.session.run` 经 deref 不变；回退方案 `Arc<Mutex<Session>>` 在 Task 1 Step 1 单独说明 ✓
- **风险覆盖**：`Session: Send+Sync` 验证（Task 1 Step 1）+ 回退；测试环境无模型的 skip 兜底 ✓
