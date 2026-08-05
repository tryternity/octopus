# macOS ConcealedType 跳过记录

- 日期：2026-08-05
- 类型：安全增强（bug fix 类，防敏感数据泄露）
- 优先级：P3
- 依赖：`clipboard-rs 0.3.5`（已含 `ContentFormat::Other` 任意类型检测）

## 1. 背景与问题

macOS 密码管理器（1Password / Bitwarden / iCloud Keychain / KeePassXC / Maccy 等）复制密码 / 一次性验证码（TOTP）时，会在 NSPasteboard 上额外标记 `org.nspasteboard.ConcealedType` 类型——这是 [nspasteboard.org 社区约定](https://nspasteboard.org/)，明确告知剪贴板消费方「这是敏感数据，不要记录」。

**当前问题**：octopus 的剪贴板 watcher（`handle_clipboard_change`，`crates/clipboard/src/watcher.rs`）在判断 files / image / text 类型时**没有检测 ConcealedType**，会把密码内容当 text 入库：

```
1Password 复制密码 → NSPasteboard 标记 ConcealedType + 写 password 文本
→ octopus watcher 触发 → handle_clipboard_change → 走 text 分支
→ insert_clipboard_item 写入 SQLite clipboard_history（content=明文密码）
→ FTS5 索引（clipboard_history_fts）收录明文密码（可被搜索）
→ 跨设备 sync（如果被收藏，favorites 同步会传播）
```

**这是敏感数据泄露风险**——密码明文存本地 DB + FTS 索引 + 潜在跨设备传播。

## 2. 方案

### 2.1 核心改动

在 `crates/clipboard/src/watcher.rs::handle_clipboard_change` 函数开头（files / image / text 三分支**之前**）加 ConcealedType 检测，命中则静默 `return Ok(())`：

```rust
pub fn handle_clipboard_change(handle: &ClipboardHandle) -> anyhow::Result<()> {
    // ── ConcealedType 检测（macOS 密码管理器保护）──
    // 1Password / Bitwarden / iCloud Keychain 等复制密码时标记此类型，
    // 明确告知消费方不要记录。静默跳过避免密码明文入库 + FTS 索引 + 跨设备同步。
    // 仅 macOS——Windows/Linux 无 ConcealedType 概念。
    #[cfg(target_os = "macos")]
    {
        const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";
        if handle.has(ContentFormat::Other(CONCEALED_TYPE.to_string())) {
            return Ok(());
        }
    }

    // 按优先级判断类型：files > image > text（现有逻辑）
    let result: anyhow::Result<()> = (|| {
        if handle.has(ContentFormat::Files) { /* ... */ }
        else if handle.has(ContentFormat::Image) { /* ... */ }
        else if handle.has(ContentFormat::Text) { /* ... */ }
        Ok(())
    })();
    // ... 现有后续
}
```

### 2.2 为什么用 `ContentFormat::Other` 而非 objc2

| 方案 | 评估 | 结论 |
|---|---|---|
| **`handle.has(ContentFormat::Other(String))`**（选用） | 复用现有 `ClipboardHandle` 抽象，零新依赖。clipboard-rs 0.3.5 macOS 后端 `has()` 的 Other 分支走 `availableTypeFromArray(&[NSString])`（platform/macos.rs:285-288），支持任意 pasteboard 类型字符串。改动：watcher.rs 加 5 行（含 `#[cfg]`） | ✅ 最小改动 |
| objc2-app-kit 直接读 `NSPasteboard::generalPasteboard().dataForType(...)` | 完全可控，与现有 `vault/autotype/clipboard.rs` concealed 写入对称。但 `crates/clipboard` 当前不直接依赖 objc2-app-kit，需加平台 cfg 分支 + Cargo.toml 改动 | ❌ 过度工程 |
| `handle.available_formats().iter().any(\|t\| t == CONCEALED_TYPE)` | 同样零依赖，但 `available_formats()` 返回 `Vec<String>`（底层 `[pb types]`），热路径上比 `has()` 多一次 Vec 分配 | ❌ 不如 has 轻量 |

### 2.3 常量位置

`CONCEALED_TYPE` 常量在 watcher.rs 本地定义（不跨 crate 引用 `vault/autotype/clipboard.rs` 的同名常量）——两者语义虽同但分属不同 crate（clipboard vs desktop/vault），跨 crate 引用会增加耦合。如果后续多处共用，再考虑提到 `octopus-infra::consts`。

### 2.4 octopus autotype 兼容

octopus 自己的 vault autotype 复制密码时也写 ConcealedType（`vault/autotype/clipboard.rs::copy_concealed`），调用方目前手动调 `clipboard.suppress_next()` 防止 watcher 记录（3 处：autotype.rs:134-137 / 335-336 / 369）。

**本改动不改 autotype 调用方**——`suppress_next` 机制保留，原因：

1. **跨平台保底**：Windows / Linux 没有 ConcealedType，靠 `suppress_next` 保证 octopus 自己的 autotype 不被记录
2. **双重保险**：macOS 上即使 `suppress_next` 因任何原因失效（理论上不会），新增的 ConcealedType 检测也能兜底跳过
3. **最小改动**：不动现有 3 处 `suppress_next` 调用，降低回归风险

watcher 的 `on_clipboard_change` 回调顺序：
```
1. check_and_clear_suppress() → 命中 return（octopus autotype 走这里）
2. is_recording_enabled() gate
3. (on_change)() → enqueue → worker → handle_clipboard_change
   → 新增：ConcealedType 检测 → 命中 return（第三方密码管理器走这里）
   → files / image / text 分支
```

## 3. 影响面

| 场景 | 当前行为 | 改动后 |
|---|---|---|
| 1Password 复制密码 | ❌ 入库（明文 + FTS 索引） | ✅ 静默跳过 |
| Bitwarden 复制 TOTP | ❌ 入库 | ✅ 静默跳过 |
| iCloud Keychain 自动填充密码 | ❌ 入库 | ✅ 静默跳过 |
| Maccy（遵守 nspasteboard 约定）复制 | ❌ 入库（Maccy 自己会标 ConcealedType） | ✅ 静默跳过 |
| 普通文本复制（无 ConcealedType） | 入库 | 入库（无影响） |
| 图片 / 文件复制 | 入库 | 入库（无影响） |
| octopus autotype 复制密码 | suppress_next 跳过 | suppress_next 跳过（macOS 双保险） |

## 4. 不变量

1. **ConcealedType 命中 → 不入库、不 emit `clipboard://changed`、不留任何痕迹**
2. **非 macOS 平台 → 此检测不生效**（平台隔离，Windows / Linux 无 ConcealedType 概念）
3. **octopus autotype 的 suppress_next 机制不变**（跨平台保底 + macOS 双重保险）
4. **检测点在 files / image / text 分支之前**（避免误入 text 分支入库后再过滤）

## 5. 测试

### 5.1 单测局限

clipboard-rs 的 `has()` 走真 NSPasteboard（`ClipboardContext` 持 `Retained<NSPasteboard>`，私有字段），单测难以 mock `has(ContentFormat::Other(...))` 的返回值。现有 watcher.rs 的单测（如 `test_handle_clipboard_change_*`）用临时 in-memory DB + 真 `ClipboardContext`，但构造 ConcealedType 需要写真 pasteboard。

### 5.2 集成测试方案

加一个 `#[cfg(test)] #[cfg(target_os = "macos")]` 测试：

```rust
#[test]
#[cfg(target_os = "macos")]
fn test_concealed_type_skipped() {
    // 1. 用 ClipboardContext::new() 拿真 pasteboard
    // 2. set_text("fake-password") + 手动 set ConcealedType 标记
    //    （需要 objc2-app-kit，或复用 vault/autotype 的 copy_concealed 逻辑）
    // 3. 调 handle_clipboard_change
    // 4. 查 DB 确认无 "fake-password" 条目
}
```

**实现挑战**：在 clipboard crate（不依赖 objc2-app-kit）里设 ConcealedType 标记需要绕路。两个选项：

- **选项 A**：测试放 `crates/desktop`（依赖 objc2-app-kit + clipboard），复用 `vault/autotype/clipboard.rs::copy_concealed` 设标记，调 `handle_clipboard_change` 验证
- **选项 B**：watcher.rs 内加 `#[cfg(test)]` helper 用 clipboard-rs 的 `set_text` + 手动 `ClipboardContext` 内部 API 设 ConcealedType（clipboard-rs 不暴露任意类型写入 API，此路不通）

**推荐选项 A**——测试放 desktop crate，复用现有 concealed 写入工具。但要注意 `copy_concealed` 走 suppress_next 路径会干扰——测试时需 bypass suppress（直接调 `copy_concealed` 后清 suppress flag，或绕过 autotype 层直接写 pasteboard）。

### 5.3 回归测试

确保现有 `test_handle_clipboard_change_text` / `test_handle_clipboard_change_image` 等测试仍通过（无 ConcealedType 标记时行为不变）。

## 6. YAGNI 边界

明确**不做**：

| 项 | 理由 |
|---|---|
| 占位条目（itemType='concealed'，显示「🔒 密码保护内容」） | YAGNI——静默跳过已满足核心诉求（防泄露）；占位条目需改 schema + FTS5 适配 + 前端渲染分支，过度工程化。如果后续用户反馈「想知道刚复制过密码」，再加 |
| 配置项开关（允许用户关闭 ConcealedType 跳过） | 密码安全是默认行为，不应暴露用户可关闭的选项（用户误关会导致密码泄露） |
| 跨平台 concealed 概念映射 | Windows / Linux 无对应标准 pasteboard 类型约定；macOS 独有 |
| 常量提到 infra::consts | 当前只有 watcher.rs（clipboard crate）和 vault/autotype/clipboard.rs（desktop crate）两处用，跨 crate 引用增加耦合；本地定义更清晰 |

## 7. 后续 follow-up（非本 spec 范围）

- **clipboard-rs 升级**：如果 clipboard-rs 未来版本原生支持 ConcealedType 检测（如新增 `ContentFormat::Concealed`），可迁移到原生 API
- **Windows Hello / Linux secret portal**：Windows / Linux 密码管理器可能有类似机制（如 Windows 的 `CF_CLIPBOARD_VIEWER_IGNORE`），后续调研

## 8. 参考实现

- **nspasteboard.org 约定**：https://nspasteboard.org/（ConcealedType / TransientType / AutoGeneratedType 等社区标准）
- **竞品参考**：Maccy（开源 macOS 剪贴板管理器）默认跳过 ConcealedType；Raycast 剪贴板历史同样跳过
- **现有 octopus concealed 写入**：`crates/desktop/src/vault/autotype/clipboard.rs:23-24`（`CONCEALED_TYPE` 常量 + `copy_concealed` 实现）
- **clipboard-rs Other 分支**：`~/.cargo/registry/.../clipboard-rs-0.3.5/src/platform/macos.rs:285-288`（`availableTypeFromArray` 支持任意类型字符串）
