# ASR 引擎架构重构与优化方案 (方案 B)

## 1. 核心问题背景

当前 `octopus-asr` 核心推理库中，每个模型引擎（如 `whisper`, `sensevoice`, `qwen3_asr`）都采用基于无状态的函数式 API：
```rust
pub fn transcribe(samples: &[f32], language: &str) -> Result<String>
```

这种无状态的设计在被 `octopus-server`（HTTP API / WebSocket）和 `octopus-desktop`（Tauri 桌面应用）调用时，会产生严重的性能缺陷：
- **重复构建会话**：每次调用 `transcribe` 时，程序都需要从磁盘读取 ONNX 文件并调用 `Session::builder()?.commit_from_file(...)` 构建 ONNX 推理会话（Qwen3 / Whisper 需要加载并编译 3 个 Session）。大模型的初始化与核函数编译过程可能耗时 **数秒至十几秒**。
- **重复加载 Tokenizer**：每次识别都需要从磁盘加载并解析庞大的 `vocab.json` 和 `merges.txt`，产生不必要的 CPU & I/O 阻塞。
- **无法高并发响应**：在高频调用场景下，服务器会被模型加载占满 CPU 从而产生 OOM 或响应超时。

---

## 2. 改造后目标架构 (Stateful Engine API)

将无状态的函数接口改造成有状态的、生命周期长驻的 **ASR 引擎结构体**（Engine Struct）。每个模型都有一个代表其生存期的结构体，它在程序启动或切换模型时加载一次，之后的识别请求全部零拷贝、零重构地复用已编译好的 ONNX Session。

### 2.1 引擎抽象设计

为所有离线 ASR 引擎定义统一的 trait [OfflineAsrEngine](file:///Users/wudarui/workspace/agent/octopus/crates/asr/src/lib.rs)，方便上层应用动态分发：

```rust
use anyhow::Result;

pub trait OfflineAsrEngine: Send + Sync {
    /// 执行语音识别
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String>;
}
```

针对各个引擎定义具体的结构体，实现长驻 Session 与 Tokenizer：

```rust
// 示例：Qwen3-ASR 结构体
pub struct Qwen3AsrEngine {
    conv_session: ort::session::Session,
    encoder_session: ort::session::Session,
    decoder_session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
}

impl Qwen3AsrEngine {
    /// 初始化加载模型（仅在启动或切换模型时调用一次）
    pub fn new(model_entry: &config::ModelEntry) -> Result<Self> {
        // 1. 发现并加载 conv_frontend, encoder, decoder 并在内存中编译 Session
        // 2. 加载并对齐 Tokenizer
        // 3. 返回持有会话的实例
    }
}

impl OfflineAsrEngine for Qwen3AsrEngine {
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String> {
        // 复用 self 中的已编译 sessions 进行零拷贝特征提取与自回归推理
    }
}
```

同样的结构适用于：
- `WhisperEngine` (持有 encoder, dec_init, dec_past 和 tokenizer)
- `SenseVoiceEngine` (持有 model 和 token 映射列表)
- `ParaformerEngine` (持有 encoder 和 decoder)
- `ZipformerEngine` (持有 model 和 vocab 映射列表)

---

## 3. 引擎管理器 (AsrEngineManager)

为了在上层组件（CLI, Server, Desktop）中支持动态模型切换和管理，我们需要设计一个引擎管理器：

```rust
use std::sync::Arc;
use parking_lot::RwLock;

pub struct AsrEngineManager {
    // 缓存已经加载过的模型，避免重复加载
    cached_engines: RwLock<HashMap<String, Arc<dyn OfflineAsrEngine>>>,
    // 当前激活的引擎
    active_engine: RwLock<Option<Arc<dyn OfflineAsrEngine>>>,
    active_engine_name: RwLock<String>,
}

impl AsrEngineManager {
    pub fn new() -> Self {
        Self {
            cached_engines: RwLock::new(HashMap::new()),
            active_engine: RwLock::new(None),
            active_engine_name: RwLock::new(String::new()),
        }
    }

    /// 切换当前激活的 ASR 模型
    pub fn switch_model(&self, model_name: &str) -> Result<()> {
        // 1. 检查是否在 cached_engines 中
        // 2. 如果没有，则读取配置并 new 出对应的 Engine 放入 cache
        // 3. 更新 active_engine
        Ok(())
    }

    /// 获取当前激活的引擎进行 transcribe
    pub fn transcribe(&self, samples: &[f32], language: &str) -> Result<String> {
        if let Some(engine) = self.active_engine.read().clone() {
            engine.transcribe(samples, language)
        } else {
            anyhow::bail!("No active ASR engine loaded")
        }
    }
}
```

> **注意**：由于 `ort::session::Session` 内部基于 ONNX Runtime 的 `run` API 是线程安全的（支持多线程并发推断），因此 `Arc<dyn OfflineAsrEngine>` 可以在 Web Server 和 GUI App 的并发请求中直接并发调用 `transcribe` 方法，不需要加 Mutex 互斥锁。

---

## 4. 上层应用集成与改造步骤

### 4.1 octopus-server 改造

在 [crates/server/src/main.rs](file:///Users/wudarui/workspace/agent/octopus/crates/server/src/main.rs) 中：
1. **修改 AppState**：将共享状态 `AppState` 中的 `asr_engine: String` 替换为 `engine_manager: Arc<AsrEngineManager>`：
   ```rust
   #[derive(Clone)]
   struct AppState {
       engine_manager: Arc<AsrEngineManager>,
   }
   ```
2. **初始化加载**：在 `main()` 中启动服务前，读取默认配置并加载激活模型：
   ```rust
   let manager = Arc::new(AsrEngineManager::new());
   manager.switch_model(&config.asr.active)?;
   ```
3. **接口调用**：
   - 在 HTTP POST `/transcribe` 接口中，直接调用：
     ```rust
     state.engine_manager.transcribe(&samples, language)
     ```
     此时请求耗时将**彻底消除模型加载的数秒级延迟**，仅保留纯 GPU/CPU 推理时间（一般为数十至数百毫秒，速度提升可达 10~50 倍）。
   - 在 WebSocket 路由 `/ws/stream` 中同理。

### 4.2 octopus-desktop 改造

在桌面 Tauri 应用的 `src-tauri` 中：
1. 在 Tauri App 的 State 中注册 `engine_manager: Arc<AsrEngineManager>`。
2. 启动时进行一次 `switch_model` 进行背景预热。
3. 当用户通过设置界面修改使用的 ASR 模型时，触发 Tauri Command 调用 `engine_manager.switch_model(...)`。由于有了缓存池，重复切回已加载过的模型可以秒级完成。

### 4.3 兼容性保留（针对 CLI）

为了不破坏 `octopus-cli` 或简单测试用例的便捷性，原本的 free function 仍然保留：
```rust
pub fn transcribe(samples: &[f32], language: &str) -> Result<String> {
    // 依然走老逻辑：临时加载模型 -> transcribe -> 释放会话。
    // 这对于只执行一次的命令行工具非常适用，无需破坏其逻辑。
}
```

---

## 5. 改造收益与性能预期

| 指标 | 改造前 | 改造后 (方案 B) |
|------|------|------|
| **模型编译加载开销** | **每次请求均产生数秒延迟** | **仅在系统初始化/切换模型时产生一次开销** |
| **首字识别时间 (RTF)** | 极差 (RTF < 0.1) | **极佳 (RTF > 5.0+, CPU 推理)** |
| **高频请求/并发承载** | 会发生重复内存申请、OOM 崩溃 | 推理内存稳定，CPU 仅用于计算，无频繁 GC 和 heap fragmentation |
| **磁盘与 I/O 开销** | 每次推理重读数百MB文件，磁盘负荷大 | **零磁盘 I/O 重读开销** |
