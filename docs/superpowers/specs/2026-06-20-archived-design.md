# 归档设计文档（2026-06-20）

> **归档说明**（2026-06-21）：以下 4 个 spec 对应功能/审查均已实现并合并 main（ort 跨平台 EP feature 的 linux/win 交叉验证受目标工具链阻塞，详见正文），各自文档原样合并归档于此，原独立文件已删除。每个章节以 `📄 <原文件名>` 标注来源。
> **交叉引用**：正文内 `[xxx.md](./xxx.md)` 链接为合并前原文件名，现指向本归档文件内同名章节；对应 plans 见 `docs/superpowers/plans/2026-06-20-archived-plans.md`。

---

## 📄 `2026-06-20-desktop-implementation-audit.md`

# octopus-desktop 实现审查复核

> Date: 2026-06-20
> 状态：收到另一 AI 对 `crates/desktop`（Tauri 语音转写桌面端，~6.8k 行）的 7 条审查结论，逐条对照真实代码复核。**结论：7 条全部成立、行号引用全部准确，无幻象**（与 qwen3-asr 审查的 #3 不同）。其中一1 / 二2 / 三1 触发面与严重度需校准，二1 应升级。
> **P0 三条（一1/一2/二1）+ P1 三条（二2/三2/三1）均实施并验证**（原 worktree `worktree-desktop-audit`）：`cargo check -p octopus-desktop --features "embedded dashscope"` 零 warning、`cargo test -p octopus-asr-local` 52 passed/0 failed、逐 commit bisect-clean。**P0+P1 均已合并 main**（P0 `44b8ab8`、P1 `9a19b6b`）。P2（一3）延后，GUI e2e 待本地验证，详见 followups plan `2026-06-20-desktop-audit-followups.md`。详见 §4/§5。
> 基线：worktree `worktree-desktop-audit` @ c259930（含全部 qwen3 修复）。
> 关联文件：`crates/desktop/src/{coordinator,dashscope_stream,paste,settings_commands,main,runtime_config,audio}.rs`、`crates/asr/src/config.rs`。
> 平行文档：`2026-06-20-qwen3-asr-inference-audit.md`（同日 asr 推理审查复核）。

## 1. 背景

另一 AI 对 octopus-desktop 提交了 7 条审查（3 稳定性 + 2 运行时配置 + 2 性能/UX）。复核方法与 qwen3-asr 审查一致：逐条读真实代码定性，区分真缺陷 / 幻象 / 已知取舍，校准严重度与触发面。desktop 无外部权威参考实现（不像 qwen3 对照 sherpa-onnx C++），故证据全部取自仓库源码 + 行号。

## 2. 复核结论总表

| # | AI 结论 | 判定 | 严重度 | 处置建议 |
|---|---|---|---|---|
| 一1 | 跨会话润色文本污染（PolishDone/FinalPolishDone 无 session_id） | ✅ 真实（**收窄**） | 中 | ✅ 已修（§4）：`PolishDone`/`FinalPolishDone` 均带 session_id + handler 校验（FinalPolishDone 为后续复核追加，见 followups §4） |
| 一2 | 云端流式连接异常→录音挂起 | ✅ 真实 | 中高 | ✅ 已修（§4）：建连/关闭失败 emit `StreamEvent::Failed` + coordinator 报错复位 |
| 一3 | 剪贴板恢复竞态 | ✅ 真实 | 低 | P2：延长 paste 后等待或平台化处理 |
| 二1 | OnceLock 缓存致 denoise/hwaccel 失效 | ✅ 真实（**升级**） | 中高 | ✅ 已修（§4）：`OnceLock`→`RwLock<Option<Arc>>` + `reload_app_config`，desktop 写 DB 后刷新 |
| 二2 | mic/engine_mode 运行时切换失效 | ✅ 真实（mic）/ **部分过时**（engine_mode） | 中 | ✅ 已修（§4）：Idle 开新会话时 mic/engine_mode 进 Toggle 同步；云/本地路由已被 DispatchEngine 解决 |
| 三1 | close `block_on` 卡主线程 | ✅ 真实（**设计取舍**） | 中 | ✅ 已修（§4）：非阻塞 close_async，结果走 `Command::CloudStreamingDone` + `Stage::CloudClosing` |
| 三2 | ASR 引擎切换缺预热 | ✅ 真实 | 中 | ✅ 已修（§4）：set_config / switch_asr_engine 切引擎时后台 switch_model 预热 |

**总判**：7/7 成立、行号全准，无幻象。校准 4 处（见 §3）。

## 3. 关键证据与校准（逐条）

> **行号 / 代码为审查时快照**（基线 `c259930`）。本节描述的是被审查的**缺陷态**，非当前代码——§3.4 的 `OnceLock`、§3.6 的同步 `close()`/`block_on` 等均已在 §4 修复（分别 → `RwLock` / `close_async` + `Stage::CloudClosing`）。下方行号随 main 演进已漂移，定位以函数/符号为准，现况以 `crates/desktop/src/*` 与 `docs/architecture.md` 为准。

### 3.1 一1 跨会话润色污染 —— 真实，但收窄到中间润色

**涉及位置**：`coordinator.rs` Command 定义（L44 `PolishDone` / L46 `FinalPolishDone`）、dispatch（L410-415）、`handle_polish_done`（L2092-2149）、`handle_final_polish_done`（L1091-1142）、中间润色 spawn（`spawn_polish_thread` L1614，发送 L1639）。

**证据**：
- `PolishDone { result }` / `FinalPolishDone { result }` 确实不带 session_id（L44/46）；而 `TranscriptionDone` 带 `session_id`（L39）——代码库本有此模式，polich 没沿用。
- 中间润色：`check_and_trigger_polish`（L1647，仅 `PolishMode::Intermediate`/mode=2）在 Streaming/Cloud 停顿时调 `spawn_polish_thread`（L1614）→ L1639 发 `PolishDone`。发生在 **Streaming 阶段（录音中）**。
- `handle_polish_done`（L2092）：匹配 `Streaming|VadSegmented|WaitingCompletion` 取**当前** transcript，**既不查 session_id 也不查 `polish_pending()`**（L2099-2109）。

**校准（复核核心）**：
- **FinalPolishDone 表面被保护，实有重开漏洞**（后续复核修正）：`handle_toggle` 对 `Stage::Polishing` 直接 `debug!("Toggle ignored: busy polishing")`→ 润色中无法开新录音；`handle_final_polish_done` 要求 stage==Polishing。原推理据此认为「Cancel 后 stage 变 Idle → 旧 FinalPolishDone 落空被忽略」——但这只覆盖「Cancel 后保持 Idle」。若用户 Cancel（→Idle）后**立刻重开新录音并再次停止触发润色 → 新 `Stage::Polishing`**，旧会话迟到的 FinalPolishDone 会匹配到新 Polishing，用新 id + 旧润色文本 `do_paste` → 跨会话污染。触发窗口窄（润色 1~3s 内 Cancel+重开+再停），但与中间 PolishDone 同类、后果相同。**已补 `FinalPolishDone.session_id` 护栏（见 followups §4）**。
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

## 4. 已实施修复

P0（一1/一2/二1）+ P1（二2/三2/三1）**均已合并 main**（P0 `44b8ab8`、P1 `9a19b6b`）。逐条 commit、bisect-clean（§5）。

### P0：一1 / 一2 / 二1（已合并 main）

#### 一1 PolishDone 跨会话护栏
- `Command::PolishDone` 加 `session_id: i64`（与既有 `TranscriptionDone` 模式一致）。`FinalPolishDone` 当时认为已被 stage guard 保护未改（见 §3.1 原推理），**后续复核发现 Cancel+重开+再润色可绕过该保护，已补 `session_id` 护栏（followups §4）**。
- `spawn_polish_thread` 加 `session_id` 参数；两处调用（`check_and_trigger_polish` / `handle_polish_now`）传 `transcript.id`。
- `handle_polish_done` 入口校验 `transcript.id != session_id` 即丢 + emit `polish-done` 恢复前端按钮。选 session_id 方案（非 polish_pending 护栏）：复用代码库既有模式、不污染 pending 语义。
- 涉及：`coordinator.rs`（Command 定义、dispatch、`spawn_polish_thread`、`handle_polish_done`、两处调用）。

#### 一2 云端失败上报 + coordinator 守卫
- `dashscope_stream.rs::open`：自留 `result_tx.clone()`，建连/建立期失败（`run_*_session` 返 `Err`：connect_async / 发 run-task / pre-roll 失败）时由此发 `StreamEvent::Failed`（原本仅 log、`result_tx` 已被 `run_*` 移入并 drop）。
- WS 意外关闭（`ws.next()→None`）发 `Failed("WS 连接意外关闭")`；与 `pcm_rx.recv()→None`（coordinator drop、优雅关闭）区分，后者保持静默 break。
- coordinator `handle_cloud_streaming_tick` 的 `Failed` 分支：清 partial + 复位 cloud 状态 + `update_result("⚠️ 云端识别失败：<msg>")`。session 由既有 `!is_closing && !is_speaking` 分支自动 take（避免与 `sess` 借用冲突），下次 onset 重开 WS（瞬时抖动自动重试；持续失败如 Key 无效每次 onset 报错）。
- 涉及：`dashscope_stream.rs`（`open`、两处 WS None）、`coordinator.rs`（Failed 分支）。

#### 二1 denoise/硬件加速运行时即时生效
- 根因复盘：`load_config()` 委托 `db::load_app_config()`——**真相源是 DB**（yaml 仅一次性迁移源）。`load_app_config_cached` 用不可重置的 `OnceLock` 冻住了 DB 读取结果。
- 修法：`APP_CONFIG` 从 `OnceLock<AppConfig>` 改为 `RwLock<Option<Arc<AppConfig>>>`；`load_app_config_cached() -> Arc<AppConfig>`（调用方均即时字段访问，Arc deref 兼容，**零调用点改动**——已 grep 全 workspace 4 处：audio×2、asr/config、hans、engine）；新增 `pub fn reload_app_config()` 从 DB 重读刷新。
- desktop 写 DB 后调 `reload_app_config()`：`set_config`（save_app_config 后，覆盖所有字段含 denoise/hwaccel）+ `set_denoise_mode`（toolbar 路径，persist 后）。撤回 `set_denoise_mode` 原「本次仍生效，重启后回退」的虚假承诺文案。
- 涉及：`asr/src/config.rs`、`desktop/src/settings_commands.rs`、`desktop/src/runtime_config.rs`。

### P1：二2 / 三2 / 三1（已合并 main `9a19b6b`）

P1 三条已实施、逐 commit bisect-clean、**已合并 main**（`9a19b6b`，原分支 `worktree-desktop-audit`）。提交粒度：每条 finding 一条 commit（dfec6fe 三2 / e1bb944 二2 / b0b7468 三1）。

#### 二2 mic/engine_mode 运行时同步
- `handle_toggle` 的 Idle（开新会话）块，在既有 `asr_engine` 刷新之后、`sync_runtime_fields` 之前，补 `config.microphone = rc.microphone.clone()` + `config.engine_mode = rc.engine_mode.clone()`。
- 与 `asr_engine` 同策略：下次录音生效（mic/引擎不支持会话中热切）。修前 audio.start 用 stale 设备名（改设置后下次录音仍用旧设备，需重启）。
- 涉及：`coordinator.rs`（handle_toggle Idle 块）。

#### 三2 引擎切换后台预热
- `main.rs`：`engine_manager` 暴露为 State（切引擎预热需持有它；DispatchEngine 已持 clone，此处再 clone 托管）。
- `runtime_config.rs`：新增 `preheat_local_engine(engine_manager, spec, engine_mode)`——仅 `engine_mode=="embedded"` 且非 cloud（aliyun）时 spawn `switch_model`；`switch_asr_engine` 切换后调用。
- `settings_commands.rs`：`set_config` 加 `engine_manager` State 参数；`key=="asr_engine"` 时调 `preheat_local_engine`。
- 涉及：`main.rs`、`runtime_config.rs`、`settings_commands.rs`。

#### 三1 云端 close 非阻塞化
- `dashscope_stream.rs`：`close()`（block_on 封装）重构为 `pub async fn close_async(self)`（发 Finish + 8s 超时 recv loop）；旧同步 `close()` 删除（无调用方，消除 block_on footgun）。
- `coordinator.rs`：
  - `Stage::CloudClosing { transcript, current_partial }`：close 在飞期间持有收尾态。
  - `Command::CloudStreamingDone { text: Result<String, String>, session_id: i64 }`：close_async 结果回传。`session_id` 为后续复核追加——CloudClosing 期间 Cancel/Discard 会清回 Idle（绕过 Toggle 忙保护），用户可立刻重开云端会话 → 新 CloudClosing，旧 close 结果会匹配新 CloudClosing 覆盖其 transcript；`handle_cloud_streaming_done` 校验 `transcript.id == session_id` 不符则丢（followups §4）。
  - stop 路径（handle_toggle CloudStreaming arm）：session 在 → spawn `close_async` + 进 CloudClosing + `return`；无 session → 直接 `finalize_cloud`。
  - `finalize_cloud(stage, transcript, current_partial, ...)`：append partial + 空→Idle / 否则 `start_final_polish_or_paste`（stop 无 session 路径与 Done 路径共用，避免重复）。
  - `handle_cloud_streaming_done`：仅 CloudClosing 处理，先校验 `transcript.id == session_id`（跨会话护栏）再 `set_full` + finalize；非 CloudClosing 或 session_id 不符则忽略。
  - `handle_toggle`/`handle_discard`/`stage_name` 补 CloudClosing arm；`handle_cancel` 走既有 `_ =>` 兜底。
- 语义（CloudClosing 期间）：Toggle 忽略（close 完成自动 finalize+粘贴）；Cancel→Idle 不粘贴不写库；Discard→写库保历史不粘贴。三条均正确。
- 涉及：`dashscope_stream.rs`、`coordinator.rs`。

### 延后（P2，未实施）
- 一3（延长 paste 后等待剪贴板恢复）。

## 5. 验证

- `cargo check -p octopus-desktop --features "embedded dashscope"`：**零 warning 零 error**（三1 的 cfg-gated 代码需带 dashscope feature 才编入；默认 `embedded` 同样干净）。
- `cargo test -p octopus-asr-local`：**52 passed / 0 failed**（P1 不动 asr 逻辑，无回归）。
- **逐 commit bisect-clean**：dfec6fe / e1bb944 / b0b7468 各自 checkout 独立编译通过。
- **未加单测的项**（依赖外部环境无法离线测）：
  - 一1 session_id 护栏：需构造 `Stage` + `Transcript` + mock `tauri::AppHandle`，coordinator 全 Tauri 耦合、无既有 test 模块。
  - 一2 Failed 上报 / 三1 非阻塞 close：需 tokio runtime + 真实 WS 连接场景。
  - 二1 reload / 二2 mic 同步 / 三2 预热：需 DB 初始化 + GUI 交互。
  - 逻辑均简单（id 比较 / channel send / RwLock swap / spawn），由 check + 逻辑审查保证；行为正确性留 GUI e2e（环境无 GUI）。

## 6. 现状与后续

P0（一1/一2/二1，`44b8ab8`）+ P1（二2/三2/三1，`9a19b6b`）**均已合并 main**。剩余：P2（一3 剪贴板恢复等待）延后 + GUI e2e 待本地验证，详见 followups plan `2026-06-20-desktop-audit-followups.md`。

## 7. 附：已澄清的过时认知

- summary 曾记「`cargo check --workspace` 在 `crates/desktop` 报 `octopus_llm::test_connection` 未找到」——**已不存在**：`crates/llm/src/lib.rs:6` 已 `pub use client::{polish, test_connection}`，定义 `client.rs:135`，desktop 调用点 `settings_commands.rs:277` 合法。该旧状态过时。

## 📄 `2026-06-20-ort-cross-platform-feature-design.md`

# ort 跨平台 EP feature 条件化设计

> Date: 2026-06-20
> 状态：已实现（2026-06-20，commits 66a8a73 + 21fb2fb）。mac 单测+release 通过；GUI e2e 已通过；linux/win 交叉 check 受阻于目标工具链（见 §7.2）
> Worktree：`feature/ort-cross-platform`
> 关联：体积裁剪报告（2026-06-20，release profile 已落地 ac576de）、[[asr 硬件加速 segfault 修复]]

## 1. 背景

octopus 的 ort（ONNX Runtime）依赖在 `crates/asr/Cargo.toml` 无差别全开四个 feature：

```toml
ort = { version = "2.0.0-rc.12", features = ["download-binaries", "cuda", "coreml", "directml"] }
```

带来两个问题：

1. **体积冗余（§7.1 实测推翻此假设）**：原以为 mac 二进制会编入 cuda/directml 的 Rust EP 代码 + 触发多余预编译下载。**实测不成立**——config.rs 的 `#[cfg]` 早把 cuda/directml 的 Rust 引用排除（从未编进 mac 二进制），GPU 预编译库无 mac 版（从未下载）。mac 二进制维持 54M。本改动真正价值是 ② segfault defense-in-depth，非体积。
2. **segfault 根源未除**：全开 feature 曾导致 macOS 上跨平台误注册 CUDA/DirectML EP，其 init 失败路径（dlopen libcuda 等）直接 SIGSEGV 绕过 Rust 错误处理。**代码层已修复**（`config.rs:424-432` 按 `#[cfg(target_os)]` 只注册本平台 EP），但 **feature 仍全开**——根源未除，未来代码回归时仍可能复发。

目标 app 需同时支持 mac/win/linux 三平台打包。

## 2. 目标

- 三平台各自**只启用对应硬件加速 EP**：mac→coreml、linux→cuda、win→directml。
- ort feature 与代码层 `#[cfg]` **1:1 对齐**（设计意图；**非编译器硬约束**——ort 的 EP 类型无条件编译、只有 `register()` 内 FFI 块按 feature gate，故不一致不会编译失败，见 §5.4/§7.3）。
- 处理 Cargo feature unification 坑（确认在此结构下不构成问题）。
- 不引入构建脚本/CI 按平台传参的脆弱依赖。

## 3. 探索结论（关键）

1. **代码层（`config.rs:401-449` `apply_session_acceleration`）已按 `#[cfg(target_os)]` 分平台注册 EP**（设计时现状；本 spec Task 2 后 win 收敛为仅 DirectML）：mac=CoreML、linux=CUDA、win=DirectML+CUDA（改动前）。含 `asr_hardware_accelerated` 开关、qwen3-asr 跳 CoreML（动态算子不兼容）、EP 注册失败 fallback CPU。代码层早就是对的。
2. **ort 全 workspace 仅 `asr/Cargo.toml` 一处声明**（`dlp/main.rs:38` 的 `#[cfg]` 非 ort EP）→ **不存在 workspace 级 feature unification 问题**。
3. **target-specific dependency 的 feature 不跨 target 合并**：mac 编译时 linux 块的 `cuda` 不激活。所谓"并集"坑只在"同一 target 内多处声明同一包"时发生，octopus 仅 base 一处 + per-target 一处，合并结果正是期望（`download-binaries` + 该平台 EP）。
4. 结论：方案比预想简单安全——标准 target-specific dependency 即可，无需 build.rs 或自定义 feature 开关。

## 4. 方案选择

| 方案 | 形态 | 评价 |
|---|---|---|
| **A. target-specific dependency（采用）** | base `[dependencies]` 放 `download-binaries`，三个 `[target.'cfg']` 各放 EP feature | Cargo 惯例、最简洁、与代码 `#[cfg]` 1:1、零构建脚本依赖 |
| B. 自定义 `[features]` + `--features` 按平台传 | 定义 cuda/directml feature，构建按 target 传参 | 需 CI/脚本传参，易忘易错，不如 A 自动 |
| C. 拆 per-platform asr 子 crate | asr-mac / asr-linux / asr-win | 过度工程，YAGNI，否决 |

**采用 A。**

## 5. 设计

### 5.1 Feature 矩阵

| 平台 | base feature | EP feature | 代码层注册的 EP |
|---|---|---|---|
| macOS | download-binaries | coreml | CoreML |
| Linux | download-binaries | cuda | CUDA |
| Windows | download-binaries | directml | DirectML（**删 CUDA**） |

CPU EP 是 ort 内置，无需 feature，所有平台自动可用（EP 注册失败时 fallback CPU）。

### 5.2 `crates/asr/Cargo.toml` 形态

```toml
[dependencies]
ort = { version = "2.0.0-rc.12", features = ["download-binaries"] }

[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["coreml"] }

[target.'cfg(target_os = "linux")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["cuda"] }

[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["directml"] }
```

> **`default-features` 实测结论（2026-06-20）：去掉 false、保留默认集开启。** 关掉会缺 `tls-native`，`download-binaries` 编译即报缺 TLS feature。ort 默认集 = std/ndarray/tracing/download-binaries/tls-native/copy-dylibs/api-24，**本不含 cuda/directml/coreml**，故保留不影响目标。另：三个 `[target.'cfg']` 块须放 `[dependencies]` 表**末尾**（非 ort 行正下方）——TOML 表头切换活跃表，放中间会让后续依赖泄漏进 windows target。

### 5.3 代码层改动（`crates/asr/src/config.rs:428-432`）

win 块删 CUDA 注册。**实测更正**：`CUDAExecutionProvider` 类型**无条件存在**（并非「feature 没开就类型不存在」）——ort EP 类型始终编译，仅 `register()` 内 FFI 块按 feature gate（cuda off 时返回 `MissingFeature`、不碰 FFI）。故删除**非编译必需**，理由实为「避免注册注定失败的死 EP」：

```rust
// 改前
#[cfg(target_os = "windows")]
{
    providers.push(ort::ep::DirectMLExecutionProvider::default().build());
    providers.push(ort::ep::CUDAExecutionProvider::default().build());
}

// 改后
#[cfg(target_os = "windows")]
providers.push(ort::ep::DirectMLExecutionProvider::default().build());
```

同步更新上方注释（`Windows=DirectML+CUDA` → `Windows=DirectML`）。

### 5.4 一致性

- ort 仅 asr 一处声明，三平台各自 base+ep 合并，**无 workspace unification 风险**。
- 代码 `#[cfg]` 与 feature 矩阵 **1:1 对齐**（设计意图，**非编译器硬约束**）。ort EP 类型无条件编译、仅 `register()` FFI 块按 feature gate——故「win 有 directml 却引用 `CUDAExecutionProvider`」之类不一致**不会编译失败**，只在运行时 register 返回 `MissingFeature`→CPU fallback。真正的 feature-level 保护见 §7.3。

### 5.5 验证策略

- **mac 本地（已验证 2026-06-20）**：`cargo test -p octopus-asr-local` 45 passed/0 failed；`cargo build --release -p octopus-desktop` 通过；desktop e2e（CoreML 录音）**已通过**（CoreML 加速正常、无 segfault 回归）。
- **linux/win 交叉（受阻）**：`cargo check --target x86_64-unknown-linux-gnu` 卡在 `openssl-sys`（mac→linux 缺 openssl dev sysroot）、`--target x86_64-pc-windows-msvc` 卡在 `esaxx-rs` C++（缺 MSVC 工具链）——均为目标平台 C/C++ 工具链缺失，**非 ort、非本改动**。feature 矩阵正确性改由源码 gate 结构（§7.3）+ mac coreml 实证代理核验；运行正确性留用户在对应平台本地自测。
- CI 是否三平台 check：当前未设。

## 6. 关键决策

1. **win 仅 DirectML（删 CUDA）**：DirectML 覆盖所有 DX12 GPU（NVIDIA/AMD/Intel 核显通吃），实时语音转写够用；省 cuda 预编译库体积；三平台单 EP 矩阵一致。代价：NVIDIA 卡无法走 CUDA（推理更快），但 YAGNI，DirectML 对实时转录足够。需同步删代码层 win 的 CUDA 注册。
2. **`default-features`（实测后：不开 false，保留默认集）**：原想避免 ort 默认拉多余 feature。实测关掉会缺 `tls-native` 致 download-binaries 编译失败；而默认集本不含 cuda/directml/coreml，保留不影响目标。详见 §5.2。

## 7. 已知限制 / 风险（含 2026-06-20 实测结论）

- **7.1 mac 体积收益 = 0（实测推翻 §1.① 假设）**：删 cuda/directml feature 后 mac 二进制仍 54M（56,778,304 字节），零下降。原因：① config.rs 的 `#[cfg]` 早把 cuda/directml 的 Rust 引用排除，从未编进 mac 二进制；② cuda/directml 的 GPU 预编译库（libcuda/cudnn/DirectML.dll）无 mac 版，从未下载/链接。这两个 feature 在 mac 本就是 size no-op。**本改动价值不在 mac 体积，而在 §7.3 的 segfault defense-in-depth。**
- **7.2 linux/win 运行未实测 + 交叉 check 受阻**：环境仅 mac。交叉 `cargo check` 在 `openssl-sys`（linux）/`esaxx-rs`（win C++）受阻于目标工具链缺失，非 ort。运行正确性靠用户在对应平台本地 `cargo check` + 自测。
- **7.3 feature↔#[cfg] 非编译器硬约束（实测更正 §2/§5.4）**：ort EP 类型无条件编译，仅 `register()` 内 FFI 块按 feature gate。故「不一致」不会编译失败。但 feature 关闭仍提供**真·defense-in-depth**：cuda/directml feature off 时，即便有人退化 config.rs 的 cfg gate、在 mac 上注册 CUDA EP，`register()` 会直接返回 `MissingFeature`、**不走** FFI dlopen-libcuda（即 segfault 那条路径），从而不崩。这是本改动对 segfault 根源的二道防线（首道是 config.rs 的 cfg gate，已在 [[asr-hw-accel-release-segfault]] 修复）。
- **7.4 `default-features` 已定（实测后保留开启，非 false）**：见 §5.2/§6.2。
- **7.5 win CUDA 删除非编译必需（实测更正 §5.3）**：类型存在，留着也能编译。删除是运行时清洁（避免注册注定 MissingFeature 的死 EP），仍是正确改动。
- **7.6 CUDA 用户感知**：win NVIDIA 用户从 CUDA 回退到 DirectML，极端长音频批量转写可能略慢；实时录音转写影响可忽略。

## 8. 不做（YAGNI 边界）

- 不动 `asr_hardware_accelerated` 开关逻辑（现状保留）。
- 不动 qwen3-asr 跳 CoreML 逻辑（现状保留）。
- 不拆 per-platform crate（方案 C）。
- 不升级 ort 版本（仍 2.0.0-rc.12）。
- 不为 linux 额外支持 coreml、不为 mac 支持 cuda（跨平台无意义）。

## 📄 `2026-06-20-qwen3-asr-inference-audit.md`

# qwen3-asr 推理实现审查复核与修复

> Date: 2026-06-20
> 状态：审查的 6 条结论已逐条复核（对照 sherpa-onnx C++ 权威实现）；#1/#2/#5/#6 已修（926550d）；#3 经核实为幻象不改；#4 KV 正确 sizing 已实现（f160cea）。**2026-06-20 e2e 回归**：#2 前缀剥离清理两处 bug（漏竖线后缀检查恒假 + 不容忍 BPE 引导空格）致 `language Chinese` 泄漏，已修（6d72f0d）。**#7 纯静音早停守卫**（另一 AI 提出、复核确认真实：Rust `trim_audio_features` 全静音返回 `trimmed_len=0` 与 C++ 分叉 → `audio_token_len==0` 进 decoder 维度失配；已加早停 + 单测）。**全部已合并 main**。
> 已合并 main：审查修复 926550d（spec 490555c）+ #4 / 泄漏修复 / 文档（f160cea + 6d72f0d + 296c8ac）。原 `fix/qwen3-asr-review`、`perf/qwen3-asr-kv-cache-sizing` 分支均删。
> 关联文件：`crates/asr/src/qwen3_asr.rs`、参考实现 `sherpa-onnx/csrc/offline-recognizer-qwen3-asr-impl.{h,cc}` + `offline-qwen3-asr-model.cc`

## 1. 背景

对 `crates/asr/src/qwen3_asr.rs`（Qwen3-ASR offline 推理：conv_frontend → encoder → 自回归 decoder）收到一份 6 条结论的代码审查。审查本身可能基于幻觉/过时行号——**复核是关键**：每条都对照 sherpa-onnx 的 C++ 官方实现定性，区分「真实问题」与「幻象」，避免改错或引入回归。

模型：`csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25`。

## 2. 复核结论总表

| # | 审查结论 | 判定 | 处置 |
|---|---|---|---|
| 1 | 空输入死锁 | ✅ 真实（高危） | 已修 |
| 2 | auto 语言检测失效 | ✅ 真实（**审查对、原代码注释错**） | 已修 |
| 3 | 缺音频特征裁剪 | ❌ 幻象 | 不改 |
| 4 | KV cache 内存抖动 | ⚠️ 真实代价，但非 Rust 缺陷（C++ 同样） | 已修（§6） |
| 7 | 纯静音无早停（trim 分叉） | ✅ 真实（另一 AI 提出，复核确认） | 已修（§4 #7） |
| 5 | `Box::leak` 内存泄漏 | ✅ 真实（小） | 已修 |
| 6 | mel filterbank 稀疏密集相乘 | ✅ 真实（perf） | 已修 |

## 3. 关键证据（对照 C++）

### 3.1 #2 auto 语言 —— 审查正确，原代码注释错误

C++ `BuildSourceIds`：
```cpp
const std::vector<int64_t> *ids_after = &prompt_ids_after_;  // = Encode("<|audio_end|><|im_end|>\n<|im_start|>assistant\n")
if (!language.empty()) {
    auto language_ids = tokenizer_->Encode("language " + language);
    prompt_ids_after_with_language =
        prompt_ids_after_ + language_ids + {asr_text_token_id_};  // 仅此处追加 <asr_text>
    ids_after = &prompt_ids_after_with_language;
}
// source_ids = before + audio_pad×audio_token_len + ids_after
```

即：
- **language 非空** → prompt 以 `language <lang> <asr_text>` 结尾。
- **language 空（auto）** → prompt 以 `assistant\n` 结尾，**不含 `<asr_text>`**；由模型自行预测 `language <检测> <asr_text>` 再出文本。

Rust 原实现 `ids.push(ASR_TEXT)` 无条件追加（`qwen3_asr.rs` 修复前），在 auto 模式下：
- 跳过模型的语言自检（auto 失效）；
- 使 `decode_tokens` 里剥离 `language…<asr_text>` 前缀的清理逻辑成为**死代码**（C++ `GenerateText` 末尾有完全相同的清理，正为 auto 路径服务）。

原代码注释「`<asr_text>` 是生成起始标记，始终注入」论断有误。

### 3.2 #3 缺音频裁剪 —— 幻象

审查担心「`audio_pad` 占位符数（`audio_token_len`）少于 `audio_features` 帧数（`trimmed_len`）→ 跨注意力失配」。经核实**不会发生**：

- C++ `GenerateText` 同样 `audio_token_len = min(audio_token_len, trimmed_len)`，且**直接传完整 `trimmed_audio_features`**（仅当 `context_len > max_seq_len` 时才 `TruncateAudioFeatures`）。
- 结构不变式：`trimmed_len ≤ encoder 输出长 = conv_num_frames = valid_frames` 恒成立 ⇒ `audio_token_len = min(valid_frames, trimmed_len) = trimmed_len`。故 pad 数与特征帧天然对齐，无需额外裁剪。
- 溢出路径（`context > model max`）只对 >~150s 长音频触发；本 app 经 VAD 分段，每段远短于此，N/A。

### 3.3 #4 KV cache 内存抖动 —— 参考设计的共享代价

C++ `CreateEmptyKVCache`：
```cpp
std::vector key_shape = {batch, max_total_len_, kv_h, hd};  // max_total_len_ 取自 ONNX past_key dim1
// 每层 alloc + std::memset(..., 0, numel * sizeof(float))
```

即 C++ **同样每调用分配 `[1, max_total_len]×28` 层 KV cache 并 memset 0**——Rust 忠实镜像该模式，非 Rust 独有缺陷。

**零填充是承重的**：未写入位置 K/V=0 ⇒ 注意力贡献为 0，这正是「不显式 mask 未写位置也能正确工作」的原因。因此**裸 buffer 复用不清零会 corrupt 输出**（stale V≠0 会被 attend）。

唯一差异（也是真正的可优化点）：`max_total_len` 来源——C++ 读模型 `past_key` shape dim1（固定、可能 ≠ 2048）；Rust 硬编码 `2048.max(s0 + MAX_NEW_TOKENS)`。正确 sizing 见 §6 跟进。

## 4. 已实施修复（#1/#2/#5/#6/#7）

### #1 空输入死锁防御
`compute_mel_features` 入口对空 samples 早返回 `(0, MEL_NUM_BINS)`；`transcribe` 对 0 帧短路返回空文本。
- 根因：`samples.len()==0` 时反射条件 `s < 0 || s >= samples.len() as isize` 退化为 `s < 0 || s >= 0`（恒真），反射在 `-120 ↔ 119` 振荡，死循环卡死进程（在 Mutex 内 → 整个引擎死锁）。
- 对齐 C++ `Decode` 头部 `f.empty()` / `num_frames < 2` 返回空。

### #2 auto 语言 prompt 对齐 C++
`<asr_text>` 移入 `if !language.is_empty() && language != "auto"` 块内（与 `language <lang>` 一起注入）。
- 见 §3.1。修复后 auto 路径自洽：prompt 以 `assistant\n` 结尾 → 模型吐 `language <检测> <asr_text> <文本>` → `decode_tokens` 剥离前缀。
- **行为变更**：auto 模式现在真正走模型语言自检（原为强行带 `<asr_text>` 直出文本）。需本地 e2e 验证中英混合场景。
- **2026-06-20 e2e 回归（已修）**：本地真模型跑出 `language Chinese进行询问。` 泄漏（部分段泄漏、部分段干净）。根因在 `decode_tokens` 的前缀剥离清理有两处叠加 bug：
  1. 后缀检查 `ends_with("<asr_text>")` **漏竖线** —— 特殊 token 渲染为 `<|asr_text|>`（带 `|`），该检查恒假，剥离永不触发。
  2. `starts_with("language ")` **不容忍首 token 的 BPE 引导空格**（`Ġlanguage` 解码为 ` language`）—— int8 模型对首 token 选择本就不稳，正是「时灵时不灵」的来源。
  - **修法**：asr_text token 已按 ID 确认存在（后缀字符串检查冗余），剥离判定改为只校验其前文本 `is_language_scaffold(text) = text.trim_start().starts_with("language ")`。抽出纯函数便于单测（无 tokenizer 依赖）。对齐 C++ `rfind("language ", 0) == 0` + 按 ID 找 token 的语义，但更鲁棒（不依赖特殊 token 的字符串渲染）。

### #5 `cache_names` 去 `Box::leak`
56 个 KV cache 输入名（`cache_key_i` / `cache_value_i`）提升为进程级 `static CACHE_NAMES: Lazy<Vec<(&'static str, &'static str)>>`，`Box::leak` 仅发生一次。
- 原实现每实例化 leak 56 个串；模块级 `transcribe`（CLI 路径）每次调用都 `Qwen3AsrEngine::new` → 每次泄漏。LRU 淘汰/频繁切换模型时累积。

### #6 mel filterbank 稀疏化
新增 `static MEL_FILTERBANK_RANGE: Lazy<Vec<(usize, usize)>>`，预计算每个 mel bin 的非零频率区间 `[start, end)`；内层循环 `for k in start..end` 只扫非零段。
- filterbank 是三角滤波、高度稀疏（201 个频率里大部分权重为 0），跳过 ~90% 的 `× 0.0` 无效乘加。
- 区间内全非零（三角滤波在 `[left_hz, right_hz]` 内单调升再降，无内部空洞）→ 数值结果不变。

### #7 纯静音早停守卫（trim 分叉）
`transcribe` 在 `audio_token_len = valid_frames.min(trimmed_len)` 后加 `if audio_token_len == 0 { return Ok(String::new()); }`，对齐 C++ `GenerateText` 的 `if (audio_token_len <= 0) { result.text=""; return; }`。
- 根因：Rust `trim_audio_features` 全静音（所有帧 `|v| < eps`）返回 `(原张量, 0)`，与 C++ `TrimAudioFeatures` **分叉**——C++ 全静音 `if (A_valid <= 0) return audio_features;` 返回原张量且 trimmed_len 不变（仍 >0）。Rust 的 `0` 让 `audio_token_len=0` → prompt 含 0 个 `<|audio_pad|>`，与送入 decoder 的完整 `audio_features [1, conv_num_frames, H]` 维度失配 → ONNX 报错或不可控幻象。
- 可由纯静音 / 降噪后近静音段触发（VAD 可能放过）。
- 锁定 `trim_audio_features` 全静音 → `trimmed_len=0` 行为的单测（防误改成返回 `a` 让静音漏进 decoder）。

## 5. 验证

- `cargo check -p octopus-asr-local --all-targets`：零 warning。
- `cargo test -p octopus-asr-local`：50 passed / 0 failed（含新增 5 个回归测试）：
  - `compute_mel_features_empty_samples_does_not_hang`（#1 死锁回归）
  - `compute_mel_features_single_sample_no_panic`（反射边界 len==1）
  - `mel_filterbank_range_is_contiguous_nonzero`（#6 正确性：区间内全非零、区间外全零）
  - `is_language_scaffold_recognizes_self_detect_prefix`（#2 e2e 回归：自检前缀识别，含 BPE 引导空格容错）
  - `trim_audio_features_all_silence_yields_zero_trimmed_len`（#7：全静音 trim→0，早停守卫触发源）
- **#2 e2e**：本地真模型首跑暴露清理 bug（`language Chinese` 泄漏），已修（§4 #2）；修后再验由用户本地重跑确认不再泄漏（环境无模型）。
- **`cargo check --workspace`**：审查时 main 既有报错（desktop↔llm `test_connection` 导出缺失），**已修复**——`crates/llm/src/lib.rs:6` 现 `pub use client::{polish, test_connection}`，desktop `settings_commands.rs:292` 正常引用。与本次 asr 改动无关。

## 6. #4 跟进（已实现 + e2e 验证）

**目标**：KV cache 正确 sizing，消除硬编码 `2048` floor 的潜在失配 + 动态维度下省内存。

**已实施方案**（对齐 C++ `InitDecoderSession`）：
- 新增 `fn decoder_kv_max_len(decoder: &Session) -> Option<usize>`：按名查找 decoder 的 `cache_key_0` 输入，读其 shape dim1；`>0` 返回 `Some`，动态（-1）返回 `None`。
- `Qwen3AsrEngine` 新增字段 `kv_max_len: Option<usize>`，`new()` 中从 decoder session 读取并存储（`log::debug` 打印实际值/动态）。
- `transcribe` 中 `let max_total_len = self.kv_max_len.unwrap_or(s0 + MAX_NEW_TOKENS);` 替代原 `2048.max(s0 + MAX_NEW_TOKENS)`。
  - dim1 具体 → 用模型声明值（正确 sizing，对齐 C++）。
  - 动态 → 仅装 prompt+生成（`s0 + MAX_NEW_TOKENS`），短音频下比 2048 floor 显著省内存。loop 的 `cur_len + s <= max_total_len` 写入守卫与 `cur_len < max_total_len` 终止条件保证不越界。

**验证**：`cargo check -p octopus-asr-local` 零 warning；`cargo test -p octopus-asr-local` 50 passed/0 failed（与 §5 一致；#4 未新增单测，故计数不变）。`decoder_kv_max_len` 依赖 ONNX session，无法离线单测。

**已 e2e 验证（2026-06-20）**：本地真模型跑出 `dim1 动态 → 按 s0+MAX_NEW_TOKENS sizing`——该模型 `past_key` dim1 确为动态（-1），0.6B 与切换后的更大模型均走动态回退路径；热启动 RTF 0.19~0.20（5× 实时），sizing 行为与内存正常。两条路径中动态路径已实测；dim1 具体路径（`Some` 分支）因现用模型均为动态未直接覆盖，但逻辑与 C++ `InitDecoderSession` 一致。

**未做（YAGNI/需实测）**：buffer 复用消除 per-call alloc。须**保留清零**（零填充承重，见 §3.3）；仅当确认模型按 `cache_position`/`attention_mask` mask 未写位置时才可免清零——需实测，暂不做。

## 7. 不做（YAGNI 边界）

- #3 的 `TruncateAudioFeatures` 防御性裁剪（正常路径天然对齐，加了反而偏离参考）。
- KV cache 免清零复用（需模型 masking 行为实测，未验证前不动）。
- 长音频（>150s）的 `context > max_seq_len` 溢出裁剪（VAD 分段场景 N/A）。

## 📄 `2026-06-20-zipformer-transducer-design.md`

# Zipformer Transducer（RNN-T）引擎设计

## 背景

原 `ZipformerEngine` 仅支持 CTC 解码（单 session → log_probs → argmax）。新增两个 Transducer 模型需要 RNN-T 解码（encoder + decoder + joiner 三 session 架构）：

- `csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`（154M, encoder_dim=512）
- `csukuangfj/sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30`（726M, encoder_dim=768）

## 设计决策

### 1. 重命名而非扩展

`ZipformerEngine` → `ZipformerCtcEngine`，新建 `ZipformerTransducerEngine`。原因：
- CTC 和 Transducer 解码控制流根本不同（CTC: 单 session argmax + blank/repeat skip；Transducer: 三 session RNN-T greedy decoding with inner emit loop）
- 符合代码库已有模式（每引擎一个 struct）
- 共享代码已提取为自由函数（`load_vocab`、`initial_encoder_states`、`decode_token_ids`）

### 2. 路由层检测（不新增 EngineCategory）

`EngineCategory::Zipformer` 分支内检测 `decoder.onnx` 存在性：
- 有 → `ZipformerTransducerEngine`
- 无 → `ZipformerCtcEngine`

### 3. RNN-T Greedy Decoding

遵循 sherpa-onnx 标准流式 greedy search 约定：
- `token_buf` 初始 `[-1, ..., -1, 0]`（长度 = context_size，末位 blank）
- 每个 encoder frame：`joiner(enc_frame, decoder_out) → logit → argmax`
- 非 blank：发射 token，滑动窗口更新，重跑 decoder
- blank：移到下一 encoder frame
- 内循环安全上限 20 次/frame

## 共享函数提取

| 函数 | 位置 | 用途 |
|---|---|---|
| `load_vocab(hf_path)` | zipformer.rs | tokens.txt → Vec<String> |
| `initial_encoder_states(session)` | zipformer.rs | 遍历 encoder inputs 创建零张量初始状态 |
| `decode_token_ids(vocab, is_bbpe, ids)` | zipformer.rs | token ID 序列 → 文本（BBPE + SentencePiece byte-fallback） |

## 引擎结构

```rust
pub struct ZipformerTransducerEngine {
    encoder_session: Mutex<Session>,
    decoder_session: Mutex<Session>,
    joiner_session: Mutex<Session>,
    chunk_len: usize,       // T=45（从 encoder metadata 读）
    chunk_shift: usize,     // decode_chunk_len=32
    context_size: usize,    // 2（从 decoder metadata 读）
    vocab: Vec<String>,
    is_bbpe: bool,
    initial_states: Vec<(String, StateValue)>,
    is_whisper: bool,       // 两新模型 feature=whisper
}
```

## 数据流

```
音频 → compute_whisper_features_linear → normalize → chunked encoder inference
                                                     ↓
                                              encoder_out [T', enc_dim]
                                                     ↓
                                    RNN-T greedy decoding (per frame):
                                      decoder(token_buf) → decoder_out
                                      joiner(enc_frame, decoder_out) → logit
                                      argmax → blank? next frame : emit + re-run decoder
                                                     ↓
                                              decode_token_ids → 文本
```

## 流式 Transducer 引擎（StreamingZipformerTransducer）

Transducer 模型（zh-int8 / xlarge）原生支持流式（`is_streaming=1`），故除离线引擎外还需流式引擎。

### 流式引擎分流

`StreamingSession::new`（`streaming_engine.rs`）检测模型目录下 `decoder.onnx` 存在性：
- **无 `decoder.onnx`** → `StreamingZipformer`（CTC，单 session log_probs argmax）
- **有 `decoder.onnx`** → `StreamingZipformerTransducer`（RNN-T，三 session greedy decoding）

两者实现 `ZipformerStreamOps` trait，`StreamingSession` 通过 trait 统一分发 `accept_samples` / `flush` / `finish` / `reset`，消除重复代码。

### 跨 chunk 持久状态

| 状态 | 说明 |
|---|---|
| `token_buf: Vec<i64>` | decoder 上下文窗口（长度 = context_size，默认 2），初始化 `[-1, ..., -1, 0]` |
| `emitted_ids: Vec<usize>` | 累积输出 token ID |
| `states: Vec<(String, StateValue)>` | encoder 缓存（cached_key/N、cached_val/N 等，与 CTC 相同） |

### 关键设计：new_from_entry

`StreamingSession::new` 经 `resolve_active_engine` 解析 entry 后，直接传 `entry` 给流式引擎的 `new_from_entry()`——而非传 bare_name 让引擎内部再查 DB。避免双重 DB 查找 + 可能选错 entry。

### run_chunk 两阶段借用

ort 2.0.0-rc.12 的 `SessionOutputs` 持有 session 的借用，调 decoder/joiner 前必须结束该借用。`run_chunk` 采用两阶段：
1. encoder session run → `SessionOutputs` → 提取 encoder_out 到 owned `Vec<f32>`（借用结束）
2. 用 owned 数据调 decoder/joiner session

### 流式 RNN-T 解码

每个 chunk 的 encoder_out 逐 frame 跑 joiner → argmax：
- **非 blank(0)**：发射 token、`token_buf` 滑动窗口更新、重跑 decoder 获取新 decoder_out
- **blank(0)**：移到下一 frame
- 内循环安全上限 20 次/frame（防理论无限循环）

## Whisper 特征归一化（3 个根因修复）

对比 sherpa-onnx 官方 C++ 实现，发现并修复 3 个导致流式 Transducer 质量差的根因：

### 根因 1：归一化公式错误

sherpa-onnx `NormalizeWhisperFeatures`（`math.cc`）：
```
mel = (max(log10(clamp(x, 1e-10)), max_v - 8.0) + 4.0) / 4.0
```
输出范围 ~0-2。我们的实现错误地用 `clamped - clamp_min`（输出范围 0-8，尺度差 4 倍），ONNX 模型输入分布不匹配。修正为 `(clamped + 4.0) / 4.0`。

### 根因 2：Transducer history 泄漏

`StreamingZipformerTransducer::process_chunks` 保留**全部未消费样本**作为 `history_samples`（可达上万样本），而非仅 1 帧（160 samples）。导致每次重算特征时归一化 max_v 剧烈跳变。修复为与 CTC 引擎一致的 1-frame history。

### 根因 3：流式归一化 scope

sherpa-onnx 做 **per-chunk 归一化**（每个 chunk 独立 normalize，配合增量特征计算）。此前误改为 pseudo-global（每次重算 history+buffer 全局归一化），但由于 history/buffer 内容每次不同，max_v 仍不稳定。回退为 per-chunk 归一化——与 sherpa-onnx 行为一致。

修复覆盖 CTC + Transducer 两套流式引擎的 `process_chunks` 和 `finish` 共四处 + `normalize_whisper_features` 函数本身。


