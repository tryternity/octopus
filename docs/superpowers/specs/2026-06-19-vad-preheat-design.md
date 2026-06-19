# 启动/录音性能优化设计：VAD session 缓存（①），lock-free 音频（③可选）

> Date: 2026-06-19
> 状态：已实现（commits `c15c159` + `569f94b` + `07a1503`）
> 修订：v3。v2 主方案 `Arc<Session>`（论据「`Session::run(&self)`」）有误——ort 源码 `session/mod.rs:212` 确认 `Session::run` 是 `&mut self`，`Arc<Session>` 编译失败（deref 只给 `&Session`）。v3 主方案改为 `Arc<Mutex<Session>>`（Mutex 提供内部可变性）。`Session: Send + Sync` 断言通过——回退非因 Send/Sync，纯因 `run &mut self`。

## 1. 背景

### 1.1 VAD 重复加载（① 主项）

coordinator 每次 Toggle（开始录音）实时构造 VAD：

- `coordinator.rs:606 / 656`（detection / streaming vad）+ filter_vad（VadSegmented 场景）调 `octopus_asr::vad::SileroVad::new(&path)`——内部 `Session::commit_from_file` 同步加载 ONNX，百 ms 级。**首次按快捷键 → 录音启动有明显延迟**。
- filter_vad 每个语音段都重新加载一次。
- `main.rs:210-226` preheat 只 preheat ASR model，不碰 VAD。

### 1.2 根因

`SileroVad::new()` 每次都 `Session::builder().commit_from_file(model_path)`——ONNX 加载慢。而 `SileroVad` 的可变状态只有 `h`/`c`（LSTM hidden/cell，各 128 元素）+ `sr`（标量），zeros 是纳秒级。重加载成本全在 Session。

### 1.3 音频回调锁（③，可选）

`audio.rs` 回调里 `mono.collect()` + 写 `Arc<Mutex<Vec<f32>>>`，coordinator `drain_samples` 持锁读。16kHz 语音，写远多于读，锁竞争低——项目长期稳定，无证据锁是瓶颈。

## 2. 目标

- **①**：VAD 的 ONNX Session 全局缓存（按 path），`SileroVad::new()` 廉价化；`main.rs` preheat 预加载，首次按下不再卡。
- detection/filter 语义完全不变（各自 owned `SileroVad`，h/c 独立）。
- **③**：方案记录但默认不实现。
- **#4**：保留。

## 3. 设计：session 级缓存（coordinator 零改动）

`SileroVad::compute(&mut self)` 只更新 `self.h`/`self.c`（自身字段）；`self.session.run(...)` 需要 `&mut Session`（ort 2.x `Session::run(&mut self)`，源码 `session/mod.rs:212`）。故：

- **Session 共享**：多个 `SileroVad` 实例共享同一 `Arc<Mutex<Session>>`——`Mutex` 提供内部可变性，满足 `run` 的 `&mut self`；`Arc` 提供共享所有权。
- **实例 owned**：detection 与 filter 各自 owned `SileroVad`，h/c 跨 tick 累积（detection）/ 每段 reset（filter）独立——无需两份缓存。
- `Session: Send + Sync`（编译期静态断言保证），可入全局 `OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>>>`。

coordinator 调 `SileroVad::new(&path)`（签名/语义不变），**零改动**。

> 对比 v2 错误论据：「`run(&self)` 线程安全，故 `Arc<Session>` 可共享」——实际 `run` 是 `&mut self`。v3 改 `Arc<Mutex<Session>>`。

## 4. 方案（已实现）

### 4.1 vad.rs：session 缓存 + Arc<Mutex<Session>>

```rust
static VAD_SESSIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>>> = OnceLock::new();

pub struct SileroVad {
    session: Arc<Mutex<Session>>,
    h: Array3<f32>,
    c: Array3<f32>,
    sr: Array1<i64>,
}

impl SileroVad {
    pub fn new(model_path: &Path) -> Result<Self> {
        // 持 cache lock 完成 get-or-insert（消除 TOCTOU）：
        // 并发 miss 时只有一个线程加载，其余等锁后命中同一 Arc。
        let session = {
            let mut cache = vad_sessions().lock().unwrap();
            if let Some(s) = cache.get(model_path) {
                s.clone()
            } else {
                let s = Arc::new(Mutex::new(
                    Session::builder()?
                        .commit_from_file(model_path)?,
                ));
                cache.insert(model_path.to_path_buf(), s.clone());
                s
            }
        };
        Ok(Self { session, h: zeros, c: zeros, sr })
    }

    pub fn compute(&mut self, samples: &[f32]) -> Result<f32> {
        // ... 准备 input/h/c/sr tensor ...
        let mut session = self.session.lock().unwrap();   // &mut Session for run
        let outputs = session.run(...)?;
        // outputs 是 owned，guard 在函数末 drop；h/c 更新不依赖 session lock
    }

    pub fn reset(&mut self) { /* 只清 h/c，不碰 session，不需 lock */ }
}
```

要点：
- **持锁 get-or-insert**（commit `07a1503`）：消除原两步模式（`get→drop→load→re-lock insert`）的 TOCTOU；并发 miss 只加载一次。持锁期间 `commit_from_file`（~100ms）仅冷启动，可接受。
- `compute` 持 session lock 仅覆盖 `run`（outputs owned，h/c 更新独立）；`reset` 不锁 session。
- 单线程 coordinator 调用，session lock 无竞争。

### 4.2 coordinator：不改

`SileroVad::new(&path)` 签名/语义不变，coordinator.rs:606/656/675 调用点零改动。Stage enum 的 vad 字段仍是 owned `SileroVad`。

### 4.3 main.rs preheat（已实现）

preheat 后台线程闭包内，ASR `switch_model` 之后追加 VAD 预加载（`main.rs:227-234`）：

```rust
if let Ok(vad_path) = octopus_asr::config::find_silero_vad() {
    match octopus_asr::vad::SileroVad::new(&vad_path) {
        Ok(_) => info!("VAD session preheated"),
        Err(e) => log::warn!("VAD 预加载失败（不影响启动，首次录音懒加载）: {}", e),
    }
}
```

### 4.4 ③ lock-free 音频（可选，未实现）

无证据锁是瓶颈；①完成后若 profiling 显示热路径延迟再启动。

### 4.5 #4 paste sleep(50ms) — 保留

时序保证，不动。

## 5. 文件清单（实际）

| 文件 | 改动 |
|---|---|
| `crates/asr/src/vad.rs` | `session: Arc<Mutex<Session>>`；`VAD_SESSIONS` 缓存 + 持锁 get-or-insert；compute lock；2 单测；Send+Sync 断言 |
| `crates/desktop/src/main.rs` | preheat 后台线程加 VAD 预加载 |
| `crates/desktop/src/coordinator.rs` | 不改（零改动已验证） |

## 6. 风险（已验证）

- ~~`Session: Send + Sync` 不确定~~：断言通过（commit `c15c159`）。
- ~~`run &self`~~：实际 `&mut self` → `Arc<Mutex<Session>>`（v3 修正）。
- ~~TOCTOU（`get→drop→load→re-lock insert`）~~：持锁 get-or-insert 修复（commit `07a1503`），3 次多线程测试无 flake。
- poison panic（`.lock().unwrap()`）：compute/new panic 会 poison，下次 new panic。生产可接受（孤立 panic 本就需重启）；未做 `into_inner()` 容错（可选，非阻塞）。

## 7. 验证

- `cargo test -p octopus-asr`：42 passed, 6 ignored（dashscope 等需真实 key）。
- `cargo check --workspace --all-targets`：clean。
- coordinator 零改动（`git diff` 空）。
- 手动：首次按快捷键录音启动延迟显著降低（待用户本地确认）。
