# 已归档设计规格（2026-06-20 ~ 2026-06-21）

> 本文件合并了以下已完成的设计规格。原文档已删除。

## 包含的规格

- 2026-06-20-archived-design（baidu-asr / bytedance-asr / clipboard-restore-race / download-model-integration / model-download / model-management-gui / moonshine-asr / paraformer-fbank-feature-extraction-fix / polish-prompt-table / tencent-asr / toggle-stop-polish-race）

---

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



---


---
## 2026-06-21-baidu-asr-design

# 百度智能云实时语音识别接入设计

> 文档：https://ai.baidu.com/ai-doc/SPEECH/jlbxejt2i
> 对标实现：AliyunStreamSession / ByteDanceStreamSession / TencentStreamSession

## 功能概述

接入百度智能云实时语音识别 WebSocket API，作为第四个云端 ASR provider。鉴权信息在 START 帧 JSON 中直接传入（appid + appkey），协议简洁。

## 协议要点

### Endpoint

固定 `wss://vop.baidu.com/realtime_asr?sn=<UUID>`

- `sn`：用户自定义的请求标识（UUID 即可），用于排查日志

### 帧类型

| 帧 | Opcode | 内容 |
|---|---|---|
| START（开始） | Text | `{"type":"START","data":{...}}` |
| 音频数据 | **Binary** | 原始 PCM s16le（无头、无压缩） |
| FINISH（结束） | Text | `{"type":"FINISH"}` |
| CANCEL（取消） | Text | `{"type":"CANCEL"}` |

### START 帧 data 参数

| 参数 | 必填 | 说明 |
|---|---|---|
| `appid` | 是 | AppID（int） |
| `appkey` | 是 | API Key |
| `dev_pid` | 是 | 语种模型（int，推荐 15372 中文加强标点） |
| `cuid` | 是 | 设备唯一标识（统计 UV，不影响识别） |
| `format` | 是 | 固定 `"pcm"` |
| `sample` | 是 | 固定 `16000` |
| `user` | 可选 | 多方言模型（dev_pid=15376）必填 |

### 音频发送

- **Binary 帧**：原始 PCM s16le，**无头、无压缩**
- **每帧**：160ms = 5120 字节（范围 20-200ms）
- **间隔**：建议实时（160ms），最长不超过 5s（否则超时断开）

### 响应格式（Text JSON）

| 字段 | 说明 |
|---|---|
| `err_no` | 0=正常，非 0=错误 |
| `err_msg` | 错误描述 |
| `type` | `MID_TEXT`（临时结果）/ `FIN_TEXT`（最终结果）/ `HEARTBEAT`（心跳） |
| `result` | 识别文本 |
| `start_time` / `end_time` | 句时间戳（仅 FIN_TEXT） |

### 结束信号

客户端发 `{"type":"FINISH"}` → 服务端完成识别后自行关闭连接。

### dev_pid 取值

| PID | 模型 | 标点 |
|---|---|---|
| 1537 | 中文普通话 | 弱标点 |
| **15372** | 中文普通话 | **加强标点（推荐）** |
| 15376 | 中文多方言 | 弱标点（需 user 参数） |
| 1737 | 英语 | 无标点 |
| 17372 | 英语 | 加强标点 |

## DB 映射

| DB 字段 | 百度含义 | 示例 |
|---|---|---|
| `source` | **AppID** | `105xxx17` |
| `secret_key` | **API Key**（appkey） | `UA4oPSxxxxkGOuFbb6` |
| `model_name` | **dev_pid**（字符串形式） | `15372` |

> Endpoint 固定，不存 DB。百度实时识别不使用 access_token / SecretKey，鉴权全在 START 帧。

## 与其他三个 provider 的差异

| 维度 | Aliyun | ByteDance | Tencent | **Baidu** |
|---|---|---|---|---|
| 鉴权 | Bearer header | X-Api-Key header | URL HMAC-SHA1 | **START 帧 appid+appkey** |
| 初始化 | run-task JSON | FULL_CLIENT_REQUEST binary | URL 参数 | **START 帧 JSON** |
| 音频帧 | Raw PCM / base64 | gzip(PCM) | Raw PCM | **Raw PCM binary** |
| 响应 | JSON text | Binary+gzip(JSON) | JSON text | **JSON text** |
| 临时结果 | result-generated | result.text | slice_type=0/1 | **MID_TEXT** |
| 最终结果 | task-finished | flags=0x3 | final=1 | **FIN_TEXT** |
| 结束信号 | finish-task JSON | 末帧 flags=0x2 | `{"type":"end"}` | **`{"type":"FINISH"}`** |

## 架构设计

### EngineCategory::Baidu

- `crates/asr/src/config.rs`：新增 `Baidu` 变体
- `resolve_category`：`provider == "baidu"` → `Some(Baidu)`
- `is_streaming_engine`：排除 Baidu（与其他三个云 provider 一致）
- `coordinator::is_cloud_engine`：追加 `Some(Baidu)`

### BaiduStreamSession

- 文件：`crates/desktop/src/baidu_stream.rs`
- 接口与其他三个完全一致：`open` / `push_pcm` / `finish` / `try_recv_text` / `close_async`
- 复用 `aliyun_stream::{PcmFrame, StreamEvent}`
- 无额外 cargo 依赖（无 HMAC/gzip/base64 需求）

### CloudSession enum

`crates/desktop/src/cloud_session.rs` 新增 `Baidu(BaiduStreamSession)` 变体。

### 文本累积策略

- `Vec<String>` 存所有 FIN_TEXT 的 result（按顺序拼接）
- `current_partial` 存当前 MID_TEXT 的 result
- `StreamEvent::Text` = `fin_texts.join("") + current_partial`
- FINISH 发送后服务端关闭连接 → `StreamEvent::Finished`

### dev_pid 处理

DB `model_name` 列存 dev_pid 字符串（如 `"15372"`），`open()` 时解析为 `i64` 填入 START 帧 `data.dev_pid`。


---
## 2026-06-21-bytedance-asr-design

# 火山引擎豆包大模型流式 ASR 接入设计

> 对标文档：[双向流式模式（优化版本）](https://www.bytedance.com/docs/6561/1354869)
> 对标实现：`DashScopeStreamSession`（aliyun / dashscope feature）

## 1. 目标

接入火山引擎豆包大模型 ASR **双向流式模式（优化版本）** 作为第二个云端 ASR provider，与
现有阿里云 DashScope（`EngineCategory::Aliyun`）并列。用户申请 API Key 后填入 DB 即可使用。

### 非目标

- 不接入单向流式 / nostream 模式（双向流式优化版即可覆盖）
- 不接入小模型 V1 协议（仅接入大模型 `bigmodel_async`）
- 不做 TTS（仅 ASR）

## 2. 协议规格

### 2.1 Endpoint

```
wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async
```

固定 host，无 region/cluster 参数。资源路由通过 WS 握手 header `X-Api-Resource-Id` 指定。

### 2.2 认证（WS 握手 Headers）

新控制台 auth：
```
X-Api-Key: <api_key>
X-Api-Resource-Id: volc.bigasr.sauc.duration | volc.seedasr.sauc.duration
X-Api-Request-Id: <UUID>
X-Api-Sequence: -1
```

| 模型 | 计费 | Resource ID |
|---|---|---|
| Doubao ASR 1.0 | 时长 | `volc.bigasr.sauc.duration` |
| Doubao ASR 1.0 | 并发 | `volc.bigasr.sauc.concurrent` |
| Doubao ASR 2.0 | 时长 | `volc.seedasr.sauc.duration` |
| Doubao ASR 2.0 | 并发 | `volc.seedasr.sauc.concurrent` |

**约定**：Resource ID 存入 DB `source` 字段（如 `volc.bigasr.sauc.duration`），endpoint 固定。
API Key 存入 `secret_key`，与 aliyun 一致。

### 2.3 二进制帧协议（4B Header + Payload）

所有整数字段大端序。

**Header（4 字节）**：
```
Byte 0: [Protocol Version 4b=0001] [Header Size 4b=0001]  → 0x11
Byte 1: [Message Type 4b]       [Msg Type Flags 4b]
Byte 2: [Serialization 4b]      [Compression 4b]
Byte 3: [Reserved 8b=0x00]
```

**Message Types**：
| 值 | 常量 | 方向 | 含义 |
|---|---|---|---|
| 0x1 | FULL_CLIENT_REQUEST | C→S | 带 JSON config 的初始帧 |
| 0x2 | AUDIO_ONLY_REQUEST | C→S | 纯音频帧 |
| 0x9 | FULL_SERVER_RESPONSE | S→C | 带 JSON 结果的响应 |
| 0xF | ERROR_RESPONSE | S→C | 错误 |

**Message Type Flags**：
| 值 | 常量 | 含义 |
|---|---|---|
| 0x0 | NO_SEQUENCE | 无 sequence number |
| 0x1 | POS_SEQUENCE | 有 sequence number |
| 0x2 | NEG_SEQUENCE | 最后一帧（负包），无 seq |
| 0x3 | NEG_WITH_SEQUENCE | 最后一帧（负包），有 seq |

**Serialization**：0x0=NONE（纯音频）, 0x1=JSON
**Compression**：0x0=NONE, 0x1=GZIP

### 2.4 客户端发帧

**FULL_CLIENT_REQUEST（初始 config）**：
```
[Header: 0x11 0x11 0x11 0x00]    // ver=1, hdr=1, type=FULL_CLIENT_REQUEST, flags=NO_SEQ, ser=JSON, comp=GZIP
[Payload Size 4B BE]
[gzip(JSON)]
```

JSON config（minimal）：
```json
{
  "user": { "uid": "<随机>" },
  "audio": { "format": "pcm", "codec": "raw", "rate": 16000, "bits": 16, "channel": 1, "language": "zh-CN" },
  "request": { "model_name": "bigmodel", "enable_itn": true, "enable_punc": true, "enable_ddc": false, "show_utterances": true }
}
```

**AUDIO_ONLY_REQUEST（正常音频帧）**：
```
[Header: 0x11 0x20 0x01 0x00]    // type=AUDIO_ONLY, flags=NO_SEQ, comp=GZIP
[Payload Size 4B BE]
[gzip(raw_audio)]
```

**AUDIO_ONLY_REQUEST（最后一帧，EOF）**：
```
[Header: 0x11 0x22 0x01 0x00]    // type=AUDIO_ONLY, flags=NEG_SEQUENCE（负包=末帧）
[Payload Size 4B BE]
[gzip(raw_audio)]
```

### 2.5 服务端响应

**FULL_SERVER_RESPONSE**：
```
[Header: 0x11 0x91 0x11 0x00]    // type=FULL_SERVER_RESPONSE, flags=POS_SEQ, ser=JSON, comp=GZIP
[Sequence 4B BE]
[Payload Size 4B BE]
[gzip(JSON)]
```

末帧 flags=0x3（NEG_WITH_SEQUENCE）。

响应 JSON：
```json
{
  "result": {
    "text": "累积全文",
    "utterances": [{ "definite": true, "text": "此句已确定", "start_time": 0, "end_time": 1705 }]
  }
}
```

- `result.text`：累积全文
- `utterances[].definite=true`：此句已 finalize
- 末帧 flags=0x3：全部结束

**ERROR_RESPONSE**：
```
[Header: 0x11 0xF1 0x00 0x00]    // type=ERROR, flags=POS_SEQ
[Error Code 4B BE]
[Error Msg Size 4B BE]
[Error Msg UTF-8]
```

### 2.6 优化版（bigmodel_async）特性

- **事件驱动响应**：不是每包音频都回，仅在结果变化时回——降低 RTF 和尾延迟
- **两遍识别**（配合 `enable_punc` + `show_utterances`）：流式 partial + `definite=true` 最终句
- 与标准双向流式相同的二进制协议，仅响应行为不同

## 3. 架构设计

### 3.1 新增 EngineCategory::ByteDance

与 `Aliyun` 平级的云端 provider：

```rust
pub enum EngineCategory {
    // ... 现有 6 个本地 ...
    Aliyun,       // DashScope Fun-ASR
    ByteDance,   // 豆包大模型 bigmodel_async
}
```

- `provider='bytedance'` → 路由到 `ByteDance`
- 与 `Aliyun` 一样：`is_streaming=true`，但在桌面端走独立 `CloudStreaming` 路径
- `is_cloud_engine` 扩展：`ByteDance || Aliyun` 均判定为云端

### 3.2 infra 层：AsrSection 新增 bytedance 字段

```rust
pub struct AsrSection {
    // ... 现有 ...
    /// 阿里云云端 ASR（DashScope Fun-ASR 实时）。
    pub aliyun: Option<HashMap<String, ModelEntry>>,
    /// 火山引擎豆包大模型 ASR（bigmodel_async 双向流式优化版）。
    #[serde(default)]
    pub bytedance: Option<HashMap<String, ModelEntry>>,
}
```

`all_sections` 维度从 7→8。

### 3.3 DB seed

```sql
('asr','bytedance','Doubao-ASR','doubao-asr-1.0-streaming','volc.bigasr.sauc.duration','zh','火山引擎豆包大模型 ASR 1.0 双向流式（bigmodel_async，DashScope-style key 填 secret_key）',0,0,1),
('asr','bytedance','Doubao-ASR-2.0','doubao-asr-2.0-streaming','volc.seedasr.sauc.duration','zh','火山引擎豆包大模型 ASR 2.0 双向流式（bigmodel_async，时长计费）',0,0,0);
```

- `provider='bytedance'`，`category='Doubao-ASR'`
- `source` = Resource ID（如 `volc.bigasr.sauc.duration`）
- `secret_key` 空（用户填 API Key）
- `is_streaming=1`（Doubao-ASR 1.0 默认开启）

### 3.4 ByteDanceStreamSession

镜像 `DashScopeStreamSession` 接口（`push_pcm` / `try_recv_text` / `finish` / `close_async`），
但内部实现完全不同——使用火山的二进制帧协议而非 DashScope 的 JSON 文本协议。

**关键模块**：`crates/desktop/src/bytedance_stream.rs`（feature gated `dashscope` 或新建 `bytedance` feature）

**设计决策**：复用 `dashscope` feature gate（而非新建 `bytedance` feature）。
原因：两个 provider 都是云端 WS 流式，feature 控制的是"是否编译云端流式路径"，两者语义一致。
避免新增 feature 增加 build matrix 复杂度。

**结构**：
```rust
pub struct ByteDanceStreamSession {
    pcm_tx: mpsc::UnboundedSender<PcmFrame>,
    result_rx: mpsc::UnboundedReceiver<StreamEvent>,
}
```

接口与 `DashScopeStreamSession` 完全一致，coordinator 可用同一 `StreamEvent` enum。

### 3.5 coordinator 路由

`is_cloud_engine` 扩展：
```rust
fn is_cloud_engine(config: &AppConfig) -> bool {
    let cat = resolve_engine_category(&config.asr_engine);
    cat == Some(EngineCategory::Aliyun) || cat == Some(EngineCategory::ByteDance)
}
```

`resolve_cloud_config` 根据 category 分派到 DashScope 或 VolcEngine：
```rust
fn resolve_cloud_config(engine_spec: &str) -> Result<CloudProvider, String> {
    let cat = resolve_engine_category(engine_spec);
    match cat {
        Some(EngineCategory::Aliyun) => Ok(CloudProvider::Aliyun { ... }),
        Some(EngineCategory::ByteDance) => Ok(CloudProvider::ByteDance { ... }),
        _ => Err(...),
    }
}
```

`Stage::CloudStreaming.session` 字段改为 enum：
```rust
session: Option<CloudSession>,  // enum: Aliyun(DashScopeStreamSession) | ByteDance(ByteDanceStreamSession)
```

或更简单：trait object（但 `DashScopeStreamSession` 方法非 async-safe，trait 化需仔细）。
**采用 enum 方案**——显式分派，类型安全，避免动态分派。

### 3.6 配置（config.yaml / AppConfig）

用户通过设置 UI 选引擎（与 aliyun 一致），`AppConfig.asr_engine` 存
`bytedance:Doubao-ASR:doubao-asr-1.0-streaming` 格式（3-part spec）。
无需新增 AppConfig 字段——复用 `asr_engine` + `language`。

## 4. 不变量

1. **endpoint 固定**：`wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async`，不通过 DB 配置
2. **Resource ID = DB source**：用户选模型时 source 字段即 Resource ID，直接用作 WS header
3. **secret_key = API Key**：与 aliyun 一致的 key 存储约定
4. **PCM 16kHz mono s16le**：与现有 coordinator 录音管线一致，无需重采样
5. **StreamEvent 共享**：两个 provider 都返回统一的 `StreamEvent`，coordinator 上层逻辑零改动

## 5. 降级路径

- **API Key 缺失**：`resolve_cloud_config` 返回 Err，coordinator 报错给用户（与 aliyun 一致）
- **WS 连接失败**：`ByteDanceStreamSession::open` 返回 Err，coordinator 回退 `is_speaking=false`
- **无 `dashscope` feature**：云端引擎不可用（`is_cloud_engine` 返回 false 时走本地 VadSegmented）

## 6. 与 Aliyun（DashScope）的关键差异

| 维度 | Aliyun | ByteDance |
|---|---|---|
| 协议 | JSON 文本帧 | 二进制帧（4B header + payload） |
| Endpoint | DB source 字段（wss://） | 固定 `openspeech.bytedance.com/api/v3/sauc/bigmodel_async` |
| Auth | `Authorization: bearer <key>` | `X-Api-Key: <key>` + Resource ID header |
| 音频编码 | 裸 PCM s16le bytes | gzip(PCM s16le) |
| 结果解析 | JSON 文本（run-task 协议） | gzip(JSON) 从二进制帧 payload |
| EOF 信号 | `finish-task` JSON | 末帧 flags=0x2（NEG_SEQUENCE） |


---
## 2026-06-21-clipboard-restore-race-design

# 剪贴板恢复竞态修复（desktop 审查一3）

**日期**: 2026-06-21
**状态**: ✅ 已实现（commit `e0f1420`，`PASTE_RESTORE_DELAY = 200ms`，详见 §3）
**来源**: desktop 审查一3（`2026-06-20-desktop-implementation-audit.md` §3.3 + `2026-06-20-desktop-audit-followups.md` §1，原 P2 延后项）。**注**：两来源文件已于 2026-06-21 归档——audit spec 见 `specs/2026-06-20-archived-design.md`、followups plan 见 `plans/2026-06-20-archived-plans.md`。
**分支**: `worktree-clipboard-restore-race`（隔离实现，main 让给 e2e 测试）

## 1. 背景

paste 流程（`paste_method = "clipboard"`，默认）经剪贴板粘贴识别文本：写识别文本到剪贴板 → Cmd/Ctrl+V → 恢复用户原剪贴板内容。

「恢复」若发生在系统粘贴动作落地之前，目标应用读到的是已恢复的旧剪贴板内容，而非刚写入的识别文本——用户看到的是自己之前的剪贴板，不是识别结果。

**触发面**：仅 `write_to_clipboard = false`（不保留识别结果）路径触发——此时才需要 save/restore 原剪贴板。`write_to_clipboard = true` 不恢复，无竞态。慢系统 / 高负载 / 慢速目标应用粘贴路径上偶发，低概率低影响。

## 2. 根因

> **行号为修复前快照**（`e0f1420` 前；常量 `PASTE_RESTORE_DELAY` 插入后整体下移）。本节描述修复前的时序根因，定位以函数/符号为准（`paste_via_clipboard`），现况见 `crates/desktop/src/paste.rs`。

`crates/desktop/src/paste.rs::paste_via_clipboard`（L71-129）时序：

```
read saved（L80）→ write_text(text)（L86）→ sleep 50ms（L89）
→ enigo Cmd/Ctrl+V（L109-117）→ sleep 50ms（L119）→ write_text(saved) 恢复（L125）
```

竞态窗口 = L119 的 50ms。该 sleep 旨在等系统粘贴落地，但 50ms 在慢系统/高负载下不足——Cmd+V 触发的粘贴异步未完成，L125 已把原剪贴板写回，目标应用随后读取时拿到旧内容。

**L89（写剪贴板后 50ms）非竞态点**：`write_text` 同步写入，50ms 足等其稳定供 Cmd+V 读取。

## 3. 方案

**纯延迟，固定 200ms，不可配置。**

- `probe「粘贴已落地」信号`：跨平台无可靠实现（系统剪贴板不暴露「已被目标应用读取」状态），YAGNI。
- `可配置 paste_restore_delay_ms`：当前无按机器调优需求，YAGNI；如未来某平台实测仍竞态再加。

### 3.1 改动

`crates/desktop/src/paste.rs`：

1. 顶部（`use` 之后、`PasteMethod` 之前）新增常量 + 注释：

```rust
/// Cmd+V 后等待系统粘贴落地、再恢复原剪贴板的延迟。
/// 审查一3 竞态修复：原 50ms 在慢系统/高负载下不足——粘贴未落地就恢复，
/// 旧内容被粘进目标应用。200ms 为保守估值；跨平台无可靠「已落地」信号，
/// 故纯延迟、固定值（probe / 可配置均判 YAGNI）。
const PASTE_RESTORE_DELAY: Duration = Duration::from_millis(200);
```

2. L119 替换：

```rust
std::thread::sleep(Duration::from_millis(50));
```
→
```rust
std::thread::sleep(PASTE_RESTORE_DELAY);
```

### 3.2 不改

- **L89 sleep 50ms**：语义为等 `write_text` 落地，同步写入下 50ms 足够，非竞态点。
- **L124-126 恢复守卫**：`!saved.is_empty()` 跳过空 saved（保护非文本剪贴板图片/富文本不被空文本覆盖）——已正确。
- **`write_to_clipboard = true` / `paste_direct` / `paste_method = none`**：不恢复原剪贴板，无竞态。

## 4. 测试

无单元测试（系统剪贴板 + enigo GUI 交互，无法离线测；与 connection-test 同理 YAGNI）。

手动 e2e（补入 `2026-06-20-desktop-audit-followups.md` §2 GUI e2e 清单）：
- `write_to_clipboard = false` + 慢系统/高负载（前台跑重任务）→ 识别粘贴 → 确认目标应用粘进的是识别文本（非之前剪贴板内容）。
- 回归：`write_to_clipboard = true` 路径行为不变（结果留剪贴板）。

## 5. 风险

- **paste 总延迟 +150ms**（50→200ms）。paste 后用户本在等粘贴落地，可接受；固定值无法按机器调优，但 200ms 保守覆盖慢系统。
- 仅缓解非根除：极端慢粘贴路径（>200ms）仍可能竞态。属可接受残余风险（触发概率极低，且无可靠 probe 信号）。

## 6. 涉及文件

| 文件 | 变更 |
|---|---|
| `crates/desktop/src/paste.rs` | 新增 `PASTE_RESTORE_DELAY` 常量 + L119 sleep 改用常量 |
| `docs/superpowers/plans/2026-06-20-desktop-audit-followups.md` | §2 GUI e2e 清单补「剪贴板恢复竞态」验证项；§1 P2 状态改为已实现 |


---
## 2026-06-21-download-model-integration-design

# octopus-download 接入模型管理（阶段1）设计

> 2026-06-21。4 阶段体积优化工程的第 1 阶段。整体方案见会话记录；本 spec 只覆盖阶段1。
> 相关：download crate spec `2026-06-21-model-download-design.md`、TLS/体积分析 `docs/download_architure.md`。

## 1. 背景与定位

`octopus-download` crate（main `a2bef60`）已完成：通用 HF 模型下载器（分块并发 + 断点续传 sidecar + SHA256 校验 + 镜像 fallback + HF 适配层 `api`/`glob`/`resolve`），替代 `huggingface-cli` 解三个终端痛点（装 Python、国内镜像切换、整库下载）。但 crate 目前**未接入任何消费者**——模型下载仍走 hf-cli，落到 `~/.cache/huggingface/hub/`。

本阶段把 download crate 接入模型管理：模型下到 `~/.octopus/models/<repo>/`，ASR 的 `resolve_model_dir` 能发现并加载。

这是 4 阶段工程的第 1 阶段：

| 阶段 | 内容 | 本 spec |
|---|---|---|
| **① download 接模型管理** | 替换 hf-cli；模型下到 `~/.octopus/models/`；resolve 扩展；cli `download` 子命令 | ✓ |
| ② ort load-dynamic | asr 不再静态含 ort，运行期 dlopen | 后续 |
| ③ download 拉 ort 运行时 | download 增加拉 `libonnxruntime` 能力 | 后续 |
| ④ 分发打包 | 三 binary 共享 `~/.octopus/bin/libonnxruntime` | 后续 |

阶段1 与 ②③④ 正交，可独立交付。

## 2. 现状（已探明）

### 2.1 resolve_model_dir
`crates/asr/src/config.rs:65`，三级查找：
1. `octopus_config_home().join(source)`（`~/.octopus/` 下相对路径，随包小模型如 silero_vad）
2. 绝对路径（`source` 是绝对路径且存在）
3. `find_hf_cache(source)`：`~/.cache/huggingface/hub/models--<repo>/snapshots/<hash>/`

### 2.2 调用点
- **13+ 处**引擎调 `resolve_model_dir(&entry.source)`：whisper / sensevoice / paraformer / streaming_paraformer / streaming_engine / streaming_zipformer / moonshine / qwen3_asr / zipformer / engine，以及 cli（×5）。
- **3 处 `hf_snapshot` 测试辅助函数**（均在 `#[cfg(test)]` 内，定位测试快照目录，**非生产路径逻辑**）：
  - `streaming_paraformer.rs:797`
  - `zipformer.rs:1297`
  - `streaming_zipformer.rs:912`

  > **勘误**：初版误判为"绕过 resolve_model_dir 的重复路径异味"。实施前核实——3 处全在 `#[cfg(test)]` 的 `hf_snapshot` 辅助函数内（streaming_paraformer:792 / zipformer:1289 / streaming_zipformer:904），仅定位测试快照目录；生产代码这些引擎均已正确调 `resolve_model_dir`（streaming_paraformer:78 / zipformer:444,717 / streaming_zipformer:74,548），无重复路径逻辑。详见 §3.2。

### 2.3 目录约定
`infra/consts.rs` 已用 `~/.octopus/models/` 根：
- `SILERO_VAD_PATH = "models/silero_vad_v4.onnx"`
- `DEFAULT_ASR_MODEL_DIR = "models/zipformer"`

### 2.4 download crate 接口（复用，不改）
```rust
HfRequest { repo, include, exclude, source_url: Option<String>, target_dir: PathBuf }
resolve_tasks(&reqwest::Client, HfRequest) -> Result<Vec<DownloadTask>>
Downloader::new(DownloadConfig) -> Result<Downloader>
Downloader::download(&DownloadTask, mpsc::Sender<Progress>, Option<...>) -> Result
```
布局：`target_dir/<repo>/<files>`（integration 测试验证：`target_dir=dir`, `repo="org/m"` → `dir/org/m/model.onnx`）。

## 3. 设计

### 3.1 resolve_model_dir 扩展（config.rs:65）
在 HF cache 查找（原第 3 级）之前插入新一级：

```
1. ~/.octopus/<source>          （既有，随包小模型）
2. 绝对路径                      （既有）
3. ~/.octopus/models/<source>   （新增：download 下的 HF 模型）★
4. find_hf_cache(source)        （既有，兼容已用 hf-cli 下的模型）
```

**纯查找语义不变**——resolve 不发起网络请求、不下载，只查本地路径是否存在。新级放在 HF cache 之前，使 download 下的模型优先于旧 hf-cli 缓存。

### 3.2 ~~统一 3 处直接拼路径~~（取消：核实为测试辅助）

> **勘误**：§2.2 所列 3 处 `join(".cache/huggingface/hub")` 全在 `#[cfg(test)]` 的测试辅助函数 `hf_snapshot` 内，仅定位测试快照目录，**不是生产路径逻辑**。生产代码已正确调 `resolve_model_dir`，收拢测试辅助路径无收益、反增维护面，故本阶段**不动**。

### 3.3 显式下载，resolve 不透明触发（关键决策）
模型缺失时 resolve **报错**，错误信息提示：
> 模型 `<source>` 未在 `~/.octopus/models/` 或 HF cache 找到，请运行 `octopus-cli download <source>` 下载。

**不自动下载**。理由：
- resolve 保持纯查找语义（快、确定、本地 IO）；ASR 引擎加载时不会突然联网 / 卡住 / 因网络失败。
- 下载是显式、可观测、有进度的动作（cli 子命令 / 未来 GUI 按钮），符合 download crate 设计 + hf-cli 使用习惯。
- 错误边界清晰：resolve 失败 = 模型缺失；download 失败 = 网络/hash/镜像问题，两类不混淆。

### 3.4 cli 加 download 子命令
`crates/cli/src/main.rs` 的 `Commands` enum 加：
```rust
Download {
    /// HF repo，如 Systran/faster-whisper-large-v3（与 config.yaml 的 entry.source 一致）
    repo: String,
    /// 只下匹配的文件（glob，对齐 hf-cli，* 跨 /）。空 = 下整库
    #[arg(long)]
    include: Vec<String>,
    /// 排除匹配的文件
    #[arg(long)]
    exclude: Vec<String>,
    /// HF 镜像，如 hf-mirror.com。覆盖 config 默认
    #[arg(long)]
    mirror: Option<String>,
}
```
行为：薄封装 download crate——构 `HfRequest`（`target_dir=~/.octopus/models`，`repo`/`source_url` 由参数+config）→ `resolve_tasks` → 逐任务 `Downloader::download` → 打印进度 → 汇总结果。

- `crates/cli/Cargo.toml` 加 `octopus-download = { path = "../download" }`。
- download 是 async；cli 当前同步 main（仅 TranscribeUrl 用 tokio runtime）。沿用既有模式：Download 子命令建 `tokio::runtime::Runtime` + `block_on`。
- **include 默认行为**：`include` 空时下整库。实施时验证 `resolve_tasks` 对空 `include` 的语义（空 = 匹配全部 siblings），若不符则在 cli 层空时传通配 `*`。

### 3.5 镜像配置
优先级：`--mirror` 参数 > config.yaml 配置 > 默认 `huggingface.co`。
- `AppConfig` 新增扁平字段 `download_mirror`（非嵌套；空串 = 用官方源 `huggingface.co`）。如 `download_mirror: https://hf-mirror.com`。
- 解国内"每次切镜像"痛点：配一次，所有 download 复用；`--mirror` 临时覆盖。

### 3.6 目录布局
- download 的 `target_dir = ~/.octopus/models`（`octopus_config_home().join("models")`）。
- repo 作子目录：`~/.octopus/models/<repo>/<files>`，如 `~/.octopus/models/Systran/faster-whisper-large-v3/model.onnx`。
- 与 resolve_model_dir 第 3 级（`~/.octopus/models/<source>`）一致，与既有 silero/zipformer 同根。

## 4. 接口契约

| 接口 | 变化 |
|---|---|
| `resolve_model_dir(source)` 签名 | **不变**（`&str -> Result<PathBuf>`），内部加 1 级查找 |
| cli `Commands` enum | 新增 `Download` 变体 |
| download crate lib 接口 | **不改**（复用 `HfRequest`/`resolve_tasks`/`Downloader`） |
| `AppConfig` | 新增扁平字段 `download_mirror`（空 = 官方源 `huggingface.co`） |

## 5. 数据流

**下载**：`octopus-cli download <repo>` → 构 `HfRequest` → `resolve_tasks`（HF api 解析 siblings + glob 过滤 + 拼 resolve URL + 镜像）→ `Downloader::download`（probe/分块/并发/校验/rename/镜像 fallback）→ `~/.octopus/models/<repo>/`。

**加载**：ASR 引擎 `resolve_model_dir(&entry.source)` → 查 `~/.octopus/models/<source>` 命中 → 返回目录 → 引擎加载 onnx/tokenizer。

## 6. 错误处理

| 场景 | 行为 |
|---|---|
| resolve 模型缺失 | 报错 + 提示 `octopus-cli download <source>` |
| download 网络/镜像失败 | download crate 已有错误（`DownloadError`），cli 透传 + 镜像 fallback |
| hash 校验失败 | download crate 整文件重下（既有逻辑） |
| target_dir 不可写 | anyhow 透传 |

## 7. 范围边界（本阶段不做）

- **不加 DB models 表**：resolve 查文件系统即可；未来 GUI 模型管理页要列表/状态时再加（YAGNI）。
- **不动 ort**：阶段② 的 load-dynamic。
- **不做 GUI**：lib-first，desktop 消费（setting-ui2 若复活）留后续。
- **不删 HF cache 兼容**：resolve 第 4 级仍查 `~/.cache/huggingface`，兼容用户已用 hf-cli 下的模型。

## 8. 测试策略

- **resolve_model_dir 单测**（asr/config.rs 或 tests）：
  - `~/.octopus/models/<source>` 命中 → 返回该路径
  - 不在 models/ 但在 HF cache → fallback 返回 HF cache 路径
  - 都不在 → Err，信息含下载提示
- **cli download 集成测试**（httpmock，复用 download crate tests 模式）：
  - 单文件 resolve + download 成功
  - include glob 过滤
  - mirror fallback
- **引擎回归**：3 处路径统一后，跑现有 asr 引擎测试确认无回归。

## 9. 后续阶段（不属于本 spec）

- **② ort load-dynamic**：`asr/Cargo.toml` 的 ort 从 `download-binaries` 改 `load-dynamic`，初始化指向 dylib 路径（`~/.octopus/bin/`）。binary 各掉 ~20-35M 静态 ort。
- **③ download 拉 ort 运行时**：download 增加拉 `libonnxruntime` 能力（版本对齐 ort 2.0.0-rc.12、平台包 mac-universal2/linux-x64-gpu/win-x64、镜像 fallback）→ `~/.octopus/bin/`。
- **④ 分发打包**：三 binary 共享 `~/.octopus/bin/libonnxruntime`；发行包不含静态 ort；首次运行按需拉取。


---
## 2026-06-21-model-download-design

# 模型下载 crate 设计（octopus-download）

> **状态**：已批准（2026-06-21）。本 spec 是实现权威，后续 plan / 代码以此为准。
> **关联**：参考项目 `omniget`（`/Users/wudarui/workspace/agent/omniget`）、`mangofetch`（`/Users/wudarui/workspace/agent/mangofetch`），二者软链在主项目根，仅参考其 Rust 下载实现。

## Goal

新建一个**通用文件下载 crate**（`crates/download/`，包名 `octopus-download`），支持**分块并发 + 断点续传 + 完整性校验 + 镜像 fallback**。首要用途是替代 `huggingface-cli` 下载 ASR 大模型（解决终端用户门槛、国内镜像、按需选 `int8` 文件三个痛点），但 crate 本身不耦合 HuggingFace——HF 逻辑放在同 crate 的 `hf/` 模块，核心 `core/` 模块零 HF 知识。

## 背景与痛点

当前大模型（whisper / sensevoice / qwen3 / paraformer / zipformer-xlarge 等）下载方式（`docs/configuration.md:295-310`）：

```bash
pip install huggingface_hub
huggingface-cli download <repo>
```

三个痛点：
1. **终端用户门槛高**：要装 Python + pip + hf-cli 才能用大模型，对非专业用户不可接受。
2. **国内镜像**：`huggingface.co` 在国内访问墙/慢，hf-cli 需切 `HF_ENDPOINT=https://hf-mirror.com`，步骤易漏。
3. **整仓下载**：hf-cli 不加 `--include` 会拉整个 repo（含 `*_fp16.onnx`、`*_merged.onnx` 等不需要的文件），但实际只需 `int8` 量化文件。

本 crate 用 Rust 内置下载，参数化 `source-url`（镜像）+ `include/exclude` glob（选文件），断点续传 + 校验，无需 Python。

## 方案概述

**单 crate 两模块**：
- `src/core/`：通用下载器。输入 `(url, mirrors, dest, expected_hash)`，输出"文件下到 dest + 校验通过"。**不识 HF、不识 glob**。
- `src/hf/`：HF 适配层（依赖 core）。输入 `(repo, include, exclude, source_url, target_dir)`，调 HF API 列文件 → glob 过滤 → 构造 resolve URL + 提取 hash → 产出 `Vec<DownloadTask>` 交 core 下载。

**统一 segment 架构**（避免返工）：单文件 = 1 个 segment，是分块的退化。同一套代码路径（probe → 规划 segments → `set_len` 预分配 → 并发 Range+seek 写 → 进度汇总 → 校验 → rename）。单段时并发数=1，多段时并发加速。从单流"启用"分块只是改规划阈值。

**持久化两层分离**：
- 单文件断点续传：sidecar `<dest>.part.resume.json`（段级进度，和 `.part` 绑定，崩溃安全，下完即删）。**不进 sqlite**。
- 模型级管理（已下哪些、版本、校验状态）：属应用层集成（后续 task，扩 `models` 表），**不在本 MVP**。

## 数据流

以 `onnx-community/whisper-small.en` 为例：

```
输入: repo=onnx-community/whisper-small.en
      include=['*','onnx/*_int8.onnx']  exclude=['*/*','onnx/*_merged_int8.onnx']
      source_url=https://hf-mirror.com   target_dir=~/.octopus/models
   │
   ▼  [HF 适配层 src/hf/]
   1. GET {source_url}/api/models/{repo}
      → siblings[].rfilename + siblings[].etag + siblings[].lfs.oid(sha256, LFS 文件有)
   2. 对每个 rfilename 应用 include/exclude glob（对齐 hf-cli 语义）
      → 选中文件集
   3. 每个选中文件：
      - resolve URL = {source_url}/{repo}/resolve/main/{path}
      - mirrors     = [镜像URL, 官方URL(https://huggingface.co/...)]  # 镜像优先 fallback 官方
      - dest        = {target_dir}/{repo}/{path}
      - expected_hash = LFS 文件用 Sha256(lfs.oid)；非 LFS 小文件用 Etag
   │
   ▼  产出 Vec<DownloadTask>
   │
   ▼  [通用 core src/core/]
   逐文件 Downloader::download(task, progress_tx, cancel):
     1. probe: GET Range bytes=0-0（或 HEAD）→ total_size, accept_ranges, etag
        镜像 fallback：主源失败试 mirrors 下一个
     2. plan_segments(total, accept_ranges):
        - !accept_ranges || total 未知 || total < CHUNK_THRESHOLD → 1 段 [0, total)
        - else → N 段，每段 ~SEGMENT_SIZE，N = min(段数, MAX_CONCURRENT)
     3. load sidecar（若存在）: 三重校验 type/total_bytes/url_hash → 复用各段 downloaded；不符则丢弃重新规划
     4. ensure_part_file(dest.part): set_len(total) 预分配 sparse 文件
     5. 并发执行 segments（JoinSet + Semaphore，单段时并发=1）:
        each segment:
          - Range: bytes={begin+downloaded}-{end}
          - （不注入 If-Range：最终整文件 hash 校验兜底内容变更；注入反而让不支持它的镜像回退 200 全文重传）
          - seek(offset) + write，BufWriter 256KB
          - 段级重试（MAX_RETRIES_PER_SEGMENT，指数 backoff + jitter）
        progress pump: AtomicU64 fetch_add → 后台 task 250ms 推 mpsc::Sender<Progress>
        sidecar 回写: 段完成时（join_next）快照各段 downloaded，原子写（tmp+rename）
        cancel: CancellationToken 贯穿，取消时 abort 全部段
     6. 全部段 done → SHA256/etag 校验(expected_hash)
          - 失败：重试整文件下载 MAX_VERIFICATION_RETRIES 次，仍失败报 HashMismatch
     7. rename(dest.part → dest) + remove(sidecar)
```

## crate 结构

```
crates/download/
├── Cargo.toml
└── src/
    ├── lib.rs              # 导出 core + hf 公共 API
    ├── core/
    │   ├── mod.rs          # Downloader, DownloadTask, DownloadConfig
    │   ├── downloader.rs   # download() 主编排（probe→plan→并发→校验→rename）
    │   ├── segment.rs      # Segment 结构 + plan_segments
    │   ├── resume.rs       # sidecar 加载/保存/三重校验（原子写）
    │   ├── verify.rs       # If-Range 续传校验 + SHA256/etag 完整性校验
    │   ├── progress.rs     # Progress 结构 + mpsc + 节流
    │   └── error.rs        # DownloadError（thiserror）
    └── hf/
        ├── mod.rs          # HfRequest, resolve_tasks
        ├── api.rs          # GET /api/models/{repo} 解析 siblings
        ├── glob.rs         # include/exclude 过滤（对齐 hf-cli fnmatch）
        └── resolve.rs      # 构造 resolve URL + 镜像 + hash 提取
```

**核心模块（`core/`）不 import `hf/`**。`hf/` 依赖 `core/`。将来下别的源（非 HF）加新顶层模块即可。

## 核心 API

```rust
// ===== src/core/ =====

/// 期望校验值
#[derive(Debug, Clone)]
pub enum Hash {
    Sha256(String),   // hex
    Etag(String),
}

/// 单文件下载任务
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,              // 主源（通常是镜像）
    pub mirrors: Vec<String>,     // 备选源（含官方源），顺序 fallback
    pub dest: PathBuf,            // 最终落地路径
    pub expected_hash: Option<Hash>,
}

/// 下载器配置
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    pub connect_timeout: Duration,        // 默认 10s
    pub read_timeout: Duration,           // 默认 45s（单段无数据超时）
    pub segment_size: u64,                // 默认 4 MiB
    pub chunk_threshold: u64,             // 默认 16 MiB，小于此走单段
    pub max_concurrent: usize,            // 默认 8，clamp(1, 32)
    pub max_retries_per_segment: u32,     // 默认 3
    pub backoff_base: Duration,           // 默认 1s（指数：base * 2^attempt + jitter）
    pub max_verification_retries: u32,    // 默认 2（整文件校验失败重下次数）
    pub buf_kb: usize,                    // 默认 256
}
impl Default for DownloadConfig { /* 上述默认值 */ }

/// 进度上报（mpsc，不持久化）
#[derive(Debug, Clone)]
pub struct Progress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: Option<f64>,    // EMA 估算
}

/// 下载器
pub struct Downloader {
    client: reqwest::Client,       // rustls-tls, default-features=false
    config: DownloadConfig,
}
impl Downloader {
    pub fn new(config: DownloadConfig) -> Result<Self, DownloadError>;
    /// 下载单个 task。progress 实时推（250ms 节流）。cancel 可选。
    pub async fn download(
        &self,
        task: &DownloadTask,
        progress: tokio::sync::mpsc::Sender<Progress>,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<(), DownloadError>;
}

// ===== src/hf/ =====

pub struct HfRequest {
    pub repo: String,                      // 如 "onnx-community/whisper-small.en"
    pub include: Vec<String>,              // glob 模式，多个 = OR
    pub exclude: Vec<String>,              // glob 模式，多个 = OR，优先于 include
    pub source_url: Option<String>,        // 如 "https://hf-mirror.com"；None=官方源
    pub target_dir: PathBuf,               // 默认 ~/.octopus/models
}
/// 解析 HF 请求为下载任务列表（调 API + glob + 构造 URL/hash）
pub async fn resolve_tasks(
    client: &reqwest::Client,
    req: HfRequest,
) -> Result<Vec<DownloadTask>, DownloadError>;
```

## 断点续传（sidecar）

文件 `<dest>.part.resume.json`，格式：

```json
{
  "type": "octopus-segmented",
  "url_hash": "<sha256(dest 路径) 前 16 hex，镜像无关>",
  "total_bytes": 12345678,
  "etag": "<probe etag，当前未注入 If-Range、靠 hash 兜底；保留字段供未来启用>",
  "segments": [
    {"begin": 0, "end": 4194303, "downloaded": 4194304},
    {"begin": 4194304, "end": 8388607, "downloaded": 1000000}
  ]
}
```

- **加载时三重校验**（任一不符即丢弃 sidecar、重新规划）：`type == "octopus-segmented"` && `total_bytes == probe 总长` && `url_hash == sha256(dest 路径)`。`url_hash` 基于 **dest 路径**而非 url——故换镜像（dest 不变）不触发重下，仅换目录/目标文件才失效（符合预期）。另：多段 sidecar 遇不支持 Range 的源（`accept_ranges=false`）会丢弃重规划为单段——否则会向不支持 Range 的服务器发分段请求、注定 200 全文错位。
- **原子写**：先写 `<dest>.part.resume.json.tmp` 再 `rename` 覆盖，崩溃时不留半截 JSON。
- **节奏**：段完成时（`download_chunked` 的 `join_next`）快照各段 `downloaded` 写一次（非独立定时 pump）。
- **清理**：下载成功 `rename(.part→dest)` 后 `remove_file(sidecar)`；致命错误（4xx）删 `.part` + sidecar；瞬时错误（5xx/超时）保留 `.part` + sidecar 待续传。
- **单段也记 sidecar**：保持架构统一（单段 = segments.len()==1 的特例），续传逻辑一套。

## 分块机制

- **规划**（`plan_segments`）：`accept_ranges && total >= chunk_threshold` → 多段；否则 1 段。段数 `N = min(total.div_ceil(segment_size), max_concurrent)`，余数逐段均摊。
- **预分配**：`ensure_part_file` 用 `File::set_len(total)` 打 sparse 洞，各段 `seek(begin + downloaded) + write` 直写最终位置——**无需下载完再合并**。
- **并发**：`tokio::task::JoinSet` + `Arc<Semaphore::new(max_concurrent)>`。每段任务 acquire 后才发请求。
- **进度汇总**：`Arc<AtomicU64>`，每段写一段 `fetch_add`，后台 pump 250ms 读总值推 mpsc + 算 EMA 速度。
- **段级重试**：每段独立重试 `max_retries_per_segment` 次，指数 backoff（`backoff_base * 2^attempt`）+ jitter。段失败回滚该段已计入的进度（减去 downloaded）。
- **响应判定**：`206` → 续写；`416` → 删该段进度从头；`200`（服务端忽略 Range）→ 当单流处理（truncate 重写该段）；其余按错误分类。
- **work-stealing 不做**（YAGNI）：模型源带宽稳定，静态分段 + 段级重试足够。

## 校验（补两参考项目的漏）

两参考项目都**没用 `If-Range`/`ETag`** 校验续传有效性，只靠 `content-length`/`url_hash`——同 URL 内容被替换会产出损坏文件。本设计补上：

1. **续传有效性**——**最终实现选择不注入 `If-Range`**（初稿设计为带 If-Range etag，实现时改）。理由：注入 If-Range 让不支持它的服务器 / 镜像回退 `200` 全文重传，得不偿失；续传正确性改由下方整文件 SHA256 校验兜底——断点后服务端内容若变更，写到 `.part` 的旧区段会被最终 hash 校验抓住并触发整文件重下（`max_verification_retries`）。sidecar 的 `etag` 字段保留供未来按需启用，当前不参与续传判定。
2. **完整性**（SHA256/etag）：全段完成后，LFS 文件算 SHA256 比 `expected_hash=Sha256(lfs.oid)`；非 LFS 小文件比 `Etag`。`spawn_blocking` 流式 hash（8KB buffer，避免阻塞 runtime）。
3. **失败处理**：校验失败重下整文件 `max_verification_retries` 次，仍失败报 `HashMismatch`（删 `.part` + sidecar）。

## 镜像（source-url）

- HF 适配层接收 `source_url`（如 `https://hf-mirror.com`），用它替换官方域名生成镜像 URL。
- `task.mirrors = [镜像URL, 官方URL]`：镜像优先，失败 fallback 官方源。
- list API（`/api/models/{repo}`）也走 `source_url`（镜像需代理 `/api`，hf-mirror 支持）。
- core 层 `download()` 主源（`task.url`，通常是镜像）失败 → 依次试 `task.mirrors`。

## glob（对齐 hf-cli，关键风险点）

- hf-cli 的 `--include`/`--exclude` 用 Python `fnmatch`（Unix shell 风格：`*` `?` `[...]`）。
- 语义：多个 include = **任一匹配则包含（OR）**；多个 exclude = **任一匹配则排除（OR）**；**exclude 优先于 include**（先 include 选出，再 exclude 剔除）。
- path 相对 repo 根（如 `onnx/model_int8.onnx`）。
- **Rust 实现**：用 `glob` crate 或手写 fnmatch，但**必须与 hf-cli 实测对齐**（尤其 `*` 是否跨 `/`、`[...]` 字符类）。
- **测试**：用真实 `huggingface-cli download <repo> --include ... --exclude ... --dry-run`（hf-cli 支持 dry-run 列出将下载的文件）输出做 golden test，确保同样参数选出相同文件集。

## 目录布局

- `{target_dir}/{repo}/{path}`，`repo` 中的 `/` 作路径分隔。
- 默认 `~/.octopus/models/onnx-community/whisper-small.en/onnx/model_int8.onnx`。
- 复刻 repo 结构，多 repo 不冲突，路径有意义。
- MVP **不做 commit pinning**：直接 `resolve/main/{path}`（最新），校验靠 etag/sha256。版本管理（pin 特定 commit）后续。

## 错误类型

```rust
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("fatal: HTTP {status} for {url}")]
    Fatal { status: u16, url: String },
    #[error("transient ({kind}): {message}")]
    Transient { kind: TransientKind, message: String },
    #[error("cancelled")]
    Cancelled,
    #[error("hash mismatch for {path}: expected {expected}, got {actual}")]
    HashMismatch { path: PathBuf, expected: String, actual: String },
    #[error("hf api error: HTTP {status} for {url}")]
    HfApi { status: u16, url: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
}

pub enum TransientKind {
    ServerError,   // 5xx
    RateLimited,   // 429
    Timeout,       // read/connect timeout
    Network,       // connection reset, dns, etc.
}
```

- **Fatal**（4xx 除 408/429）：不重试，删 `.part` + sidecar。
- **Transient**（5xx / 408 / 429 / 超时 / 网络）：重试 + 指数 backoff + jitter。
- **Cancelled**：CancellationToken 触发，停止。
- 不用参考项目的"字符串匹配 error message"分类法——按 `StatusCode` + `io::ErrorKind` 分类。

## 依赖（Cargo.toml）

```toml
[dependencies]
reqwest = { version = "0.12", default-features = false,
            features = ["stream", "http2", "rustls-tls", "json"] }
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["rt"] }          # CancellationToken
futures = "0.3"
sha2 = "0.10"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
glob = "0.3"                                                  # 实际手写 fnmatch（见 hf/glob.rs），此依赖未使用，待清理
log = "0.4"                                                   # workspace 用 log，非 tracing
anyhow = "1"

[dev-dependencies]
httpmock = "0.8"                                              # mock HTTP，对齐参考项目（mangofetch 0.8.3）
tokio = { version = "1", features = ["full", "test-util"] }
```

> 注：具体版本号在 plan 阶段对齐 workspace 现有（`cargo tree` / 各 crate Cargo.toml），避免引入重复版本。

## 测试策略

**core**（httpmock mock 服务器）：
- 单段（小文件）下载成功
- 多段（大文件）分块并发下载成功
- 断点续传：模拟中断（保留 `.part` + sidecar），重启后从段进度恢复
- 续传兜底：整文件 SHA256 校验失败 → 整文件重下（不注入 If-Range，靠 hash 兜底）
- SHA256 校验：成功 / 失败（mock 错误内容）/ 失败重试
- 镜像 fallback：主源 500，镜像 200
- 取消：CancellationToken 触发后停止
- 错误分类：4xx→Fatal、5xx→Transient、超时→Transient
- sidecar 三重校验：total 不符 / url_hash 不符 → 丢弃重新规划

**hf**：
- glob 对齐 hf-cli：**golden test**，用真实 `huggingface-cli download <repo> --include ... --exclude ... --dry-run` 输出做期望（`include`/`exclude` 多组合）
- resolve URL 构造：镜像域名替换正确
- API 解析：mock `/api/models/{repo}` 返回 siblings，正确提取 rfilename / etag / lfs.oid

> glob golden test 的期望文件生成：`huggingface-cli download onnx-community/whisper-small.en --include '*' 'onnx/*_int8.onnx' --exclude '*/*' 'onnx/*_merged_int8.onnx' --dry-run`（需 Python 环境，仅生成期望时用，非 crate 运行时依赖）。

## MVP 边界（不含）

- **CLI**：先只做 lib。CLI 形态（独立 binary / `octopus-cli` 子命令）后续讨论。
- **sqlite 管理**：不建表。模型级管理（已下哪些、版本、校验状态）属应用层，后续与 `resolve_model_dir` 集成一起做。
- **`resolve_model_dir` 扩展**：现有只查 `~/.cache/huggingface/hub/`，需扩展支持 `~/.octopus/models/` + DB `models` 表 source 处理，才能让下载的模型被加载。**这是紧接的后续 task**（下载 lib 本身不依赖它）。
- **work-stealing**：YAGNI。
- **commit 版本 pinning**：直接 `resolve/main/`，后续。

## 后续工作（spec 外，记录待办）

1. CLI 形态设计（独立 binary vs `octopus-cli download model` 子命令）。
2. 应用层集成：`resolve_model_dir` 扩展支持 `~/.octopus/models/` + `models` 表（local_path / commit / verified / 文件清单）。
3. 下载任务队列 / 历史 / GUI 显示（如需要，sqlite 应用层）。
4. 可能的 work-stealing（仅当实测出现"某段卡死拖慢全局"才加）。
5. commit 版本 pinning（若需固定模型版本）。

## 关键设计决策记录（权衡动机）

- **统一 segment 架构 vs 先单流后分块**：选统一架构。单流 = 1 segment 退化，后续无返工。返工的唯一来源是"单流用 append、分块用 set_len+seek"写法不一——本设计单段也用 `set_len+seek` + sidecar，彻底消除。
- **sidecar vs sqlite 存进度**：选 sidecar。与 `.part` 强绑定、崩溃安全、下完即删、保持 crate 通用（无 sqlite 依赖）。sqlite 留给应用层模型管理。
- **`If-Range`/ETag 续传校验**：两参考项目都漏，本设计补上——HF 模型可能重传同名文件，必须防内容被换。
- **类型化错误 vs 字符串匹配**：两参考项目用 `anyhow.to_string().contains("HTTP 4xx")`（脆弱），本设计用 `thiserror` enum 按 `StatusCode`/`ErrorKind` 分类。
- **纯通用 core + HF 适配层分离**：呼应"不限于此"。core 零 HF 知识，将来下别的源加模块即可。


---
## 2026-06-21-model-management-gui-design

# 模型管理 GUI 接入设计（desktop 设置窗口页面 3）

> 2026-06-21 初版（已合并 main `7fd0682`）。2026-06-22 **就绪逻辑重构 v2**（见 §9）：`is_enabled` 改为「就绪」语义、点下载先探查、`secret_key` 存文件清单 + sha256 自举、新增完整性复核。
> 相关：download crate spec `2026-06-21-model-download-design.md`、阶段1 接入 spec `2026-06-21-download-model-integration-design.md`。
> worktree：`model-mgmt-ui`（分支 `worktree-model-mgmt-ui`）。

## 1. 背景与定位

阶段1（merge `f6f02bb`）已交付下载能力的「后端三层」：cli `download` 子命令、`resolve_model_dir` 第3级 `~/.octopus/models/<source>`、`AppConfig.download_mirror`。v1（merge `7fd0682`）把下载接到了 GUI：设置窗口「模型管理」页列出可下载的本地 ASR 模型，点按钮即下载到 `~/.octopus/models/<repo>/`，实时进度推前端。

**v2 重构动机**：v1 的「已就绪」= `resolve_model_dir().is_ok()`（文件在任意路径即算），且用 `list_engines()` 取列表——但 `list_engines` 走 `load_config`，后者在 DB 加载层硬过滤 `is_enabled=1`，导致 seed 里 `is_enabled=0` 的可下载模型**根本列不出来**。v2 改为：直读 DB 列全部本地模型，`is_enabled` 作为「就绪」标志，点下载先探查文件（命中即就绪、置 true，不重下），并对已就绪模型做 sha256 完整性复核（损坏置 false）。

**与 `setting-ui2` 分支的关系**：setting-ui2 在做设置窗口其他 UI（非模型管理页），也会改 `dist/settings/index.html`。本工作的前端 JS 隔离到独立 `models.js`，使对 `index.html` 的改动压缩到两处局部编辑，合并冲突最小化。

## 2. 现状（已探明）

### 2.1 设置窗口结构
- Tauri webview，加载 `dist/settings/index.html`（**手写单文件**，无前端构建步骤）。
- 侧边栏 4 页：`history`（识别记录）/ `settings`（系统设置）/ `models`（模型管理）/ `prompts`（提示词，setting-ui2 的）。
- `switchPage(name)` 切 `.active`；前端用 `window.__TAURI__.core.invoke` 调命令、`window.__TAURI__.event.listen` 听事件。
- 模型管理页 `#page-models` 顶部第一张卡片即「下载镜像」（镜像输入），第二张「ASR 模型」（`#models-list` 由 `models.js` 填充）。

### 2.2 后端命令
`get_config` / `set_config`（含 `download_mirror` 字段）/ `get_history` / `test_llm_connection` / `test_asr_connection` 等。模型管理命令在独立模块 `model_commands.rs`（v1）：`list_downloadable_models` / `download_model` / `set_download_mirror`。v2 新增 `verify_model`。

### 2.3 引擎 DB = 天然下载目录（关键利好）
`infra/src/db.sql` 的 models 表 seed：本地 ASR（is_local=1，12 行）每个引擎的 `source` 即 HF repo（如 `csukuangfj/sherpa-onnx-streaming-paraformer-zh`），`source` ↔ cli `download <repo>` ↔ `resolve_model_dir` 三者对齐。**默认/兜底引擎 `zipformer-small-ctc`（随包打包，本地路径 `models/zipformer`）由代码写死（`asr/config.rs` `FALLBACK_ASR_ENGINE_NAME`），不在 seed/DB 中**——`app_config.asr_engine` 空/不匹配时 `fallback_engine` 硬构造，开箱可用、不依赖本表。

### 2.4 `is_enabled` 过滤发生在 DB 加载层（v2 关键发现）
`load_models_at`（`infra/src/db.rs:396`）SQL 硬编码 `WHERE domain='asr' AND is_enabled = 1`——**`is_enabled=0` 的模型不进 `AsrConfig`、不进 `list_engines()`**。seed 里 local 可下载模型 `is_enabled` 全 0，所以 v1 用 `list_engines()` 列表时只能看到用户手编置 1 的。v2 因此改为**直读 DB（不过滤 is_enabled）**列全部。`ModelEntry` 已含 `is_enabled` 字段。

### 2.5 `RUNTIME_CONFIG` 不可刷新（v2 改造点）
`asr/config.rs:16` `static RUNTIME_CONFIG: OnceLock<AsrConfig>`，注释「手编 DB models 表后需重启进程生效」——`OnceLock` 不能 reset。v2 改为可刷新的 `RwLock<Option<Arc<AsrConfig>>>` + `reload_models_config()`（对齐既有 `reload_app_config` / `APP_CONFIG` 模式，审查二1 已验证），让「改 `is_enabled` 后引擎下拉立即更新」。

### 2.6 download crate 公开 API（复用，不改）
```rust
HfRequest { repo, include, exclude, source_url: Option<String>, target_dir: PathBuf }
resolve_tasks(&reqwest::Client, HfRequest) -> Result<Vec<DownloadTask>>
Downloader::new(DownloadConfig) -> Result<Downloader>          // .client() 借内部 reqwest::Client
Downloader::download(&DownloadTask, mpsc::Sender<Progress>, None) -> Result
Progress { downloaded_bytes: u64, total_bytes: Option<u64>, speed_bps: Option<f64> }
```
download crate 下载时已做服务端 sha256 校验（阶段1），下载成功的文件即可信——v2 自举算 sha256 以此为信任基础。

## 3. 设计

### 3.1 后端模块 `crates/desktop/src/model_commands.rs`
独立模块（不动 `settings_commands.rs`，降低与 setting-ui2 冲突面）。命令：`list_downloadable_models` / `download_model` / `verify_model` / `set_download_mirror`。

**`list_downloadable_models()` → `Vec<DownloadableModel>`**
- 直读 `list_all_local_asr_models()`（infra 新函数，`domain='asr' AND is_local=1`，**不过滤 is_enabled**），不再走 `list_engines()`/`load_config`。
- `is_hf_repo(&source)` 过滤（排除随包 `models/`、绝对路径、云端 `http`/`wss`）。
- 返回 `{ name, repo, category, description, is_enabled }`（v2：`downloaded` 字段 → `is_enabled`）。

**`download_model(repo, rc, app_handle) async`**（v2：先探查）
1. `resolve_model_dir(&repo)` 探查三个路径（`~/.octopus/<source>`、`~/.octopus/models/<source>`、HF cache）。
2. **命中**（文件已就绪，如用户 hf-cli 下过的在 cache）→ 自举：遍历目录常规文件算 sha256 写 `secret_key`（`set_model_secret_key`）+ 置 `is_enabled=true`（`set_model_enabled`）+ `reload_models_config()`；emit `download-done {repo, already_ready:true}`；**不重下**。
3. **未命中** → 镜像 `rc.download_mirror`（空=官方源）+ `target_dir=~/.octopus/models` + `HfRequest` + `resolve_tasks`（用 `Downloader::client()`）；mpsc 进度转发 task emit `download-progress`/`download-file`；逐文件 `Downloader::download`；全部完成 → 自举算 sha256 写 `secret_key` + 置 `is_enabled=true` + `reload_models_config()`；emit `download-done {repo, already_ready:false}`。
4. 失败透传 `DownloadError`。

**`verify_model(model_name, repo)`**（v2 新增，完整性复核）
1. 读 `secret_key` JSON 清单。
2. **清单空** → 自举生成（遍历 `resolve_model_dir` 目录文件算 sha256 写回）+ 确保 `is_enabled=true` + reload；返回「已生成清单，就绪」。
3. **清单非空** → 逐文件算当前 sha256 比对（缺文件/hash 不符即损坏）。
   - 全匹配 → 确保 `is_enabled=true`；返回「校验通过」。
   - 任一不符 → 置 `is_enabled=false` + `reload_models_config()`；返回损坏文件清单。

**`set_download_mirror(value, rc)`**：v1 不变（独立命令，写 rc + `save_app_config`）。

### 3.2 前端独立 `dist/settings/models.js`
- `renderModels()`：`invoke('list_downloadable_models')` → 卡片列表。`is_enabled=true` → 「✓ 已就绪」+「重新校验」按钮；`false` → 「下载」按钮。
- 下载按钮：`invoke('download_model', {repo})`；`listen('download-file')` 显示「文件 i/total」+ `listen('download-progress')` 更新当前文件进度条；`listen('download-done')` → toast（已就绪/下载完成）+ 重新 `renderModels`。
- 「重新校验」按钮：`invoke('verify_model', {model_name, repo})` → toast 结果 + `renderModels`。
- 镜像输入框（顶部卡片）：读 `get_config().config.download_mirror` 回填；`change` → `invoke('set_download_mirror', {value})`。
- `window.initModelsPage` 挂全局，由导航点击或页面加载调用。

### 3.3 `index.html` 两处局部改动（v1 已落地，v2 不动结构）
1. `#page-models` 占位 → 顶部「下载镜像」卡片 + 「ASR 模型」卡片（`<div id="models-list">`）。
2. `</body>` 前加 `<script src="models.js"></script>`。

### 3.4 接线
- `Cargo.toml`：`octopus-download = { path = "../download" }`（v1 已加）。
- `main.rs`：`mod model_commands;` + invoke_handler 注册 `list_downloadable_models` / `download_model` / `verify_model` / `set_download_mirror`。

## 4. 接口契约

| 接口 | 变化 |
|---|---|
| download crate | **不改** |
| `resolve_model_dir` | **不改**（只读探查） |
| `infra::db` | **新增** `list_all_local_asr_models` / `set_model_enabled` / `set_model_secret_key`（+ `_at` 变体） |
| `asr::config` | **新增** `reload_models_config`；`RUNTIME_CONFIG` OnceLock→RwLock |
| desktop `invoke_handler` | 新增 `verify_model`（list/download/mirror v1 已注册） |
| Tauri 事件 | 新增 `download-done`（v2）；保留 `download-progress`/`download-file` |
| `DownloadableModel` DTO | `downloaded: bool` → `is_enabled: bool` |

## 5. 数据流

**列模型**：`models.js renderModels` → `list_downloadable_models` → `list_all_local_asr_models`（直读 DB，不过滤）+ `is_hf_repo` 过滤 → 卡片（按 `is_enabled` 显示就绪/下载）。

**下载**：点按钮 → `download_model(repo)` → 探查 `resolve_model_dir`：
- 命中 → 自举 sha256 写 `secret_key` + 置 `is_enabled=true` + reload → `download-done{already_ready:true}` → 刷新（就绪）。
- 未命中 → `resolve_tasks` → 逐文件 `Downloader::download` → mpsc → emit `download-progress`/`download-file` → 完成 → 自举 + 置 true + reload → `download-done{already_ready:false}` → 刷新（就绪）。

**重新校验**：点按钮 → `verify_model` → 读 `secret_key` 清单 → 复核 sha256 → 通过(确保 true)/损坏(置 false + reload) → toast + 刷新。

## 6. 错误处理

| 场景 | 行为 |
|---|---|
| `list_all_local_asr_models`/读 DB 失败 | 命令返回 Err，前端 toast |
| 探查命中但自举算 sha256 失败（目录权限/IO） | `download_model` 返回 Err，toast；不置 is_enabled |
| `resolve_tasks` 失败（仓库不存在/网络） | `download_model` 返回 Err，toast |
| 单文件下载失败 | 透传 `DownloadError`，镜像 fallback 由 download crate 处理 |
| `verify_model` 发现损坏 | 置 `is_enabled=false` + reload，返回损坏清单，toast |
| `secret_key` JSON 损坏（无法解析） | 视为「清单空」→ 自举重新生成 |

## 7. 范围边界（不做）

- **不增删改模型条目**：本地 ASR 模型清单是应用限定的（开发适配过的），列表只读，只能下载/校验/切 `is_enabled`。
- **不删除模型文件 / 在文件夹中显示**（YAGNI）。
- **不下载取消**（download crate 暂无取消 API）。
- **不并发多模型下载**（一次一个）。
- **云端模型不在此页**：火山/腾讯/百度/阿里等云端 ASR 走「系统设置」填 key + 连接测试，另套管理，与本页无关。

## 8. 测试策略

- **`is_hf_repo` 单测**（v1 已有）：随包/绝对路径/云端/空/真实 repo。
- **`list_all_local_asr_models` 单测**（infra）：含 `is_enabled=0` 的也被列出；`secret_key`/`is_enabled` 字段正确。
- **`set_model_enabled_at` / `set_model_secret_key_at` 单测**（infra）：写入后重读生效。
- **`RUNTIME_CONFIG` reload 单测**（asr）：改 DB 后 `reload_models_config()` → `load_config()` 返回新值。
- **`verify_model` 清单比对逻辑**：纯逻辑抽函数测（清单 parse + sha256 比对判定），文件系统部分靠手动。
- 前端 / Tauri 集成无自动化（webview + 网络），靠 `cargo check --workspace --all-targets` + clippy + 手动。

## 9. 就绪逻辑重构 v2 详述（2026-06-22）

### 9.1 `is_enabled` 语义
`is_enabled` 统一表达「该模型文件是否就绪可用」：`true`=文件完备可被引擎加载，`false`=未就绪。引擎下拉（`list_engines`→`load_config`）只收 `is_enabled=1` 是 load 层既有行为，自然联动——**未就绪的模型不会出现在「系统设置」的引擎下拉**，避免选了下载不全的模型。seed 里本地 ASR 初始**全部 `is_enabled=0`**（未就绪，待下载）；**默认引擎 `zipformer-small-ctc` 不占 seed 行**（代码写死、随包打包），首次启动靠代码兜底即开箱可用，其余引擎用户在本页下载后置 true。（2026-06-22：seed 本地 ASR 已按实时 DB is_local=1 的 12 行重生成，旧随包 `zipformer-small-ctc` 等移出 seed；`app_config.asr_engine` seed 改空=代码兜底。）

### 9.2 `RUNTIME_CONFIG` 可刷新化
- `static RUNTIME_CONFIG: OnceLock<AsrConfig>` → `static RUNTIME_CONFIG: RwLock<Option<Arc<AsrConfig>>>`（对齐 `APP_CONFIG`）。
- `load_config()`：读 `RwLock`；首次空则 `ensure_db` + `load_models` + 写入。13+ 调用点行为不变（读法从 `OnceLock::get` 换 `RwLock::read`，clone 成本不变）。
- `reload_models_config()`：从 DB 重读 `AsrConfig` 替换缓存（对齐 `reload_app_config`）。`download_model`/`verify_model` 改 `is_enabled` 后调用，让引擎下拉即时更新。

### 9.3 `secret_key` 复用与 JSON schema
local 模型 `secret_key`（DB 默认 `''`，原仅 api 模型用）重载为「文件清单 + sha256」JSON；api 模型（`is_local=0`）`secret_key` 仍是真 API key，**按 `is_local` 分支，不冲突**。schema（**path 为 key 的 map**，紧凑可读；`BTreeMap` 保证字母序、diff 友好）：
```json
{"model.onnx":{"sha256":"<hex>","size":12345}, "tokens.txt":{"sha256":"<hex>","size":75756}}
```
key 为相对模型目录根（`resolve_model_dir` 返回目录）的路径。读取时 JSON 解析失败→视为清单空→自举重建。manifest 逻辑（`bootstrap_manifest`/`verify_against_manifest`/`Manifest`）下沉到 `asr::manifest`，desktop（`download_model`/`verify_model`）与 cli（`sync-models`）共用。

**批量预填**：cli `octopus-cli sync-models` 扫描所有本地 ASR 模型，就绪的（`resolve_model_dir` 命中）自举清单写 `secret_key` + 置 `is_enabled=true`，未就绪置 `false`，末尾 `reload_models_config`——供首次填充或批量复核（GUI 的 `verify_model` 是单模型按需触发）。

### 9.4 自举（manifest 生成）
触发时机：① 下载完成；② 探查命中（已有文件，如 hf-cache）；③ `verify_model` 发现清单空。
做法：遍历 `resolve_model_dir` 返回目录下的**常规文件**（递归），算 sha256 + 相对路径 + 字节数，写 `secret_key`。HF cache snapshot 目录下是 symlink 到 blobs——按**实际文件内容**算 hash（follow link 读字节）。跳过隐藏/系统文件（`.DS_Store` 等）。

### 9.5 校验算法 = sha256
与 download crate（阶段1 服务端 sha256 校验）同一套，自举/复核一致，不引入 md5 第二套。用户原话「md5 对不上」理解为「校验码对不上」泛指。

### 9.6 校验时机（性能）
- **列表展示**：只按 `is_enabled` 显示，**不算 hash**（快，进页面即时）。
- **sha256 校验**：仅在「点下载探查时」（自举/复核）和「重新校验按钮」触发。大模型（1G+）算 sha256 几百 ms~秒级，不在列表每次跑。
- 「重新校验」按钮供用户怀疑文件损坏时手动触发全量复核。


---
## 2026-06-21-moonshine-asr-design

# Moonshine ASR 引擎接入设计

**日期**: 2026-06-21
**状态**: ✅ 已实现并合并 main。`greedy_decode` 全程零拷贝——KV cache 用 owned `Value` 复用、logits argmax 走 `try_extract_tensor` 的零拷贝 `&[f32]`、encoder_out 保留 owned `Value`（详见 §3 张量零拷贝管理）；moonshine en-only 经 `transcribe_with_vad` 的 `language=en` 自动跳过通用中文 corrector（不靠每引擎手动覆盖 `skip_corrector()`）。
**分支**: `feature/setting-ui2`

## 背景

项目需要接入 [Moonshine](https://github.com/moonshine-ai/moonshine) 语音识别模型——Useful Sensors 开发的端侧 ASR，专为低延迟优化。已通过 `csukuangfj/sherpa-onnx-moonshine-{base,tiny}-en-int8` 下载到 HF 缓存。

与 Whisper 相比：模型更小（tiny 26M / base 58M）、无需 30s padding、macOS arm64 上延迟更低（tiny ~34ms vs whisper ~277ms）。

## 模型架构

Moonshine 是**纯 ONNX 体系**的 encoder-decoder Transformer，与项目现有 `ort` 依赖完全契合，无需引入新框架。

### 4 个 ONNX session（v1 格式）

| Session | 输入 | 输出 | 作用 |
|---------|------|------|------|
| `preprocess.onnx` | `audio (1, N)` f32 | `features (1, T, 416)` | 学习型 conv 前端（替代手写 Mel），下采样率 384× |
| `encode.int8.onnx` | `features (1, T, 416)` + `features_len (1,)` i32 | `encoder_out (1, T, 416)` | Transformer encoder |
| `uncached_decode.int8.onnx` | `token (1, L)` i32 + `encoder_out` + `seq_len (1,)` i32 | `logits (1, 1, 32768)` + **N 个 KV cache 张量**（base=32，由模型层数决定） | 首个 token 解码（初始化 KV cache） |
| `cached_decode.int8.onnx` | `token (1, L)` + `encoder_out` + `seq_len` + **N 个 KV cache 张量** | `logits (1, 1, 32768)` + N 个新 KV cache 张量 | 后续 token 解码（复用 KV cache） |

- **N 个 cache 张量** = (层数 × 2)（K, V 各一），数量运行时从 `uncached_decode` 输出数动态获取（`(outputs-1)/1`，减去 logits）。base 模型 = 32 个（16 层 × 2），spec 初版误记为 36
- **vocab 32768**：byte-level BPE（与 Llama 1/2 兼容），`tokens.txt` 格式（token_text + tab + token_id）
- **特殊 token**：`<unk>=0`, `<s>=1`(BOS), `</s>=2`(EOS)

### Decode 循环（sherpa-onnx `offline-moonshine-greedy-search-decoder.cc` 参考）

```
BOS(1) → uncached_decode → logits + N cache
         ↓ argmax → token_0
         (EOS? stop)
token_0 → cached_decode(prev_cache) → logits + N new cache
         ↓ argmax → token_1
         (EOS? stop)
... 循环至 EOS(2) 或 max_len
```

`max_len = audio_seconds * 6`（语音每秒约 6 个 token 上限）。

### 文件布局

```
~/.cache/huggingface/hub/models--csukuangfj--sherpa-onnx-moonshine-base-en-int8/snapshots/<hash>/
├── preprocess.onnx
├── encode.int8.onnx
├── uncached_decode.int8.onnx
├── cached_decode.int8.onnx
├── tokens.txt              # 32768 行，格式: "token_text\ttoken_id"
└── test_wavs/
```

## 设计

### 1. 新增 `EngineCategory::Moonshine`

**`crates/asr/src/config.rs`**：

```rust
pub enum EngineCategory {
    Whisper,
    SenseVoice,
    Paraformer,
    Qwen3Asr,
    Zipformer,
    Moonshine,  // ← 新增
    Aliyun,
}
```

映射函数（4 处）：
- `engine_category_from_str`: `"moonshine" => Some(Moonshine)`
- `category_label`: `Moonshine => "moonshine"`
- `all_sections`: 新增 `(cfg.asr.moonshine.as_ref(), Moonshine)`
- `pick_entry`: `Moonshine => cfg.asr.moonshine.as_ref()`

### 2. 新增 `AsrSection.moonshine` 字段

**`crates/infra/src/db.rs`**：

```rust
pub struct AsrSection {
    pub whisper: Option<HashMap<String, ModelEntry>>,
    // ... existing ...
    #[serde(default)]
    pub moonshine: Option<HashMap<String, ModelEntry>>,  // ← 新增
    pub aliyun: Option<HashMap<String, ModelEntry>>,
}
```

`load_asr_config` 的 category 映射追加 `(_, "moonshine") => &mut asr.moonshine`。

### 3. 新建 `crates/asr/src/moonshine.rs`

实现 `OfflineAsrEngine` trait。

```rust
pub struct MoonshineEngine {
    preprocess_session: Session,
    encode_session: Session,
    uncached_decode_session: Session,
    cached_decode_session: Session,
    vocab: Vec<String>,           // tokens.txt 加载
    // Session 是 Send+Sync（ort 保证），无需 Mutex 包裹
}
```

**`new(entry: &ModelEntry)`**：
1. `resolve_model_dir(&entry.source)` 定位 HF 缓存目录
2. 加载 4 个 ONNX session（preprocess / encode / uncached_decode / cached_decode）
3. 加载 `tokens.txt` 为 `Vec<String>`（32768 项）

**`transcribe(&self, samples: &[f32], _language: &str) -> Result<String>`**：

```rust
fn transcribe(&self, samples: &[f32], _language: &str) -> Result<String> {
    // 1. preprocess: audio (1, N) → features (1, T, 416)
    let features = self.run_preprocess(samples)?;
    let features_len = features.shape()[1] as i32;

    // 2. encode: features → encoder_out (1, T, 416)
    let encoder_out = self.run_encode(features, features_len)?;

    // 3. decode loop (greedy)
    let max_len = (samples.len() as f32 / 16000.0 * 6.0) as i32;
    let token_ids = self.greedy_decode(&encoder_out, features_len, max_len)?;

    // 4. tokens → text
    Ok(decode_tokens(&token_ids, &self.vocab))
}
```

#### greedy_decode 内部逻辑

```
token = [1]  // BOS
seq_len = [1]

// 首 token: uncached_decode
(logits, kv_caches) = uncached_decode(token, encoder_out, seq_len)

loop:
    next_token = argmax(logits)
    if next_token == EOS(2): break
    tokens.push(next_token)
    seq_len += 1

    // 后续 token: cached_decode
    (logits, kv_caches) = cached_decode([next_token], encoder_out, seq_len, kv_caches)
```

#### 张量零拷贝 + 热路径无分配

`preprocess → encode → greedy_decode` 全程不让张量离开 ORT 内部，decode 循环也无堆分配：

**张量零拷贝（owned `Value` 流转）**：
- **preprocess**：`run_preprocess` 返回 owned `Value`（(1,T,416)）+ features_len，不 `to_vec()` 成 `Array2`。
- **encode**：`run_encode` 接收 `&Value` features、返回 owned `Value` encoder_out（(1,T,416) 几 MB），不 `to_vec()` 成 `Array3`。
- **`state_values: Vec<ort::value::Value>`**（owned `DynValue`：`[0]`=logits + `[1..]`=N 个 cache，N = `uncached_out.len()-1`，base = 32）。KV cache 每步以 `ValueView`（O(1) Arc 引用计数）传回。
- **logits argmax**：`try_extract_tensor` 返回零拷贝 `(&Shape, &[f32])`（借用 Value 内存），argmax 直接在 slice 上迭代（省 vocab×4B = 128KB/步）。
- **encoder_out 传递**：`greedy_decode` 以 `&Value` 直接传为 uncached/cached 的 `args_1`。

**热路径无分配**：
- **cache 键名预算**：循环外预算 `cache_keys: Vec<String>`（"args_3".."args_{N+2}"），循环内以 `Cow::Borrowed` 复用——消除每步 N 次 `format!` + 堆分配（base=32，长音频循环数百次）。

数据流：
- preprocess 输出 owned `Value` features → `&Value` 传 encode args_0
- encode 输出 owned `Value` encoder_out → `&Value` 传 decode args_1
- uncached_decode 输出 `into_iter()` 消费为 owned Value 列表 → 初始化 `state_values`
- cached_decode 输入 args_1=encoder_out(`&Value`) + args_3..args_{N+2}=N cache（ValueView）→ 输出 `into_iter()` 消费 → 替换 `state_values`（新 logits=[0]，新 cache=[1..]）

> **实现演进**：① 初版 `state_data: Vec<Vec<f32>>` + `state_shapes`，每步 `to_vec()` 深拷贝 N 个随 seq_len 增长 cache（长音频每步 MB 级）→ 改 owned `Value` 复用。② logits 也 `to_vec()`（128KB/步，仅做 argmax）→ 改 `try_extract_tensor` 的零拷贝 `&[f32]` 直接迭代。③ encoder_out 也 `to_vec()` 成 `Array3` 仅为转 view 传回 → 改保留 owned `Value`。④ preprocess/encode 输出 `to_vec()` 成 `Array2`/`Array3` 仅为传下一步 session → 改 owned `Value` 流转，全链路零拷贝。⑤ decode 循环每步 `format!` N 个 cache 键名 → 改循环外预算 + `Cow::Borrowed` 复用。
> 注：ort `2.0.0-rc.12` 中 `DynValue` 不实现 `Clone`（bound 限 `DefiniteTensorValueTypeMarker`），故取 owned Value 走 `SessionOutputs::into_iter()` 而非 `.clone()`；`SessionOutputs<'_>` 持 session 借用，需先 `collect` 成 `Vec<Value>`（'static）才能跨 session 生命周期返回（`run_preprocess`/`run_encode`）；`&Value` 在 `ort::inputs!` 宏里直接传入以避免 `view().into()` 的 `From` 歧义（`From<&Value>` 与 `From<ValueRef>`）。

### 4. `AsrEngineManager` 路由

**`crates/asr/src/engine.rs:69`** match 追加：

```rust
config::EngineCategory::Moonshine => Arc::new(MoonshineEngine::new(entry)?),
```

### 5. CLI 入口

**`crates/cli/src/main.rs`** 追加 Moonshine 分支（类似 whisper 的 `transcribe` 路径）。

## 不涉及

- **流式识别**：Moonshine v1 是 offline 模型（v2 有 streaming 但当前使用 v1）
- **CoreML/Metal 加速**：preprocess/encode/decode 已 INT8 量化，`apply_session_acceleration` 自动适用
- **多语言**：当前模型为 `en` only（Moonshine 有其他语言版本但不在本次范围）
- **VAD 分段**：长音频走现有 `transcribe_with_vad`（`engine.rs:134`），与 Whisper 路径一致

## 验证

- 单元测试：加载 `sherpa-onnx-moonshine-base-en-int8`，对 `test_wavs/` 内置样本识别，对比 sherpa-onnx 输出
- CLI 测试：`cargo run -p octopus-cli -- transcribe <wav> --model moonshine-base-en`
- 现有引擎回归：whisper / paraformer / zipformer 测试不受影响

## 涉及文件

| 文件 | 变更 |
|------|------|
| `crates/asr/src/moonshine.rs` | **新建**：MoonshineEngine 实现 |
| `crates/asr/src/lib.rs` | `pub mod moonshine;` |
| `crates/asr/src/config.rs` | `EngineCategory::Moonshine` + 4 处映射 |
| `crates/asr/src/engine.rs` | match 路由 |
| `crates/infra/src/db.rs` | `AsrSection.moonshine` + `load_asr_config` 映射 |
| `crates/cli/src/main.rs` | CLI transcribe 入口 |


---
## 2026-06-21-paraformer-fbank-feature-extraction-fix

# 流式 Paraformer fbank 特征提取修复

**日期**: 2026-06-21
**状态**: ✅ 已实现（待 e2e 验证），已合并 main（`72e308d`，原分支 `feature/setting-ui2`）

## 背景

流式 Paraformer 识别质量严重退化：输出文本出现大量 token 重复（如 `"thedayday"`、`"tomtomor"`、`"星星期三"`），且英文单词粘连无空格、中文停顿无逗号。

## 根因分析

通过逐层对比 sherpa-onnx（Python `sherpa_onnx` v1.13.2 + C++ `feature-window.cc`）源码，定位到 **fbank 特征提取** 层有 5 个缺陷：

### 缺陷 1: 缺少 DC offset removal
sherpa-onnx `FeatureExtractorConfig` 默认 `remove_dc_offset = true`——每帧 FFT 前减去帧均值。我们的 `compute_fbank()` 完全缺失此步骤。

### 缺陷 2: 缺少 pre-emphasis 滤波
sherpa-onnx 默认 `preemph_coeff = 0.97`——预加重滤波器 `y[i] = x[i] - 0.97 * x[i-1]`，提升高频能量补偿语音谱高频衰减。我们完全缺失。

### 缺陷 3: 窗口函数错误
流式 Paraformer 使用 **povey 窗** `(0.5 - 0.5*cos(2πi/(N-1)))^0.85`，而非 hamming 窗 `0.54 - 0.46*cos(...)`。povey = hanning^0.85，与 hamming 差异显著。

### 缺陷 4: mel 滤波器 high_freq 错误
sherpa-onnx 默认 `high_freq = -400`（即 Nyquist - 400 = **7600 Hz**），我们用了 **8000 Hz**（Nyquist），导致 mel 滤波器覆盖范围不一致。

### 缺陷 5: 流式架构 — 重叠 chunk 重复提取 fbank
原架构按音频 chunk 重复提取 fbank，相邻 chunk 有 1 帧（10ms）重叠但各自独立调用 `compute_fbank()`——pre-emphasis 状态（`x_prev`）无法跨 chunk 正确传递，导致重叠帧的 fbank 值不一致。

sherpa-onnx 采用**增量式**架构：`OnlineFbank` 线性追加音频样本，fbank 帧按序计算，pre-emphasis 状态自然跨所有帧传递。

## 修复方案

### 1. `compute_fbank()` 重构（`paraformer.rs`）

参数化窗口类型 + pre-emphasis 状态：

```rust
pub(crate) fn compute_fbank(
    samples: &[f32],
    window: &[f32],        // povey（流式）或 hamming（离线）
    preemph_coeff: f32,    // 0.97
) -> Result<Array2<f32>>
```

帧处理流水线（对齐 knf `feature-window.cc`）：
```
帧样本提取 → DC offset removal（减帧均值）→ pre-emphasis（×0.97，回溯 samples[start-1]）
→ povey/hamming 窗 → FFT → 功率谱 → mel 滤波器组 → log
```

### 2. povey 窗 + mel 滤波器修正

- 新增 `POVEY_WINDOW` static + `povey_window()` 函数
- `mel_filterbank_fbank()` 的 `high_freq` 从 8000 → 7600 Hz（`high_freq = -400`）
- mel 滤波器权重计算改为 mel 空间（此前已在上一轮修复）

### 3. 流式增量式 fbank 提取（`streaming_paraformer.rs`）

**完全重写**流式引擎的音频处理架构，从"按 chunk 提取"改为"线性追加 + 增量计算"：

| 原架构 | 新架构 |
|--------|--------|
| `sample_buffer: Vec<f32>`（原始样本） | `raw_samples: Vec<f32>`（× 32768 后样本） |
| 每 chunk 调 `compute_fbank(chunk_samples)` | `fbank_cache: Vec<f32>`（已计算的所有 fbank 帧） |
| pre-emphasis 状态每 chunk 重置 | 无状态，直接回溯 `raw_samples[start-1]`（帧重叠时 start-1 才是真正前序样本） |
| chunk 间重叠帧 fbank 不一致 | 增量计算，无重复帧 |

数据流：
```
accept_samples(δsamples)
  → raw_samples.extend(δsamples × 32768)
  → compute_new_fbank_frames()    // 增量计算新帧，pre-emphasis 跨帧
  → while fbank_ready >= processed + CHUNK_SIZE:
      process_chunk_at(frame_start)  // 从 fbank_cache 切 CHUNK_SIZE 帧
      processed += CHUNK_SIZE - 1    // 1 帧重叠
```

`flush()` 补零到足够帧数后同样走 `process_chunk_at()`，最后一个 chunk force-fire CIF。

### 4. 英文单词空格 + chunk 间智能拼接

#### `decode_tokens` 重写（`paraformer.rs`）

对齐 sherpa-onnx `Convert()` 的空格逻辑：
- ASCII 词前加空格（非 subword 续接时）
- `@@` BPE 子词合并不加分隔
- 中英文边界（ASCII ↔ 非 ASCII）加空格

#### `smart_append` 辅助函数

chunk 边界拼接时自动检测 ASCII ↔ 非 ASCII 过渡并插入空格：
```rust
pub(crate) fn smart_append(existing: &mut String, new: &str) {
    // ASCII ↔ ASCII: 加空格
    // 中文 ↔ ASCII / ASCII ↔ 中文: 加空格
    // 中文 ↔ 中文: 不加空格
}
```

`StreamingParaformer::accept_samples` / `flush` 内部累积文本用 `smart_append`；
`StreamingSession::accept_samples` / `flush` 在 accumulated 与 delta 间用 `smart_append`。

### 5. VAD 停顿逗号即时反馈

`StreamingSession::flush(insert_comma: bool)` 新增参数：
- `insert_comma = true` 时，flush 产生的文本末尾**立即追加逗号**
- 此前逗号只在下一句话到来时才插入（`accept_samples` 的 `was_silent` 分支），停顿期间无标点反馈
- `coordinator.rs` 和 `server/main.rs` 的 flush 调用均传 `insert_comma = true`

## 涉及文件

| 文件 | 变更 |
|------|------|
| `crates/asr/src/paraformer.rs` | `compute_fbank` 参数化 + DC offset + pre-emphasis + povey 窗 + mel high_freq + `decode_tokens` 重写 + `smart_append` |
| `crates/asr/src/streaming_paraformer.rs` | 增量式 fbank 架构重写（`raw_samples` + `fbank_cache`，pre-emphasis 无状态）|
| `crates/asr/src/streaming_engine.rs` | `flush(insert_comma)` + `smart_append` 拼接 |
| `crates/desktop/src/coordinator.rs` | `flush(true)` 调用 |
| `crates/server/src/main.rs` | `flush(true)` 调用 |

## 验证

### 识别质量对比（test_wavs/0.wav）

| 版本 | 输出 |
|------|------|
| 修复前 | `昨天是mondaytodayisplease班二thedaydaytomtomorrow星星期三` |
| **修复后** | `昨天是 monday today day is 礼拜二 the day after tomorrow 是星期三` |
| sherpa-onnx 参考值 | `昨天是 monday today day is 礼拜二 the day after tomorrow 是星期` |

47 项单元测试全通过，server/cli/desktop release 构建成功。

## 后续优化（同分支追加）

### 6. BPE 跨 chunk 整体解码

**问题**：`value` 被切成 `val@@` + `ue` 两个 token，chunk 边界各自 decode 导致断词。

**修复**：`StreamingParaformer` 新增 `all_token_ids: Vec<i64>` 跨 chunk 累积所有有效 token ID，`accept_samples` / `flush` 整体调用 `decode_tokens(all_token_ids)` 返回完整 ASR 文本。`process_chunk_at` 只累积 token 不再逐 chunk 解码。

`StreamingSession` Paraformer 路径改为 `punct_prefix` + `committed_chars` 双字段逗号管理：静音点冻结当前 ASR 快照 + 插逗号（`committed_chars` 推进），后续新 delta（`full_asr` skip 已提交字符）拼在逗号后。

### 7. 热路径性能优化（零拷贝）

| 优化点 | 每次节省 | 方式 |
|--------|---------|------|
| decoder_caches 更新 | ~320KB 堆分配（16×512×10×4B） | `copy_from_slice` 复用预分配 Array3，维度变化才重分配 |
| encoder 特征构造 | ~45KB clone（10×560×4B） | `into_shape` 零拷贝 reshape 替代 `iter().cloned().collect()` |
| run_cif encoder 数据 | ~20-40KB 拷贝 | `as_slice().unwrap()` 直接拿 `&[f32]`，移除 `.to_vec()` |
| decoder input 键名 | 16× `format!()` | `cache_keys: Vec<String>` 预分配于 `new()` |
| FFT 规划 | 每 chunk 一次 `FftPlanner::new()` + `plan_fft_forward(512)`（堆分配 + twiddle 计算） | `FBANK_FFT: Lazy<Arc<dyn rustfft::Fft<f32>>>` 全局静态（`paraformer.rs`，与 `POVEY_WINDOW` 同位置），`compute_fbank` 与流式 `compute_new_fbank_frames` 共用 |
| feat_cache reshape | ~17.5KB clone（8×560×4B） | `apply_feat_overlap` 用 `ArrayView2::from_shape` 包装 `&self.feat_cache` |
| acoustic_embeds 输入 | `acoustic.to_vec()` 拷贝（num_tokens×560×4B） | `run_decoder` 用 `ArrayView3::from_shape` 包装 `acoustic: &[f32]`，与 `qwen3_asr.rs` 的 `ArrayView3+TensorRef` 模式一致 |
| 单元素长度张量 | 2× `vec![x]` 堆分配（`enc_len` + `acoustic_len`） | `run_decoder` 用栈数组 `[x]` + `ArrayView1::from(&[x])` 替代 `Array1::from_vec(vec![x])` |
| `reset()` decoder 缓存 | 16× Array3 重新分配（首次 / 形状匹配后均为 fill） | `decoder_caches` 形状与初始 `(1, encoder_output_size, cache_time)` 一致时 `fill(0.0)` 复用内存；不一致（run_decoder 慢路径改过维度）才重分配恢复初始形状 |
| 离线 `transcribe` encoder 输出 | `enc_tensor.clone().into_raw_vec_and_offset()` 整段拷贝（enc_len×512×4B） | `paraformer.rs` 离线 CIF 循环改用 `enc_tensor.slice(s![0, ..enc_len_scalar, ..]).as_slice().unwrap()` 直接借用，`enc_tensor` 保留供 decoder `view()` 使用；附带将 `speech_lengths`/`acoustic_len`/`enc_len_for_dec` 单元素张量统一为栈数组 + `ArrayView1` |

### 8. mask_alphas 越界防护

`mask_alphas` / `mask_alphas_left_only` 改为 `n = alphas.len().min(enc_len)` 再循环，消除 `alphas.len() < enc_len` 时的 panic 风险。

### 9. 边界鲁棒性防御

| 位置 | 风险 | 修复 |
|------|------|------|
| `smart_append`（`paraformer.rs`） | 空格字节 `0x20` 满足 `< 0x80`（ascii），若 `existing` 末尾或 `new` 首字符已是空格，会再 push 空格 → 双空格 | 空格判定条件追加 `&& last_byte != 0x20 && first_byte != 0x20`，任一侧已是空格则不再添加 |
| `run_cif` / `run_cif_final`（`streaming_paraformer.rs`） | `enc_len` 来自 ONNX `enc_len_data[0]`，若异常（padding/截断）导致 `enc_len > enc_tensor.shape()[1]`，`slice(s![0, ..enc_len, ..])` 直接 panic | 改为 `..enc_len.min(enc_tensor.shape()[1])` 防御性截断，与 `mask_alphas` 同模式 |

### 10. accept_samples 清除 flush 的 input_finished 标记（会话内状态污染修复）

**问题**：`input_finished: bool`（`streaming_paraformer.rs`）在 `flush()` 静音冲刷时置 `true`，让 `compute_new_fbank_frames` 走收尾分支——末帧允许越界、零 padding 多算帧，配合 CIF force-fire（`run_cif_final`）吐出憋住的尾音。该标记仅在 `reset()` 清除，而 `reset()` 只在会话边界（录音停止 / 取消）调用。**Paraformer 流式会话内不 reset**（累积上下文跨 chunk，见 architecture「流式 ASR 状态语义」），导致用户停顿冲刷尾音后继续说话时，`accept_samples` 仍见 `input_finished=true` → 持续走收尾分支 → 每次 `accept_samples` 多算越界零 padding 帧 → 特征错乱 → 识别错乱 / 丢字 / 大量重复字（首次停顿后整段会话腐烂）。

**修复**：`accept_samples` 入口 `self.input_finished = false`——`accept_samples` 的语义即「继续说话」，必须回到正常帧计算模式。`reset()` 仍在会话边界不动（清的是会话级全部状态）。

**严重度**：本专项（「首字识别不出来 / 尾字吐不出来」审查）中最严重的一个——前 3 个问题（`segment_audio_vad` padding、`filter_speech` 两端 trim、Zipformer flush replicate padding）影响首尾字边界或单 tick 尾音延迟，本问题导致**首次停顿后会话级识别腐烂**，用户全程可感知。

**回归测试**：`test_accept_samples_clears_input_finished_after_flush`——断言 `flush()` 后 `input_finished == true`，`accept_samples()` 后 `input_finished == false`。



---
## 2026-06-21-polish-prompt-table-design

# LLM 润色提示词表设计

**日期**：2026-06-21
**类型**：新功能（DB schema 变化 + prompt 组装重构）
**相关文件**：`crates/infra/src/db.sql`、`crates/infra/src/db.rs`、`crates/llm/src/prompt.rs`、`crates/desktop/src/main.rs`、`crates/desktop/src/settings_commands.rs`、`crates/desktop/src/coordinator.rs`、`crates/desktop/src/runtime_config.rs`、`docs/architecture.md`、`docs/configuration.md`

## 1. 目标

把当前单文件 `~/.octopus/VOICE_POLISH.md` 的润色 prompt 机制改为 DB 多 prompt 管理：

- DB `prompts` 表存多条润色 prompt
- `app_config.active_polish_prompt` 指定当前激活的一条（id）
- 有一条 `is_system=true` 的默认兜底 prompt（seed，不可编辑/删除）
- 用户可添加任意特色 prompt（如「日常沟通」「技术写作」「会议纪要」），激活其一
- 删除 `VOICE_POLISH.md` 文件读取机制（开发阶段无历史遗留）

## 2. DB Schema

### 2.1 新增 `prompts` 表

```sql
CREATE TABLE IF NOT EXISTS prompts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,    -- 系统主键，app_config.active_polish_prompt 引用此字段（用户不可编辑）
    title       TEXT    NOT NULL,                     -- 用户可读别名（允许重复，用户自行区分）
    category    TEXT    NOT NULL DEFAULT 'voice_text_polish', -- 用途分类（当前固定 voice_text_polish 语音文本润色）
    content     TEXT    NOT NULL,                     -- system prompt 的「风格规则」部分（不含增量保留规则）
    description TEXT    NOT NULL DEFAULT '',          -- 用户可读描述
    is_system   INTEGER NOT NULL DEFAULT 0,           -- 1=系统内置（不可编辑/删除），0=用户自建
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);
```

**设计决策**：
- `id` 作为系统主键（全表唯一、系统生成、用户不可编辑），`app_config.active_polish_prompt` 存 id（以字符串形式，与其他 app_config 一致）
- `title` 作为用户可读别名，**允许重复**（用户自行区分即可，不做唯一约束）
- `category` 标记 prompt 用途，当前固定 `voice_text_polish`（语音文本润色）
- `is_system` 用 INTEGER（0/1），与 `models.is_local` 等现有列一致
- `content` 只存「风格规则」部分，增量保留规则由代码强制拼接（见 §3）
- 不设 `is_enabled` 列——prompt 不需要禁用，不激活就不用

### 2.2 Seed 默认 prompt

```sql
INSERT OR IGNORE INTO prompts (id, title, category, content, description, is_system) VALUES
    (1, '默认润色', 'voice_text_polish', '<当前 DEFAULT_SYSTEM_PROMPT 的前 6 条规则>', '默认润色（系统内置）', 1);
```

固定 `id=1` 作为系统默认 prompt。Seed content = 现有 `DEFAULT_SYSTEM_PROMPT` 去掉第 7 条（增量保留规则），第 7 条改为代码常量强制拼接。

### 2.3 新增 app_config key

```sql
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
    ('active_polish_prompt', '1', '激活的润色 prompt id（prompts 表 id 字段）');
```

默认值 `'1'` 指向 seed 的系统内置 prompt（id=1）。存为字符串（与其他 app_config 一致，TEXT 列）。

### 2.4 Schema 版本迁移

`init_schema` 新增 `v3 → v4` 迁移：
- 执行 `CREATE TABLE IF NOT EXISTS prompts` + `INSERT OR IGNORE` seed（幂等，IF NOT EXISTS / OR IGNORE）
- 执行 `INSERT OR IGNORE INTO app_config` seed `active_polish_prompt`
- `PRAGMA user_version = 4`

同时更新 `INIT_SQL`（`db.sql`）包含新表 + seed，保证全新安装一步到位。

## 3. Prompt 组装重构（`crates/llm/src/prompt.rs`）

### 3.1 当前结构

```
DEFAULT_SYSTEM_PROMPT = 第 1~6 条风格规则 + 第 7 条增量保留规则
PROMPT_OVERRIDE = VOICE_POLISH.md 内容（整体覆盖）
system_prompt() = PROMPT_OVERRIDE 或 DEFAULT_SYSTEM_PROMPT
```

### 3.2 新结构

```
INCREMENTAL_RULE = 第 7 条增量保留规则（代码常量，含 CONFIRMED_MARKER）
system_prompt(user_content: &str) = user_content + "\n" + INCREMENTAL_RULE
```

**关键变化**：
- `set_system_prompt_override` / `PROMPT_OVERRIDE` / `DEFAULT_SYSTEM_PROMPT` **删除**
- 新增 `pub fn build_system_prompt(content: &str) -> String`：拼接用户 prompt + 强制增量规则
- `user_prompt()` 不变（它构造 user message，与 system prompt 解耦）
- `CONFIRMED_MARKER` 不变（`INCREMENTAL_RULE` 复用它）

### 3.3 调用方改造

**`crates/desktop/src/main.rs`**（启动时加载 prompt）：
- 删除 `VOICE_POLISH.md` 读取逻辑（约 130-145 行）
- 改为从 DB 读 `active_polish_prompt` → 查 `prompts` 表取 content → `build_system_prompt(content)` 传给润色流程

**润色调用链**（`coordinator.rs` → `spawn_polish_thread` → `octopus_llm::polish`）：
- 当前 `octopus_llm::polish` 内部调 `system_prompt()` 取全局静态值
- 改为：调用方传入 `system_prompt: &str` 参数（由 `build_system_prompt` 构建）
- 或：保留全局静态，但改为运行时可切换（`set_system_prompt` 接受新 content → 重新 build）

**推荐方案：运行时可切换的全局静态**。理由：
- 改动最小（`polish` 签名不变，内部仍调 `system_prompt()`）
- 切换 prompt 时只需 `set_system_prompt(new_content)` → 下次润色生效
- `system_prompt()` 返回 `&'static str` 改为 `&str`（指向 `RwLock<String>`）

具体实现：
```rust
static SYSTEM_PROMPT: RwLock<String> = RwLock::new(String::new());

/// 设置当前 system prompt（content 为用户 prompt 部分，内部自动拼接增量规则）
pub fn set_system_prompt(content: &str) {
    *SYSTEM_PROMPT.write().unwrap() = build_system_prompt(content);
}

/// 获取当前 system prompt（已含增量规则）
pub fn system_prompt() -> String {
    SYSTEM_PROMPT.read().unwrap().clone()
}
```

**注意**：`system_prompt()` 从 `&'static str` 改为 `String`（clone），调用方需适配。影响范围：`crates/llm/src/lib.rs`（`polish` 函数）。

## 4. 设置窗口 Tauri 命令

新增 5 个命令（`settings_commands.rs`）：

| 命令 | 签名 | 说明 |
|------|------|------|
| `list_prompts` | `() -> Vec<PromptInfo>` | 列出所有 prompt（按 id 排序，系统内置优先） |
| `get_active_prompt` | `() -> i64` | 返回当前激活的 prompt id |
| `set_active_prompt(id: i64)` | `-> Result<()>` | 设置激活 prompt（校验 id 存在 + 写 app_config + 调 `set_system_prompt` 即时生效） |
| `create_prompt(title, content, description)` | `-> Result<i64>` | 新建用户 prompt（校验 title 非空）返回新 id |
| `update_prompt(id, title, content, description)` | `-> Result<()>` | 更新用户 prompt（拒绝 is_system=true） |
| `delete_prompt(id)` | `-> Result<()>` | 删除用户 prompt（拒绝 is_system=true；若删的是激活项，回退到 id=1） |

**`PromptInfo` 结构**：
```rust
#[derive(Serialize)]
pub struct PromptInfo {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub description: String,
    pub is_system: bool,
}
```

## 5. 运行时切换

工具栏已有「润色模型」切换（`switch_polish_llm`）。是否需要工具栏加「润色 prompt」切换？

**决定：不加**。理由：
- prompt 切换是低频操作（不像润色模式 / 引擎切换那样需要快速访问）
- 设置窗口的 prompt 管理页足够
- `set_active_prompt` 即时生效（写 app_config + `set_system_prompt`），下次润色就用新 prompt

若后续需要快速切换，可在工具栏加二级菜单，当前 YAGNI。

## 6. DB CRUD 函数（`crates/infra/src/db.rs`）

新增函数：

```rust
pub struct PromptRecord {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub description: String,
    pub is_system: bool,
}

pub fn list_prompts() -> Result<Vec<PromptRecord>>       // 按 id 排序，is_system 优先
pub fn load_prompt(id: i64) -> Result<Option<PromptRecord>>
pub fn insert_prompt(title: &str, content: &str, description: &str) -> Result<i64>  // 返回新 id
pub fn update_prompt(id: i64, title: &str, content: &str, description: &str) -> Result<()>
pub fn delete_prompt(id: i64) -> Result<()>
```

**约束**（DB 层或应用层）：
- `id` 主键唯一由 DB 保证；`title` 无唯一约束（允许重复）
- `update_prompt` / `delete_prompt`：应用层检查 `is_system`，拒绝系统 prompt

## 7. 不变量

1. **prompts 表永远有一条 id=1 的记录**（seed 保证，用户不能删）
2. **`active_polish_prompt` 永远指向存在的 prompt id**（set_active_prompt 校验；若指向的 prompt 被外部删除，fallback 到 id=1）
3. **system prompt 永远含增量保留规则**（`build_system_prompt` 强制拼接，用户 content 无论写什么都会追加）
4. **is_system=true 的 prompt 不可编辑/删除**（update/delete 应用层拒绝）
5. **切换 prompt 即时生效**（`set_active_prompt` 调 `set_system_prompt`，下次润色用新 prompt；进行中的润色不受影响——LLM 请求已发出）

## 8. 降级路径

- **DB 读 prompt 失败**：fallback 到 `INCREMENTAL_RULE` 拼接空 content（等价于无风格规则，仅保留增量逻辑）+ warn 日志
- **`active_polish_prompt` 指向不存在的 id**：fallback 到 id=1 + warn 日志 + 自动修正 app_config
- **prompt content 为空**：允许（等价于纯增量规则，用户可能只想做标点修正）

## 9. 验证方法

- **单元测试**（`db.rs`）：list/load/insert/update/delete prompt 的 CRUD + is_system 保护
- **单元测试**（`prompt.rs`）：`build_system_prompt` 拼接正确（用户 content + 增量规则）
- **集成验证**（手动）：
  1. 启动 → 确认默认 prompt = id=1（默认润色），润色结果与改动前一致
  2. 新建 prompt「技术写作」→ 激活 → 确认润色风格变化
  3. 切回 id=1 → 确认风格恢复
  4. 尝试删除 id=1 → 确认被拒绝
  5. 尝试编辑 id=1 → 确认被拒绝
  6. mode=2 中间润色 → 确认增量保留规则生效（已确认部分不被 LLM 改）
- **构建验证**：`cargo build -p octopus-desktop --features embedded,cloud` + `cargo test`

## 10. 文件变更清单

| 文件 | 变更 |
|------|------|
| `crates/infra/src/db.sql` | 新增 prompts 表 + seed + app_config seed |
| `crates/infra/src/db.rs` | v3→v4 迁移 + PromptRecord struct + 5 个 CRUD 函数 + 测试 |
| `crates/llm/src/prompt.rs` | 删除 PROMPT_OVERRIDE/DEFAULT_SYSTEM_PROMPT，新增 INCREMENTAL_RULE/build_system_prompt/set_system_prompt，system_prompt 改返回 String |
| `crates/llm/src/lib.rs` | 适配 system_prompt() 返回类型变化 |
| `crates/desktop/src/main.rs` | 删除 VOICE_POLISH.md 读取；改为从 DB 读 active prompt → set_system_prompt |
| `crates/desktop/src/settings_commands.rs` | 新增 6 个 Tauri 命令 + PromptInfo struct |
| `crates/desktop/src/runtime_config.rs` | set_active_prompt 即时生效（调 set_system_prompt） |
| `crates/infra/src/consts.rs` | 删除 VOICE_POLISH_FILE 常量 |
| `crates/desktop/src/main.rs` | 注册新 Tauri 命令 |
| `docs/architecture.md` | 同步 prompt 管理章节 |
| `docs/configuration.md` | 新增 active_polish_prompt 字段说明 |


---
## 2026-06-21-tencent-asr-design

# 腾讯云 ASR 实时语音识别接入设计

> 文档：https://cloud.tencent.com/document/product/1093/48982
> 对标实现：ByteDanceStreamSession / AliyunStreamSession（Stage::CloudStreaming 路径）

## 功能概述

接入腾讯云实时语音识别 WebSocket API，作为第三个云端 ASR provider（与 Aliyun、ByteDance 并列）。采用签名鉴权（HMAC-SHA1），WebSocket 文本帧响应 JSON 结果。

## 协议要点

### Endpoint

固定 `wss://asr.cloud.tencent.com/asr/v2/<appid>?{params}`

- `<appid>` 替换为用户 AppID（URL 路径段）
- `{params}` 为查询参数串（含签名）

### 鉴权（签名生成）

三步：

1. **拼接签名原文**：除 `signature` 外的所有参数按**字典序**排序，拼接为
   `asr.cloud.tencent.com/asr/v2/<appid>?key1=value1&key2=value2&...`
2. **HMAC-SHA1 + Base64**：`signature_raw = Base64(HMAC-SHA1(sign_str, SecretKey))`
3. **URL 编码**：`signature = urlencode(signature_raw)`（必须编码 `+`、`=`、`/` 等特殊字符）

最终 URL = `wss://...?{sorted_params}&signature={encoded_signature}`

### 必填握手参数

| 参数 | 说明 |
|---|---|
| `secretid` | 腾讯云 SecretID |
| `timestamp` | 当前 UNIX 时间戳（秒） |
| `expired` | 签名过期时间戳（秒），须 > timestamp |
| `nonce` | 随机正整数（≤10 位） |
| `engine_model_type` | 引擎模型（如 `16k_zh`、`16k_zh_en`） |
| `voice_id` | 音频流 UUID（每次连接重新生成） |
| `signature` | 签名 |

可选参数：`voice_format=1`（PCM）、`needvad=1`、`filter_punc=1`、`vad_silence_time=1000`

### 音频发送

- **WebSocket Binary 帧**：原始 PCM s16le 字节，**无额外头**
- **速率**：200ms 音频 = 6400 字节（16k），1:1 实时率
- 发送过快或间隔 >6s 会被服务端断开

### 响应格式（Text 帧 JSON）

顶层字段：
- `code`：0=正常，非 0=错误（错误码表见官方文档）
- `message`：错误描述
- `final`：1 = 全部识别结束（连接将断开）
- `result`：识别结果对象

`result` 字段：
- `slice_type`：0=开始，1=识别中（非稳态），2=识别结束（稳态）
- `index`：句序号（从 0 递增）
- `voice_text_str`：文本

### 结束信号

客户端发 Text 帧 `{"type":"end"}` → 服务端返回 `final=1` → 断开连接。

## DB 映射

需要 3 个鉴权信息：AppID、SecretID、SecretKey。

| DB 字段 | 腾讯含义 | 示例 |
|---|---|---|
| `source` | `{appid}:{secretid}` 复合字段 | `1259221234:AKIDxxxxxxxxxxxxx` |
| `secret_key` | SecretKey（HMAC 签名密钥） | `yyyyyyyyyyyyyy` |
| `model_name` | DB 内标识（= engine_model_type） | `16k_zh`、`16k_zh_en` |

> Endpoint 固定，不存 DB。`source` 用冒号分隔 AppID 和 SecretID（与 model spec 的 3-part 冒号不冲突——DB `source` 列是自由文本）。

## 与 Aliyun / ByteDance 的差异

| 维度 | Aliyun | ByteDance | **Tencent** |
|---|---|---|---|
| 鉴权 | Bearer token | X-Api-Key header | **URL 签名（HMAC-SHA1）** |
| 音频帧 | Raw PCM / base64 | gzip(PCM) | **Raw PCM（binary frame）** |
| 响应 | JSON text | Binary + gzip(JSON) | **JSON text** |
| 结束信号 | finish-task JSON | 末帧 flags=0x2 | **`{"type":"end"}` text** |
| Endpoint 来源 | DB source | 固定 | **固定 + appid 路径段** |
| 额外依赖 | — | flate2 | **hmac + sha1** |

## 架构设计

### EngineCategory::Tencent

- `crates/asr/src/config.rs`：新增 `Tencent` 变体
- `resolve_category`：`provider == "tencent"` → `Some(Tencent)`
- `is_streaming_engine`：排除 Tencent（与 Aliyun/ByteDance 一致）
- `coordinator::is_cloud_engine`：`matches!(cat, Some(Aliyun) | Some(ByteDance) | Some(Tencent))`

### TencentStreamSession

- 文件：`crates/desktop/src/tencent_stream.rs`
- 接口与 `AliyunStreamSession` / `ByteDanceStreamSession` 完全一致：
  `open` / `push_pcm` / `finish` / `try_recv_text` / `close_async`
- 复用 `aliyun_stream::{PcmFrame, StreamEvent}`

### CloudSession enum

`crates/desktop/src/cloud_session.rs` 新增 `Tencent(TencentStreamSession)` 变体，方法分派。

### 文本累积策略

腾讯返回分句增量结果（`slice_type=0/1/2`），需自行累积：
- `slice_type=2`（稳态）→ 存入 `BTreeMap<index, text>`
- `slice_type=0/1`（非稳态）→ 临时 partial
- 发给 coordinator 的 `StreamEvent::Text` = `stable_segments.join("") + current_partial`
- `final=1` → `StreamEvent::Text(stable)` then `StreamEvent::Finished`

## 降级与安全

- 无 API Key 时 DB `secret_key` 为空 → `resolve_tencent_config` 返回明确错误
- 签名过期（timestamp 偏差大）→ 服务端返回 code=4002，session 报 Failed
- 速率超限 → 服务端返回 code=4000 并断开


---
## 2026-06-21-toggle-stop-polish-race-design

# Toggle 停止时立即润色结果丢失修复

**日期**：2026-06-21
**类型**：Bug 修复（涉及状态机改造）
**相关文件**：`crates/desktop/src/coordinator.rs`、`crates/desktop/src/transcript.rs`

## 1. 问题描述

### 1.1 复现步骤

1. 用户说话（ASR 持续累积 `transcript.full`）
2. 用户点击工具栏「立即润色」按钮 → `handle_polish_now` 发起异步 LLM 润色请求（`Command::PolishDone` 待返回）
3. **LLM 请求在途时**（典型 1~3 秒），用户按快捷键 Toggle 结束录音

### 1.2 错误行为

- 插入到光标位置的是**原始 ASR 文本**，不是润色后的文本
- SQLite 数据库 `transcriptions` 表只存了 `raw_text`，`polished_text` 为空
- 用户主动点的「立即润色」结果完全丢失

### 1.3 根因分析

`handle_toggle` 的三个停止分支（Streaming / VadSegmented / CloudStreaming）在停止录音时都会执行：

```rust
transcript.clear_polish_pending();  // 第 827/889/957 行
```

然后调用 `start_final_polish_or_paste`。

这导致两个问题：

**问题 A：立即润色的 `Command::PolishDone` 被丢弃**

`handle_polish_done`（第 2385 行）要求当前 stage 仍是活跃录音阶段（Streaming / VadSegmented / WaitingCompletion / CloudStreaming / CloudClosing），否则：

```rust
_ => {
    debug!("PolishDone ignored: stage={} 不是录音/等待阶段，润色结果丢弃", ...);
    let _ = app_handle.emit("polish-done", ());
    return;
}
```

Toggle 后 stage 已切换到 `Polishing` / `Pasting` / `Idle`，立即润色的结果到达时被直接丢弃。

**问题 B：最终润色被 `polish_mode` 跳过**

`start_final_polish_or_paste` 调用 `crate::config::llm_config(config)` 判断是否润色：

```rust
pub fn llm_config(cfg: &AppConfig) -> Option<...> {
    if cfg.polish_mode == PolishMode::Disabled {
        return None;  // mode=0 时直接返回 None
    }
    ...
}
```

当 `polish_mode=0`（Disabled）时返回 `None`，`start_final_polish_or_paste` 走 `None => do_paste` 分支，**跳过润色**直接粘贴。此时 `final_text = transcript.db_text()` = 原始 ASR 文本。

**两个问题叠加**：立即润色结果被丢弃 + 最终润色被跳过 → 用户得到的是原文。

### 1.4 影响范围

- `polish_mode=0` + 立即润色 + Toggle：**必现**，用户主动润色的结果丢失
- `polish_mode=1/2` + 立即润色 + Toggle（LLM 未及时返回）：立即润色结果丢失，但最终润色会重新润色全量文本（部分恢复，但浪费了一次 LLM 调用 + 用户看到的是第二次润色的结果而非第一次）

## 2. 设计

### 2.1 核心思路

**不再清除 `polish_pending`，而是等待立即润色完成后再走 final 路径。**

引入新 stage `Stage::StoppingPolish { transcript }`：Toggle 停止时，若 `transcript.polish_pending() == true`，把 transcript 移入此 stage 等待 `Command::PolishDone`；PolishDone 到达后按 `polish_mode` 决定后续路径。

### 2.2 立即润色语义澄清

**立即润色** = 中途触发的一次 LLM 润色，等同 `polish_mode=2` 的停顿润色。Toggle 停止后：

- **mode=0（Disabled）**：最终输出 = 立即润色结果（`polished`）+ 后续新增 ASR（`increase`）。**不再触发最终润色**。DB `polished_text` = 立即润色结果（不含 Toggle 后新增 ASR）。
- **mode=1/2**：触发最终润色（preserved 含已 polished 的部分 + increase 整体再润色一次）。DB `polished_text` = 最终润色结果。

### 2.3 新增 Stage

```rust
/// Toggle 停止录音后，仍有进行中的立即润色（PolishNow 未返回）。
/// 持有 transcript 等待 `Command::PolishDone` 到达，再按 polish_mode 决定后续路径。
StoppingPolish {
    transcript: Transcript,
},
```

### 2.4 Toggle 停止路径改造

三个 Toggle 停止分支（Streaming / VadSegmented / CloudStreaming）抽取公共收尾函数 `finalize_after_stop`：

```rust
fn finalize_after_stop(
    stage: &mut Stage,
    transcript: Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    // 1. 立即润色仍在途：等其完成再走 final 路径
    if transcript.polish_pending() {
        info!("Toggle stop: polish_pending=true, entering StoppingPolish");
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Processing);
        crate::result_window::show_result(app_handle, "⏳ 等待润色完成...");
        *stage = Stage::StoppingPolish { transcript };
        return;
    }
    // 2. 无 pending：检查是否需要最终润色
    //    优化：若 polished 非空且无新增 ASR（has_increase=false），立即润色已覆盖全部文本，
    //    跳过最终润色（mode=1/2 也跳过），直接 paste，避免平白多一次 LLM 调用。
    let skip_final_polish = !transcript.polished().is_empty() && !transcript.has_increase();
    //    句末标点补全 + display_text 计算（与原 final 路径一致）
    let combined = if let Some(edited) = transcript.edited_display() {
        edited
    } else if transcript.full().is_empty() {
        String::new()
    } else if transcript
        .full()
        .ends_with(|c: char| ",.，。！？!?\n".contains(c))
    {
        transcript.db_text()
    } else {
        format!("{}。", transcript.db_text())
    };
    if combined.is_empty() {
        *stage = Stage::Idle;
        crate::result_window::hide_result(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
        return;
    }
    crate::result_window::show_result(app_handle, &transcript.display_text());
    if skip_final_polish {
        // 立即润色已覆盖全部文本，直接 paste（polish_status="done"，DB polished_text=立即润色结果）
        info!("Toggle stop: skip final polish (polished covers all, no increase)");
        do_paste(stage, &transcript.display_text(), transcript.id, &transcript.db_text(), "done", config, app_handle, tx);
    } else {
        // 走原 final 路径（按 polish_mode 决定是否润色）
        start_final_polish_or_paste(stage, &combined, transcript, config, app_handle, tx);
    }
}
```

**`skip_final_polish` 判定逻辑**：
- `!transcript.polished().is_empty()`：立即润色成功（有 polished 文本）。若立即润色失败，polished 为空，仍需走最终润色兜底。
- `!transcript.has_increase()`：Toggle 时无新增 ASR 文本（`raw_len == full.len()`）。`has_increase` 仅在 `polish_mode=Intermediate` 时有意义（其他 mode 恒返回 false），但这不影响正确性——非 Intermediate mode 时 polished 已是全量润色，无 increase 概念，跳过最终润色同样正确。

**效果**：
- mode=0 + 立即润色成功 + 无新增：直接 paste display_text（polished），DB polished_text=立即润色结果
- mode=1/2 + 立即润色成功 + 无新增：**跳过最终润色**（原行为会再调一次 LLM），直接 paste display_text
- mode=1/2 + 立即润色成功 + 有新增：走最终润色（preserved=polished + increase 整体润色）
- mode=1/2 + 立即润色失败 + 任意：走最终润色（兜底）
- 任何 mode + 无立即润色：走原 final 路径（start_final_polish_or_paste）

### 2.5 `handle_polish_done` 改造

新增 `StoppingPolish` arm：

```rust
Stage::StoppingPolish { transcript } => {
    // 校验 session_id（跨会话护栏，与现有逻辑一致）
    if transcript.id != session_id { ...丢弃... return; }
    // 写入润色结果（on_polish_done / on_polish_failed）
    match result {
        Ok(polished) => { transcript.on_polish_done(polished); ...DB UpdatePolished... }
        Err(e) => { transcript.on_polish_failed(); }
    }
    // PolishDone 处理完成后，走 final 路径
    let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
    finalize_after_stop(stage, tr, config, app_handle, tx);
}
```

关键：`on_polish_done` 后 `polish_pending == false`，所以 `finalize_after_stop` 会走第 2 分支（无 pending），按 `polish_mode` 决定：
- mode=0：`llm_config` 返回 None → `do_paste(display_text)`（含 polished + increase）
- mode=1/2：`llm_config` 返回 Some → `start_final_polish_or_paste`（最终润色）

### 2.6 其他命令对 StoppingPolish 的处理

| 命令 | 处理 |
|------|------|
| `Toggle` | 忽略（busy，与现有 Polishing 一致） |
| `Cancel` | 删除 DB 脏数据（已有逻辑覆盖）+ 回 Idle |
| `Discard` | finalize DB 记录 + 回 Idle（与现有 Polishing stage 处理一致） |
| `PolishNow` | 忽略（已有 polish_pending，原有逻辑覆盖） |
| `PolishDone` | 见 §2.5 |
| `FinalPolishDone` | 忽略（此阶段不会有最终润色在途） |
| `StreamingTick` / `VadSegmentedTick` / `CloudStreamingTick` | 忽略（录音已停） |
| `TranscriptionDone` | 忽略（VadSegmented 已停） |

### 2.7 UI 反馈

进入 `StoppingPolish` 时：
- 托盘：`TrayState::Processing`（「处理中」）
- 结果窗：`show_result("⏳ 等待润色完成...")`
- 前端「立即润色」按钮：保持 disabled（polish_pending 期间）

PolishDone 到达后按 final 路径走，UI 由 `start_final_polish_or_paste` / `do_paste` 接管。

## 3. 不变量

1. **进入 StoppingPolish 前所有 ASR 源已停止**：Streaming 的 `finish()` / VadSegmented 的 tick 停止 / CloudStreaming 的 session 处理都在进 StoppingPolish 之前完成。StoppingPolish 期间 `transcript.full` 不会再增长。
2. **`polish_pending` 在 StoppingPolish 期间保持 true**：直到 `on_polish_done` / `on_polish_failed` 清除。
3. **PolishDone 的 session_id 护栏不变**：跨会话（Cancel + 重开）时旧 PolishDone 会被丢弃。
4. **VadSegmented active_count > 0 仍走 WaitingCompletion**：WaitingCompletion 收齐后会调 `finalize_after_stop`（不再 clear_polish_pending），若此时仍有 pending 则进 StoppingPolish。

## 4. 降级路径

- **LLM 失败**（PolishDone 返回 Err）：`on_polish_failed` 清 pending 但不写 polished → `finalize_after_stop` 走无 pending 分支 → mode=0 时 paste `display_text`（polished 为空则用 raw）；mode=1/2 时触发最终润色（兜底）。
- **用户 Cancel**：`handle_cancel` 已有逻辑覆盖（检测 `db_inserted` 并 Delete），StoppingPolish arm 加入即可。
- **用户 Discard**：`handle_discard` 已有逻辑覆盖（finalize DB 记录），StoppingPolish arm 加入即可。

## 5. 验证方法

- **单元测试**：transcript.rs 已有的 `take_polish_input` / `on_polish_done` 测试覆盖核心逻辑，无需新增
- **集成验证**（手动）：
  1. mode=0：说话 → 立即润色 → 立即 Toggle → 确认粘贴的是润色结果 + 新增 ASR
  2. mode=1：说话 → 立即润色 → 立即 Toggle → 确认触发最终润色 → 粘贴最终润色结果
  3. LLM 慢（模拟）：说话 → 立即润色 → 等 2s → Toggle → 确认进入「⏳ 等待润色完成...」→ PolishDone 后正常 paste
  4. Cancel during StoppingPolish：进入 StoppingPolish → 按 Esc → 确认 DB 记录被删除
- **构建验证**：`cargo build -p octopus-desktop --features embedded,cloud` + `cargo test -p octopus-desktop --features embedded,cloud`

## 6. 与现有代码的关系

- `Stage::Polishing`（最终润色）：**不变**。StoppingPolish 是其前置阶段（仅当 Toggle 时有 pending）
- `Stage::WaitingCompletion`（VadSegmented 等识别完成）：**不变**。收齐后调 `finalize_after_stop`
- `Stage::CloudClosing`（云端 close_async 等待）：**不变**。CloudStreamingDone 回来后调 `finalize_cloud` → `finalize_after_stop`
- `handle_polish_done`：**扩展**，新增 StoppingPolish arm
- `handle_cancel` / `handle_discard`：**扩展**，新增 StoppingPolish arm
- `handle_toggle`：**扩展**，新增 StoppingPolish arm（忽略，busy）
- `stage_name`：**扩展**，新增 StoppingPolish

