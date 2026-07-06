# Rust 代码审查报告（rust-patterns skill）

- 日期：2026-07-05
- 分支：`rust-review-2026-07-05`（基于 main 6ce7b36）
- 方法：clippy + rg 统计 + 人工抽样，对照 rust-patterns skill 六大领域

## 总览

| 指标 | 数值 |
|---|---|
| Rust 文件 | 107 |
| clippy 警告 | 0 |
| 测试 | 269 passed, 0 failed, 7 ignored |
| unwrap() 总量 | 464（其中约 350+ 在 `#[cfg(test)]`） |
| 非测试 unwrap() | 约 30 处 |
| unsafe 块 | 约 27 处（均有 SAFETY 文档） |
| pub(crate) 使用 | 仅 asr-local(37) / asr-cloud(5) |

**整体评价：代码质量较高。** 错误处理规范化（thiserror + anyhow），枚举穷尽匹配，unsafe 有文档，clippy 零警告。

---

## 亮点

### 1. 错误处理典范（download crate）
`crates/download/src/core/error.rs` 是 thiserror 最佳实践：
- 穷尽枚举 `DownloadError`，无 `Box<dyn Error>`
- `TransientKind` + `ErrorClass` 将非法状态排除出类型系统
- 状态分类 `classify_status` 穷尽匹配，无通配符

### 2. unsafe 全部有 SAFETY 文档
所有 unsafe 块/impl 均带注释说明安全性不变量：
- `denoise.rs:88-105` — `Df3Backend` 的 `Send/Sync` impl 有单线程串行访问文档
- `screenshot_commands.rs` — CoreGraphics FFI 边界有空指针检查
- `audio.rs` — ByteBuffer 转换有对齐说明

### 3. 枚举状态建模
download crate 的 `ErrorClass::Fatal | Transient(TransientKind)` 体现了"非法状态不可表示"原则。

---

## 问题清单

### P1（应修复）

#### P1-1: Mutex lock().unwrap() 在生产路径（poisoned panic 风险）

**位置**：
- `crates/cli/src/main.rs` — 10 处
- `crates/download/src/core/downloader.rs:285`
- `crates/server/src/pipeline.rs:116`

**问题**：`Mutex::lock().unwrap()` 在 Mutex 中毒（持有锁的线程 panic）时会 panic 传播。rust-patterns skill 要求"切勿在生产环境中使用 unwrap()"。

**建议**：替换为 `.lock().unwrap_or_else(|e| e.into_inner())`（恢复中毒 Mutex），或用 helper 函数封装。

#### P1-2: HeaderValue::parse().unwrap() 在生产路径

**位置**：`crates/desktop/src/settings_commands.rs:449`

```rust
format!("bearer {}", entry.secret_key).parse().unwrap(),
```

**问题**：`secret_key` 含非法 HTTP header 字符时 panic。
**建议**：`.map_err(|e| format!("secret_key 含非法字符: {}", e))?`

#### P1-3: 生产路径中 ndarray as_slice().unwrap()

**位置**：
- `crates/asr-local/src/streaming_paraformer.rs:607, 663, 757`

**问题**：`as_slice()` 在 ndarray 非连续内存时返回 None。当前代码依赖 ONNX 输出连续布局——若模型升级导致布局变化，静默 panic。
**建议**：`.ok_or_else(|| anyhow!("encoder output non-contiguous"))?`，或 `.to_vec()` 兜底。

### P2（可改进）

#### P2-1: pub(crate) 使用不足

infra / download / clipboard / llm / ocr / capx 六个 crate **零 pub(crate)**。作为底层库，内部辅助函数应优先 `pub(crate)` 而非 `pub`，最小化公开 API 面。

**重点**：
- `infra/src/db.rs` — 42 个 `pub fn`，部分是内部辅助（如 `migrate_yaml_key`、`init_schema`）
- `clipboard/src/` — 39 个 `pub`，store.rs 内部函数可收窄

#### P2-2: block_on 在 coordinator 主线程

**位置**：`crates/desktop/src/cloud_pipeline.rs:122`

```rust
tauri::async_runtime::block_on(async {
    octopus_asr_cloud::open_cloud_session(...)
})
```

注释说明 `open` 只 spawn task 立即返回，不阻塞。但 `block_on` 在 coordinator 主线程仍是反模式（若 open 内部 await 建连变更则卡住主线程）。
**建议**：长期改为 channel / spawn 接管，消除 block_on。

#### P2-3: std::thread::sleep 在多个非 async 路径

`paste.rs`、`pin_window.rs`、`clipboard_commands.rs`、`coordinator.rs` 共 10 处 `std::thread::sleep`。大部分在专用线程（coordinator tick 循环），可接受；但 `paste.rs` 的 sleep 在模拟键盘粘贴时阻塞调用线程，若被 async 调用则有风险。

### P3（建议）

#### P3-1: #[must_use] 缺失

155 个 `pub fn -> Result<>` 均无 `#[must_use]`。虽然 `Result` 类型本身在 Rust 中有 `#[must_use]`（编译器内置），但显式标注有助于文档生成。

#### P3-2: db.rs 的 migrate_yaml_key unwrap

`crates/infra/src/db.rs:236`：

```rust
let old_val = map.remove(&old_key).unwrap();
```

在 `if map.get(&old_key).is_some()` 守护下逻辑安全，但惯用法应改用 `if let`：

```rust
if let Some(old_val) = map.remove(&old_key) {
    map.insert(new_key, old_val);
}
```

#### P3-3: 通配符匹配仅 2 处（UI 边界）

`screenshot_commands.rs:1135, 1182` 的 `_ => return/continue`。在 UI 事件回调中可接受——丢失帧的降级语义正确。

---

## 不需要改动的部分

| 领域 | 状态 |
|---|---|
| 测试中 unwrap() | 正常——测试 panic 是期望行为 |
| ONNX session.run().unwrap() 在测试 | 正常——测试依赖模型文件存在 |
| stitch.rs 24 处 unwrap | 均在 `!is_empty()` / 边界检查守护下 |
| zipformer.rs:1059 unwrap | 在 `!ans.is_empty()` 守护下 |

---

## 建议优先级

1. **P1-1**（Mutex unwrap）— 最常见，改起来机械
2. **P1-2**（Header parse unwrap）— 单点修复
3. **P1-3**（ndarray as_slice unwrap）— 需确认 ONNX 布局契约
4. **P2-1**（pub(crate) 收窄）— 渐进式改进，每次改动相关文件时顺手
5. P2-2 / P2-3 — 架构层面，长期演进
