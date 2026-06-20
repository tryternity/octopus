# octopus-desktop 实现审查复核

> Date: 2026-06-20
> 状态：收到另一 AI 对 `crates/desktop`（Tauri 语音转写桌面端，~6.8k 行）的 7 条审查结论，逐条对照真实代码复核。**结论：7 条全部成立、行号引用全部准确，无幻象**（与 qwen3-asr 审查的 #3 不同）。其中一1 / 二2 / 三1 触发面与严重度需校准，二1 应升级。
> **P0 三条（一1/一2/二1）已实施并验证**（worktree `worktree-desktop-audit`）：`cargo check --workspace --all-targets` 零 warning、`cargo test -p octopus-asr` 50 passed/0 failed。P1/P2 待定。详见 §4/§5。
> 基线：worktree `worktree-desktop-audit` @ c259930（含全部 qwen3 修复）。
> 关联文件：`crates/desktop/src/{coordinator,dashscope_stream,paste,settings_commands,main,runtime_config,audio}.rs`、`crates/asr/src/config.rs`。
> 平行文档：`2026-06-20-qwen3-asr-inference-audit.md`（同日 asr 推理审查复核）。

## 1. 背景

另一 AI 对 octopus-desktop 提交了 7 条审查（3 稳定性 + 2 运行时配置 + 2 性能/UX）。复核方法与 qwen3-asr 审查一致：逐条读真实代码定性，区分真缺陷 / 幻象 / 已知取舍，校准严重度与触发面。desktop 无外部权威参考实现（不像 qwen3 对照 sherpa-onnx C++），故证据全部取自仓库源码 + 行号。

## 2. 复核结论总表

| # | AI 结论 | 判定 | 严重度 | 处置建议 |
|---|---|---|---|---|
| 一1 | 跨会话润色文本污染（PolishDone/FinalPolishDone 无 session_id） | ✅ 真实（**收窄**） | 中 | ✅ 已修（§4）：`PolishDone` 带 session_id + `handle_polish_done` 校验 |
| 一2 | 云端流式连接异常→录音挂起 | ✅ 真实 | 中高 | ✅ 已修（§4）：建连/关闭失败 emit `StreamEvent::Failed` + coordinator 报错复位 |
| 一3 | 剪贴板恢复竞态 | ✅ 真实 | 低 | P2：延长 paste 后等待或平台化处理 |
| 二1 | OnceLock 缓存致 denoise/hwaccel 失效 | ✅ 真实（**升级**） | 中高 | ✅ 已修（§4）：`OnceLock`→`RwLock<Option<Arc>>` + `reload_app_config`，desktop 写 DB 后刷新 |
| 二2 | mic/engine_mode 运行时切换失效 | ✅ 真实（mic）/ **部分过时**（engine_mode） | 中 | P1：mic/engine_mode 进 Toggle 同步；云/本地路由已被 DispatchEngine 解决 |
| 三1 | close `block_on` 卡主线程 | ✅ 真实（**设计取舍**） | 中 | P1：非阻塞 close，结果走 `Command::CloudStreamingDone` |
| 三2 | ASR 引擎切换缺预热 | ✅ 真实 | 中 | P1：set_config 切引擎时后台 switch_model 预热 |

**总判**：7/7 成立、行号全准，无幻象。校准 4 处（见 §3）。

## 3. 关键证据与校准（逐条）

### 3.1 一1 跨会话润色污染 —— 真实，但收窄到中间润色

**涉及位置**：`coordinator.rs` Command 定义（L44 `PolishDone` / L46 `FinalPolishDone`）、dispatch（L410-415）、`handle_polish_done`（L2092-2149）、`handle_final_polish_done`（L1091-1142）、中间润色 spawn（`spawn_polish_thread` L1614，发送 L1639）。

**证据**：
- `PolishDone { result }` / `FinalPolishDone { result }` 确实不带 session_id（L44/46）；而 `TranscriptionDone` 带 `session_id`（L39）——代码库本有此模式，polich 没沿用。
- 中间润色：`check_and_trigger_polish`（L1647，仅 `PolishMode::Intermediate`/mode=2）在 Streaming/Cloud 停顿时调 `spawn_polish_thread`（L1614）→ L1639 发 `PolishDone`。发生在 **Streaming 阶段（录音中）**。
- `handle_polish_done`（L2092）：匹配 `Streaming|VadSegmented|WaitingCompletion` 取**当前** transcript，**既不查 session_id 也不查 `polish_pending()`**（L2099-2109）。

**校准（复核核心）**：
- **FinalPolishDone 实际被保护**，不可利用：`handle_toggle` 对 `Stage::Polishing` 直接 `debug!("Toggle ignored: busy polishing")`（L971-973）→ 润色中无法开新录音；`handle_final_polish_done` 要求 stage==Polishing（L1098-1108），Cancel 后 stage 变 Idle → 旧 FinalPolishDone 落空被忽略。AI 把两条并列、过度泛化。
- **真正可利用的是中间 PolishDone**：Streaming 期用户 Esc（Cancel→Idle）+ 快速重开新录音 → 新会话 transcript（非 pending）→ 旧 PolishDone 被 `handle_polish_done` 应用到新 transcript + 写错 DB 行（`UpdatePolished`/`UpdateEdited` 用新 transcript.id）。
- 触发面窄：需 mode=2 + 停顿触发润色 + 润色窗口（1~3s）内 Cancel + 快速重开。但命中即数据污染。

**建议修法**（二选一）：
- 稳：`PolishDone` 加 `session_id: i64`，`handle_polish_done` 校验 `transcript.id == session_id` 不符则丢。
- 轻：`handle_polish_done` 入口加 `if !transcript.polish_pending() { return }`（新会话 transcript 非 pending 即拦住；`check_and_trigger_polish` 已 `mark_polish_pending` L1676）。代价是 pending 语义被复用为跨会话护栏，需注释说明。

### 3.2 一2 云端连接异常→录音挂起 —— 真实

**涉及位置**：`dashscope_stream.rs` `open`（L70-98）、`run_ws_session`（L159-322）、`run_qwen_realtime_session`（L413-575）；`coordinator.rs` `handle_cloud_streaming_tick`（L1362-1545）、停止路径（L928-941）。

**证据**：
- `open` 立即返回 `Ok(Self)`，建连在 spawned task（L82-95）。
- **建连/建立期失败不发 Failed**：`run_*_session` 在 `connect_async`（L179/L442）、发 run-task/session.update/pre-roll 失败时返回 `Err`，被 L92-94 捕获**仅 log**，不 send `StreamEvent::Failed`。
- **意外关闭也不发 Failed**：WS `next()→None`（L316/L569）→ `break` → `Ok(())`，无 Failed。
- 对照：只有**中途**错误才发 Failed（task-failed L299-306、WS 读错 L312-314、error 事件 L552-559）。
- coordinator：`try_recv_text()`（`result_rx.try_recv().ok()`，dashscope_stream.rs:120-122）通道关闭后恒 `None`，无法区分「没数据」与「已断」。`push_pcm` 失败仅 `warn!`（coordinator L1468），不重置 `is_speaking`/`session`/`is_closing`。`is_closing=true` 后 L1516 `if !is_closing && !is_speaking` 为假，session 不再 take → 僵尸会话。

**后果**：网络故障/Key 无效 → 应用卡在「正在聆听…」无输出无报错，直到 Esc。**非硬死锁**（Esc/Toggle 仍处理），但是静默失败 + 糟糕 UX。

**校准**：AI 描述准确。其建议的修法方向对但**不完整**——Failed 分支（L1507-1512）目前只清 `current_partial` + 复位 cloud 状态，**不会停录音也不会向用户报错**。完整修法需：①建连/关闭失败 emit Failed；②coordinator 收到 Failed（或检测 push_pcm 持续失败）后停录音 + UI 报错（如 `result_window::show_result("云端连接失败：…")`）。

### 3.3 一3 剪贴板恢复竞态 —— 真实（低）

**涉及位置**：`paste.rs` `paste_via_clipboard`（L71-129）。

**证据**：写剪贴板（L85）→ sleep 50ms（L89）→ Cmd+V（L109-117）→ **sleep 50ms（L119）→ 恢复原剪贴板**（L124-126，仅 `!write_to_clipboard && !saved.is_empty()`）。

**校准**：AI 说「等待 50ms 后恢复」——实际前后各一个 50ms sleep，竞态窗口是 Cmd+V 后那 50ms（L119）。慢应用/高负载下目标窗口处理 paste 可能 >50ms → 读到已恢复的旧剪贴板内容。仅 `!write_to_clipboard`（不保留结果）路径触发，低概率低影响。

**建议**：延长 L119 等待（150~200ms）或 macOS 用系统 API 延迟写；或引导用户开 `write_to_clipboard` 完全规避恢复逻辑。

### 3.4 二1 OnceLock 缓存致 denoise/hwaccel 失效 —— 真实，升级

**涉及位置**：`asr/src/config.rs` `APP_CONFIG`（L387）、`load_app_config_cached`（L389-397）、`apply_session_acceleration`（L401-402）；`desktop/src/audio.rs` L97/L209；`desktop/src/runtime_config.rs` `set_denoise_mode`（L295）/ `persist_denoise_mode`（L119）。

**证据**：
- `static APP_CONFIG: OnceLock<AppConfig>`（L387），`load_app_config_cached` 首次 `get_or_init` 后永不失效。注释 L384-386 **自述「手编 config.yaml 后需重启进程生效」**。
- `apply_session_acceleration`（L402）经此读 `asr_hardware_accelerated`（L404）；`audio.rs::process_pipeline` **每帧** L97 读 `denoise_mode`（L98）。两值冻在启动值。
- coordinator `sync_runtime_fields`（L531-538）不含 denoise_mode/asr_hardware_accelerated；注释 L529-530 主动排除 denoise_mode（称「音频路径有独立 cfg 读取」），但那个读取正是 OnceLock 缓存。

**校准（升级理由）**：不止「需重启」——`set_denoise_mode`（runtime_config.rs:295）写 SharedRuntimeConfig + DB，其错误信息（L305）自称「**本次仍生效，重启后回退**」，但 audio 每帧读 OnceLock 缓存、apply_session_acceleration 读缓存，**本次也不生效**。设置 UI 在承诺一个不存在的即时生效。这是功能性缺陷（含误导性 UX），非仅「已知限制」。

**建议修法**：
- 方案 A（小）：denoise_mode / asr_hardware_accelerated 改走 `SharedRuntimeConfig`（audio + apply_session_acceleration 读运行期共享态，不读 OnceLock）。
- 方案 B：给 `APP_CONFIG` 加失效接口（设置变更后 reset），下次 `load_app_config_cached` 重读。代价是缓存语义变化（原本为省 yaml 解析），需评估 session 构建频率。
- 任一方案都应同步把 `set_denoise_mode` 的「本次生效」承诺坐实，或撤回该文案。

### 3.5 二2 mic/engine_mode 切换失效 —— mic 真实，engine_mode 部分过时

**涉及位置**：`coordinator.rs` `sync_runtime_fields`（L531-538）、Toggle Idle 同步块（L256-275）、`handle_toggle` `audio.start`（L598）；`main.rs` engine 构造（L241-250）；`settings_commands.rs` `set_config`（L62-106）、`apply_config_value`（mic L180 / engine_mode L122-128）。

**证据**：
- `sync_runtime_fields` 只同步 polish_mode/polish_llm/asr_correct/output_simplified/hide_toolbar/edit_shortcut（L531-538）；Toggle Idle 块（L259-263）只额外刷 `asr_engine`。**`microphone`、`engine_mode` 从构造起 stale**。
- mic 实锤：`audio.start(&config.microphone)`（L598）用 stale 快照 → 改设置里的麦克风，下次录音仍用启动设备，需重启。
- engine_mode：`use_streaming = config.engine_mode == "embedded" && ...`（L224/L265）用 stale engine_mode；`set_config` 对 engine_mode 不触发 `update_runtime`（L82-87）。

**校准（部分过时）**：AI「底层引擎类不重新创建为 WsRemoteEngine/GrpcRemoteEngine」**已部分被解决**——dashscope feature 下 engine 是 `DispatchEngine`（main.rs:244），持有 engine_manager，**每次 transcribe 按 asr_engine spec 动态路由云/本地**，注释 L238-240 自述「解决运行时切换云/本地引擎不匹配」。而 `asr_engine` 在 Toggle Idle 时已同步（L259）。故引擎路由层面运行时可切；真正 stale 的只剩 `engine_mode` 对 `use_streaming`/预热的 gate。

**建议**：mic 进 `sync_runtime_fields`（或 Toggle Idle 块刷新 `config.microphone`）让下次录音生效；engine_mode 同理刷新以正 `use_streaming`。

### 3.6 三1 close block_on 卡主线程 —— 真实，设计取舍

**涉及位置**：`dashscope_stream.rs` `close`（L133-156）；`coordinator.rs` 停止路径 `sess.close(&rt)`（L928-941，调用点 L933）。

**证据**：`close` 内 `rt.block_on(timeout(8s, ...))`（L136/L141），Toggle 停止 CloudStreaming 时在 coordinator 同步线程调用（L933）。8s 与段级超时一致（注释 L140）。

**校准（设计取舍）**：代码**自知**此阻塞——`close` 文档注释 L124-127 明确「**不要在 tick handler 中调用**——`block_on` 会阻塞 coordinator 线程；tick 应使用 `finish()` 非阻塞」。即作者已把 tick 路径走非阻塞 `finish()`，仅停止路径用阻塞 `close()`（为拿最终文本）。AI 夸大了「8 秒」频率：正常路径最终文本秒回，8s 是 WS 挂起的最坏上限。但停止路径阻塞 coordinator 期间，新 Command（重开/取消）确会堆积无响应， responsiveness 问题真实。

**建议**：停止路径也改非阻塞——`close` 的收尾在 async task 完成，结果以 `Command::CloudStreamingDone { text }` 发回 coordinator；需新增对应 Stage（如 `Stage::CloudClosing`）承载等待态。属较大改动，权衡收益。

### 3.7 三2 引擎切换缺预热 —— 真实

**涉及位置**：`main.rs` setup 预热（L207-236）；`settings_commands.rs` `set_config`（L62-106，asr_engine L183）。

**证据**：预热仅 setup 一次——`do_preheat = engine_mode=="embedded"`（L207），spawn 线程 `em.switch_model(&active_model)`（L219-220）+ VAD 预载（L227-234）。`set_config` 改 asr_engine/engine_mode 时**只写 RuntimeConfig + DB（L73-78），不触发预热**（L82-87 仅 5 个 polish 字段调 update_runtime）。

**后果**：运行时切引擎（如 zipform→whisper）→ 首次 transcribe 在 `spawn_blocking` 懒加载模型（反序列化 + ONNX session 创建，视模型 1~数秒）→ 首录卡顿。

**建议**：`set_config` 检测 asr_engine 变更时，后台 spawn `switch_model` 预热（复用 setup 的 L219 模式）。注意仅 embedded 本地引擎需要（云引擎无需）。

## 4. 已实施修复（P0：一1/一2/二1）

用户选定 P0 三条，已实施并验证（§5）。

### 一1 PolishDone 跨会话护栏
- `Command::PolishDone` 加 `session_id: i64`（与既有 `TranscriptionDone` 模式一致；未改 `FinalPolishDone`——它已被 stage guard 保护，见 §3.1）。
- `spawn_polish_thread` 加 `session_id` 参数；两处调用（`check_and_trigger_polish` / `handle_polish_now`）传 `transcript.id`。
- `handle_polish_done` 入口校验 `transcript.id != session_id` 即丢 + emit `polish-done` 恢复前端按钮。选 session_id 方案（非 polish_pending 护栏）：复用代码库既有模式、不污染 pending 语义。
- 涉及：`coordinator.rs`（Command 定义、dispatch、`spawn_polish_thread`、`handle_polish_done`、两处调用）。

### 一2 云端失败上报 + coordinator 守卫
- `dashscope_stream.rs::open`：自留 `result_tx.clone()`，建连/建立期失败（`run_*_session` 返 `Err`：connect_async / 发 run-task / pre-roll 失败）时由此发 `StreamEvent::Failed`（原本仅 log、`result_tx` 已被 `run_*` 移入并 drop）。
- WS 意外关闭（`ws.next()→None`）发 `Failed("WS 连接意外关闭")`；与 `pcm_rx.recv()→None`（coordinator drop、优雅关闭）区分，后者保持静默 break。
- coordinator `handle_cloud_streaming_tick` 的 `Failed` 分支：清 partial + 复位 cloud 状态 + `update_result("⚠️ 云端识别失败：<msg>")`。session 由既有 `!is_closing && !is_speaking` 分支自动 take（避免与 `sess` 借用冲突），下次 onset 重开 WS（瞬时抖动自动重试；持续失败如 Key 无效每次 onset 报错）。
- 涉及：`dashscope_stream.rs`（`open`、两处 WS None）、`coordinator.rs`（Failed 分支）。

### 二1 denoise/硬件加速运行时即时生效
- 根因复盘：`load_config()` 委托 `db::load_app_config()`——**真相源是 DB**（yaml 仅一次性迁移源）。`load_app_config_cached` 用不可重置的 `OnceLock` 冻住了 DB 读取结果。
- 修法：`APP_CONFIG` 从 `OnceLock<AppConfig>` 改为 `RwLock<Option<Arc<AppConfig>>>`；`load_app_config_cached() -> Arc<AppConfig>`（调用方均即时字段访问，Arc deref 兼容，**零调用点改动**——已 grep 全 workspace 4 处：audio×2、asr/config、hans、engine）；新增 `pub fn reload_app_config()` 从 DB 重读刷新。
- desktop 写 DB 后调 `reload_app_config()`：`set_config`（save_app_config 后，覆盖所有字段含 denoise/hwaccel）+ `set_denoise_mode`（toolbar 路径，persist 后）。撤回 `set_denoise_mode` 原「本次仍生效，重启后回退」的虚假承诺文案。
- 涉及：`asr/src/config.rs`、`desktop/src/settings_commands.rs`、`desktop/src/runtime_config.rs`。

### 待定（未实施）
- **P1**：二2（mic/engine_mode 进 sync）、三2（切引擎后台预热）、三1（非阻塞 close + `Command::CloudStreamingDone`）。
- **P2**：一3（延长 paste 后等待）。

## 5. 验证

- `cargo check --workspace --all-targets`：**零 warning**（含 test targets；pub fn 签名变更无遗漏调用点）。
- `cargo test -p octopus-asr`：**50 passed / 0 failed**（config.rs 改动未破坏既有）。
- **未加单测的项**（与 qwen3 `decoder_kv_max_len` 同类，依赖外部环境无法离线测）：
  - 一1 session_id 护栏：需构造 `Stage` + `Transcript` + mock `tauri::AppHandle`，coordinator 全 Tauri 耦合、无既有 test 模块。
  - 一2 Failed 上报：需 tokio runtime + 真实 WS 连接失败场景。
  - 二1 reload：需 DB 初始化（`load_config`→`db::load_app_config`），单测环境无 DB。
  - 逻辑均简单（id 比较 / channel send / RwLock swap），由 check + 逻辑审查保证；行为正确性留 GUI e2e（环境无 GUI）。

## 6. 待决定

P0 三条已实施、待提交。下一步选项：①提交到 worktree 分支；②继续 P1（二2/三2/三1）；③就此打住。修复前不预设 P1/P2 范围。

## 7. 附：已澄清的过时认知

- summary 曾记「`cargo check --workspace` 在 `crates/desktop` 报 `octopus_llm::test_connection` 未找到」——**已不存在**：`crates/llm/src/lib.rs:6` 已 `pub use client::{polish, test_connection}`，定义 `client.rs:135`，desktop 调用点 `settings_commands.rs:277` 合法。该旧状态过时。
