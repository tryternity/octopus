# 粘贴前切换输入源（三段式文本注入）设计

> **状态**：已实现
> **日期**：2026-07-10
> **scope**：粘贴前临时切换到 ASCII 输入源（ABC）→ 模拟 Cmd+V → 恢复原输入源，解决 CJK 输入法下 Run And Paste 乱码
> **调研依据**：[`2026-07-09-action-bar-related-tools-survey.md`](./2026-07-09-action-bar-related-tools-survey.md) §1.2 VoxFlow VoxFlowTextInsertion

---

## 1. 背景与动机

### 1.1 问题

CJK 输入法（中文拼音/五笔/双拼、日文 IME、韩文输入法）在 **composing 状态**下，如果程序模拟 Cmd+V 粘贴文本，IME 会把粘贴的字符当作 composing 输入处理，导致：

- **乱码**：拼音输入法把英文字母当拼音编码，粘贴 "hello" 变成拼音候选词
- **字符丢失**：composing 缓冲区被覆盖，部分字符被丢弃
- **意外提交**：composing 中的候选词被意外确认提交

这在 octopus 的两条粘贴路径都会出现：
- **ASR 识别结果粘贴**（`paste.rs::paste_via_clipboard`）——语音识别完成后模拟 Cmd+V 把文本输入到当前光标
- **剪贴板浮窗双击粘贴**（`focus_tracker.rs::simulate_paste_platform`）——剪贴板历史条目双击粘贴

### 1.2 解决方案

参考 VoxFlow `VoxFlowTextInsertion` 模块的 **三段式文本注入**：

```
1. 粘贴前：临时切换到 ASCII 输入源（如 ABC）
2. 执行：模拟 Cmd+V（此时无 IME 干扰）
3. 完成后：恢复原输入源
```

用 RAII guard 保证步骤 3 总是执行（即使粘贴过程 panic 也能恢复）。

### 1.3 对标

| 产品 | 方案 | 平台 |
|------|------|------|
| **VoxFlow** | `VoxFlowTextInsertion` 模块：`Clipboard` / `FastPaste` / `SimulatedTyping` / `Coordinator`，粘贴前切换输入源 | macOS（Swift） |
| **octopus** | Carbon TIS API FFI + RAII guard，两条粘贴路径统一接入 | macOS（Rust） |

---

## 2. 技术方案

### 2.1 macOS 输入源 API

macOS 的输入源管理由 **Carbon HIToolbox** 提供（`Carbon.framework` 的一部分），核心 C API：

| API | 作用 | 内存规则 |
|-----|------|---------|
| `TISCopyCurrentKeyboardInputSource()` | 获取当前键盘输入源 | Copy（+1 retain，调用者需 CFRelease） |
| `TISGetInputSourceProperty(source, key)` | 读取输入源属性（如 InputSourceID） | Get（不 +1，不释放返回值） |
| `TISSelectInputSource(source)` | 切换到指定输入源 | 无 retain/release，返回 OSStatus（0=成功） |
| `TISCreateInputSourceList(props, includeAll)` | 查询输入源列表 | Copy（+1，返回 CFArray，需 CFRelease） |

**InputSourceID** 示例：
- `com.apple.keylayout.ABC` — 现代 macOS 默认 ASCII 布局
- `com.apple.keylayout.US` — 旧版/自定义键盘的 ASCII 布局
- `com.apple.inputmethod.SCIM.ITABC` — 苹果简体拼音
- `com.sogou.inputmethod.sogou.pinyin` — 搜狗拼音

### 2.2 FFI 绑定

`crates/desktop/src/input_source.rs` 用 `extern "C"` 直接声明 Carbon API，配合 `core-foundation` crate 的 `CFString` / `CFRelease` / `TCFType` 处理 Core Foundation 类型：

```rust
extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> CFTypeRef;
    fn TISGetInputSourceProperty(source: CFTypeRef, propertyKey: *const c_void) -> CFTypeRef;
    fn TISSelectInputSource(source: CFTypeRef) -> i32;
    fn TISCreateInputSourceList(properties: CFTypeRef, includeAllInstalled: u8) -> CFTypeRef;
    fn CFArrayGetCount(array: CFTypeRef) -> i64;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, idx: i64) -> *const c_void;
}
```

> ⚠️ **类型转换**：core-foundation 0.10 对类型严格，`CFString::as_concrete_TypeRef()` 返回 `*const __CFString`，传给需要 `*const c_void` 的 API 必须 `as *const c_void`；反过来 `wrap_under_get_rule` 需要 `as *const _` 让编译器推断。

### 2.3 RAII Guard

```rust
pub struct InputSourceGuard {
    previous: CFTypeRef,  // 构造时 TISCopyCurrentKeyboardInputSource 的 +1 ref
}

impl InputSourceGuard {
    pub fn switch_to_ascii() -> Option<Self> {
        // 1. 获取当前输入源
        // 2. 若已是 ABC/US → 返回 None（跳过，省 50ms 延迟）
        // 3. 从输入源列表找 ABC/US 并 Select
        // 4. 等 50ms（SWITCH_SETTLE_DELAY）让 Carbon 注册切换
        // 5. 返回 guard（持有 previous ref）
    }
}

impl Drop for InputSourceGuard {
    fn drop(&mut self) {
        // TISSelectInputSource(previous) 恢复原输入源
        // CFRelease(previous) 释放 +1 retain
    }
}
```

**设计要点**：

- **已是 ASCII 则跳过**：`is_ascii_id` 检测当前 InputSourceID，若已是 ABC/US 直接返回 `None`——不产生切换/恢复开销、不产生 50ms 延迟
- **RAII 保证恢复**：guard 在 paste 完成后 drop 自动恢复，即使 paste 过程出错（`?` early return / panic unwind）也能恢复
- **只找启用的输入源**：`TISCreateInputSourceList(null, 0)`——`includeAllInstalled=0` 只返回用户实际启用的输入源（用户没装 ABC 就返回 `None`，用当前 IME 粘贴——降级而非失败）

### 2.4 两条粘贴路径接入

#### 路径 1：ASR 识别结果粘贴（`paste.rs::paste_via_clipboard`）

```rust
fn paste_via_clipboard(text, handle, write_to_clipboard, switch_ime) -> Result<()> {
    // ... 备份剪贴板 + 写入文本 ...
    
    // 三段式：切输入源 → Cmd+V → guard drop 恢复
    let _ime_guard = if switch_ime {
        crate::input_source::switch_to_ascii_for_paste()
    } else {
        None
    };
    
    // ... enigo 模拟 Cmd+V ...
    
    // _ime_guard 在函数结束时 drop → 恢复原输入源
}
```

受 `config.switch_input_source_on_paste` 控制（默认 `true`）。

#### 路径 2：剪贴板浮窗双击粘贴（`focus_tracker.rs::simulate_paste_platform`）

```rust
fn simulate_paste_platform() {
    // 三段式：切输入源 → osascript Cmd+V → guard drop 恢复
    let _ime_guard = crate::input_source::switch_to_ascii_for_paste();
    
    // ... osascript keystroke "v" using command down ...
    
    // _ime_guard 在函数结束时 drop → 恢复原输入源
}
```

此路径直接开（不受 config 控制），因为剪贴板粘贴就是要把指定文本粘贴出去，CJK 干扰问题同在。

---

## 3. 线程安全分析

### 3.1 与 enigo UCKeyTranslate 的区别

architecture.md 已记录：`paste_via_clipboard` 的 V 键用固定虚拟键码 `Key::Other(9)` 而非 `Key::Unicode('v')`，因为 enigo 0.6.1 的 `Key::Unicode` 在 macOS 走 `get_layoutdependent_keycode`（循环调用非线程安全的 Carbon `TIS*`/`UCKeyTranslate` API），在 `spawn_blocking` 非主线程会 SIGTRAP。

**本次的 TIS API 与上述不同**：

| API | 线程安全性 | 原因 |
|-----|----------|------|
| `UCKeyTranslate` | ❌ 非线程安全 | 底层 UCKeyboardLayout 结构有共享状态，多线程并发读写竞态 |
| `TISCopyCurrentKeyboardInputSource` | ✅ 线程安全 | 纯 getter，只读当前 TIS 状态，不修改共享结构 |
| `TISSelectInputSource` | ✅ 线程安全（实践中） | 高层 API，内部通过分布式通知投递切换请求给主 RunLoop，不在调用线程修改 Carbon 全局状态 |

**关键区别**：`UCKeyTranslate` 在调用线程**同步**修改布局查找的内部缓冲区，而 `TISSelectInputSource` 只投递一个**异步通知**，实际切换由主 RunLoop 处理。50ms 延迟（`SWITCH_SETTLE_DELAY`）足够主 RunLoop 处理完通知。

### 3.2 spawn_blocking 上下文

`paste_via_clipboard` 在 `tauri::async_runtime::spawn` + `tokio::task::spawn_blocking` 中执行（粘贴异步化，见 architecture.md）。`InputSourceGuard::switch_to_ascii` 在此非主线程调用 TIS API 是安全的——如上分析。

---

## 4. 配置

### 4.1 新增配置字段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `switch_input_source_on_paste` | bool | `true` | 粘贴前是否临时切换到 ASCII 输入源。仅 macOS 生效（Windows/Linux 无此问题） |

### 4.2 添加步骤（serde 自动映射）

按 architecture.md「新增配置字段清单」只需 3 处：
1. `crates/infra/src/config.rs`：struct + serde default + fn + Default impl ✅
2. `crates/desktop/src/settings_commands.rs`：`apply_config_value` match（bool 类型无需特殊校验）✅
3. `crates/infra/src/db.sql`：seed INSERT ✅

load/save 自动跟随，round-trip 测试自动覆盖。

### 4.3 DB round-trip 测试

`app_config_roundtrip_all_fields` 测试已加哨兵值 `cfg.switch_input_source_on_paste = false`，确保 save→load 往返完整。

---

## 5. 不在本次范围

- **Windows / Linux 支持**：Windows 用 `ActivateKeyboardLayout` API，Linux 用 `ibus`/`fcitx` DBus 接口——当前 octopus 粘贴仅 macOS 模拟 Cmd+V（其他平台用 enigo `Shift+Insert` 或直接输入），CJK IME 干扰问题不显著
- **输入源选择 UI**：当前固定切到 ABC/US。未来可加配置项让用户指定切换目标（如有些用户装了第三方 ASCII 布局）
- **IME composing 状态检测**：当前不检测 IME 是否处于 composing 状态（无条件切换）。未来可检测 composing 状态只在需要时切换

---

## 6. 文件变更

| 文件 | 变更 |
|------|------|
| `crates/desktop/src/input_source.rs` | **新建**——Carbon TIS API FFI + RAII guard |
| `crates/desktop/src/main.rs` | 加 `mod input_source;` |
| `crates/desktop/src/paste.rs` | `paste_via_clipboard` 加 `switch_ime` 参数 + guard |
| `crates/desktop/src/focus_tracker.rs` | `simulate_paste_platform` 加 guard |
| `crates/desktop/src/settings_commands.rs` | `apply_config_value` 加 `switch_input_source_on_paste` bool 分支 |
| `crates/infra/src/config.rs` | 加 `switch_input_source_on_paste` 字段 + default |
| `crates/infra/src/db.rs` | round-trip 测试加哨兵值 |
| `crates/infra/src/db.sql` | seed INSERT 加新行 |

---

## 7. 信息来源

- [VoxFlow VoxFlowTextInsertion](https://github.com/xingbofeng/VoxFlow)（Swift 实现，参考其三段式理念）
- [Carbon HIToolbox TIS Reference](https://developer.apple.com/documentation/carbon/text-input-source-manager)（Apple 官方 API 文档）
- [TextInputSourcesReference](https://developer.apple.com/documentation/carbon/text-input-manager)（TIS API 详细说明）
