# desktop 重复代码清理 plan：e2s + create_window + reveal/open

> **Status: ✅ Phase A/B 完成 + Phase C 7/10 窗口完成**（2026-07-29）。剩余 3 窗口（clipboard/screenshot/record_area_picker）ROI 不足留原样——clipboard build 后 136 行副作用照留（抽 builder 只省 8 行）、screenshot/record_area_picker 循环动态 label + 状态机耦合。验证：441 test 全过。
>
> **Spec**: [`2026-07-29-desktop-dry-cleanup.md`](../specs/2026-07-29-desktop-dry-cleanup.md)

## Phase A：reveal/open 去重（最干净，顺带修跨平台 bug）

### Task A.1: 新建 sys_open.rs + reveal_path 提取

**Files:** 新建 `crates/desktop/src/sys_open.rs`；`main.rs` 加 `mod sys_open;`；改 `search_commands.rs`

- [x] **Step 1: 新建 sys_open.rs**，`reveal_path(impl AsRef<Path>) -> Result<(), String>` + `reveal_path_lossy`（从 search_commands 提取，`pub(crate)`）
- [x] **Step 2: search_commands::reveal_path 改薄包装**（`#[tauri::command]` 签名不变，内部调 sys_open）
- [x] **Step 3: main.rs 加 mod sys_open;**

### Task A.2: open_with_default

- [x] **Step 1: sys_open.rs 加 `open_with_default(target: &str) -> Result<(), String>`**（三分支）
- [x] **Step 2: search_commands 的 open_url/open_file/launch_app 内部改调 open_with_default**

### Task A.3: 改 7 调用点

- [x] **Step 1: clipboard_commands::reveal_in_file_manager → sys_open::reveal_path**
- [x] **Step 2: record_commands::reveal_recording → sys_open::reveal_path**（修跨平台）
- [x] **Step 3: record_commands::reveal_subtitle → sys_open::reveal_path**（修跨平台）
- [x] **Step 4: record_commands::stop_and_store_inner → sys_open::reveal_path_lossy**
- [x] **Step 5: record_commands::open_recording_file → sys_open::open_with_default**（修跨平台）
- [x] **Step 6: action_bar_commands 内联 URL → sys_open::open_with_default**
- [x] **Step 7: clipboard_commands::open_file_item 抽 IO 部分调 open_with_default**

### Task A.4: 验证

- [x] `cargo build -p octopus-desktop --features embedded` + `cargo test -p octopus-desktop`

## Phase B：e2s 错误转换推广

### Task B.1: 新建 error_util.rs

**Files:** 新建 `crates/desktop/src/error_util.rs`；`main.rs` 加 `mod error_util;`

- [x] **Step 1: 定义 `e2s<E: Display + Debug>` + `e2s_ctx<E: Display>`**（pub(crate)，保留 log）
- [x] **Step 2: 补单测**
- [x] **Step 3: main.rs 加 mod error_util;**

### Task B.2-B.7: 逐文件推广（每批 cargo check）

- [x] **clipboard_commands.rs（52 处）**
- [x] **action_bar_commands.rs（30 处）**
- [x] **hotword_commands.rs（19 处）**
- [x] **settings_commands.rs（16 处）**
- [x] **screenshot_commands.rs（16 处）**
- [x] **model_commands.rs（15 处）+ 其余小文件**

### Task B.8: 验证

- [x] `cargo build` + `cargo test -p octopus-desktop`

## Phase C：create_window 透明浮动窗口抽象

### Task C.1: 新建 window_factory.rs

**Files:** 新建 `crates/desktop/src/window_factory.rs`；`main.rs` 加 `mod window_factory;`

- [x] **Step 1: 定义 FloatWindowSpec struct + build_float_window**（5 参数默认 + spec 参数，返回 WebviewWindow）
- [x] **Step 2: main.rs 加 mod window_factory;**

### Task C.2-C.5: 逐窗口改（每批验证）

- [x] **简单窗口：action_bar / overlay / password_generator / record_config**
- [x] **带 on_window_event：result / clipboard**
- [x] **destroy+rebuild：record_control / record_annotation**
- [x] **循环动态 label：screenshot / record_area_picker**

### Task C.6: 验证

- [x] `cargo build` + `cargo test` + 手动冒烟

## Phase D：全量验证 + 文档同步

- [x] `cargo clippy --workspace`（warning 不增）
- [x] `cargo test`（核心层 + desktop）
- [x] 更新 architecture.md（补 sys_open / error_util / window_factory 模块描述）
- [x] review plan
