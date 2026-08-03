# 全量代码审查 bugfix（21 crate，14 处修复）

> **日期**：2026-08-03
> **状态**：✅ 已实现（14 处全修复，全量测试通过）
> **来源**：外部全量代码审查报告（21 crate 非增量审查，14 个具体问题 + 中低清单）
> **分支**：`.worktrees/bugfix_pr_0801`（bugfix/pr-0801）

## 0. 复查结论

外部报告的 14 个问题**全部 CONFIRMED，无一驳回**（#7 部分成立：剪贴板覆盖 YES，「跳过 clear_cached_pid」NO——clear_cached_pid 在外层 paste() 不受内层早退影响）。复查方式：6 个并行 Explore agent 独立读源码 + 主审亲自核实 #3/#4。

## 1. P0 — 进程级 / 核心数据正确性

### #1. zipformer 空音频死循环 🔴

**根因**：`asr-local/engines/zipformer.rs` 的 `compute_whisper_features_linear`（:1089）和 `compute_fbank_features`（:1150）在 `samples.is_empty()` 时 `wave_dim=0`，反射 padding 循环 `while s < 0 || s >= wave_dim`（wave_dim=0 时 `s < 0 || s >= 0` 恒真）陷入 `199 ↔ -200` 二态振荡，卡死整个进程（非 panic 非 Result，无法兜底）。同源问题 qwen3 已修（`qwen3_asr.rs:697-704`），zipformer 漏修。

**修复**：两个特征函数头部加 `if samples.is_empty() { return Ok(Array2::zeros((0, Z_NUM_BINS))); }`（照搬 qwen3 guard）；两个 `transcribe`（CTC:512 / Transducer:863）特征提取后加 `if my_feats.nrows() == 0 { return Ok(String::new()); }`。

**回归测试**：`compute_whisper_features_linear_empty_does_not_hang` + `compute_fbank_features_empty_does_not_hang` + `compute_features_single_sample_no_panic`（照搬 qwen3 测试，钉死空/极短输入）。

### #2. Tencent 多句缺分隔符 🔴

**根因**：`asr-cloud/tencent_stream.rs` 3 处 `stable_segments.values().cloned().collect::<String>()`（:176/208/228）无分隔符拼接 + `format!("{}{}", stable, current_partial)`（:208）stable↔partial 无分隔。英文多句拼成 `"hello worldtoday"`。`open()` 的 `_language` 参数被丢弃（无法用 `sentence_separator`）。baidu/aliyun 均用 `sentence_separator`，tencent 零引用。

**修复**：`open()` 的 `_language` → `language`，透传给 `run_tencent_session`；函数内 `let sep = sentence_separator(&language);`；3 处拼接改为 `.collect::<Vec<_>>().join(sep)`，stable↔partial 改为 `format!("{}{}{}", stable, sep, current_partial)`。

### #3 + #4. cloud_close_error 早退吞没 + 写竞态 🔴（P2-2 自审缺陷）

**#3 根因**：`finalize_cloud`（`lifecycle.rs:474-482`）`combined.is_empty()` 早退 `return`，跳过 `:491-499` 的 `cloud_close_error` 落库。触发：开云端录音→未返回文本→close 失败（Err）→transcript+partial 空→combined 空→早退→最该捕获的 close 失败诊断被吞。

**#4 根因**：`update_transcription_raw`（`paste.rs:201-210`）异步入队（`sender.send` 即返），紧接着 `update_meta_field`（`transcription.rs:65`）同步 `with_db` 执行 → INSERT 未处理时 UPDATE 命中 0 行；且 `update_meta_field` 包装 `update_meta_field_at` 时丢弃返回行数（`:68` 直接 `Ok(())`）→ 连 warn 都不打。双重吞没。

**修复**：
1. 新增 `DbCommand::UpdateMetaField`（`db_queue.rs`，`#[cfg(feature="cloud")]` gate）+ 处理分支 → 走 DB 队列保证 FIFO（INSERT 先于 UpdateMetaField）。
2. 新增 `enqueue_cloud_close_error(id, err)` helper（`lifecycle.rs`）入队。
3. `finalize_cloud` 早退路径（combined 空）也调 `update_transcription_raw` + `enqueue_cloud_close_error`（先确保 INSERT 空记录再入队诊断）。

## 2. P1 — 数据丢失 / 跨设备一致性

### #5. Paraformer flush() 丢新段首字

**根因**：`streaming_paraformer.rs:201-250` flush() 不重置 `last_emitted_token`；去重逻辑（:444-446 `if !seen_first_valid && tid == self.last_emitted_token { continue }`）对新段首 token 误去重（旧段末 == 新段首时）。

**修复**：flush() 末尾（`fresh_segment = true` 后）加 `self.last_emitted_token = -1;`（段边界后去重上下文失效）。

### #6. hotword 级联词 is_deleted=1 早夭

**根因**：`infra/db/hotword.rs:186` `delete_hotword_set_at` 级联软删词写 `is_deleted=1`（字面量 1 = 1970），而 set 级用 `now_secs`。GC（:244 `is_deleted < cutoff`）当日硬删词 tombstone；sync（`sync/hotword.rs:126 is_tombstone_expired`）同阈值过滤→词 tombstone 不 export/pull→跨设备删除意图无法传播。

**修复**：级联词 UPDATE 改 `is_deleted=?2` + `params![id, now_secs]`（与 set 级一致）。

### #7. paste.rs self-webview 覆盖剪贴板

**根因**：`platform/paste.rs:184` self-webview 路径 emit paste-text 后 `return Ok(())` 早退，跳过底部 `restore_clipboard`（:200-204）。`write_to_clipboard=false` 时用户剪贴板被 :139 `handle.write_text` 覆盖且不还原（备份了却不还原）。报告子主张「跳过 clear_cached_pid」**不成立**（clear_cached_pid 在外层 paste() :64，不受内层早退影响）。

**修复**：self-webview 分支 `return` 前，若 `!write_to_clipboard` 则 `sleep(PASTE_RESTORE_DELAY) + restore_clipboard(handle, saved)`。

### #8. download Etag 永不校验

**根因**：`download/core/verify.rs:38-44` Etag 分支恒返 `Ok(true)`（占位），If-Range 请求层未实现（下载请求只发 Range）→ Etag 路径无完整性校验。半成品 TODO。

**修复**：Etag 分支加 `log::warn!` 让运行时可见（完整修复需在 download_segment_once 发 If-Range + 206/200 区分，留待后续）。

### #9. scheduler task panic 杀全线程

**根因**：`scheduler/lib.rs:147,161` 两处 `(task.run)()` 无 `catch_unwind`，一个任务 panic 杀死整个调度线程→剪贴板清理/bigram 索引等全停，无监控无重启。

**修复**：两处包 `std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (task.run)()))`，Ok 时 `mark_run`，Err 时 `log::error!` 继续调度。

## 3. P2 — 健壮性 / 不对称修复

### #11. keychain 缺 fsync 父目录

**根因**：`vault/keychain.rs:387` Unix 分支 `rename` 后无父目录 fsync，与 `sync/store.rs:350-356` N3 修复不对称。断电可能丢 machine_key 目录元数据。

**修复**：rename 后照搬 store.rs N3 模式加 `dir.sync_all()`（best-effort）。

### #12. rename_folder O(N) 全表扫

**根因**：`vault/storage/folder.rs:83-87` 用 `list_vault_folders().find()` 全表扫读 sort_order，与 `engine.rs` P-FOLDER-SCAN 修复（已用 `load_vault_folder(id)` O(1)）不对称。

**修复**：改用 `db::load_vault_folder(id)?.map(|f| f.sort_order).unwrap_or(0)`。

### #13. record SpawnFailed 误报

**根因**：`record/session.rs:281,293,340` 运行期 stdin write / child wait 的 IO 错误被 `.map_err(RecordError::SpawnFailed)`，展示为「helper spawn failed」但 helper 早已 spawn 成功，诊断误导。

**修复**：`error.rs` 加 `IpcWriteFailed` + `HelperWaitFailed` 变体；3 处 map_err 改用对应变体。`SpawnFailed` 保留专用于 start 阶段。

### #14. paddle-ocr ratio 覆盖非累乘

**根因**：`paddle-ocr/pipeline/image_ops.rs:37-38` 双分支 resize 第二分支 `ratio_h = rh` 覆盖第一分支，应 `*=` 累乘。两分支同时触发（用户 `min_side_len > 32`）时检测框坐标回投偏差第一段 ratio 倍。默认 `min_side_len=30 ≤ 32` 不触发。

**修复**：改 `ratio_h *= rh; ratio_w *= rw;`。回归测试：极端宽高比图（4000×8）+ `min_side_len=64` 验证累乘。

### #10. ASR transcribe 未 spawn_blocking（文档化，不修）

**现状**：`record/record_commands/postprocess.rs:514-515` 已有注释承认权衡（"engine.transcribe 内部已并发""为简化暂不 spawn_blocking，如发现卡 UI 再包"）。确属已知性能债，非 bug。维持现状。

## 4. 验证

全量编译（默认 + cloud feature）0 error 0 warning；8 个受影响 crate + desktop（cloud）共 **1360 测试全过**：
- asr-local 169（+3 空输入测试）/ asr-cloud 57 / infra 178 / pty 32 / vault 258 / paddle-ocr 47（+2 ratio 测试）/ download 33 / record 50 / desktop 536

## 5. 第三轮审查修复（2026-08-03，P1-1～P2-8）

第三轮全新核查发现：多处修复存在「未覆盖的同类兄弟路径」或「修复引入的副作用」。全部 CONFIRMED 并修复（P1-3 / P2-7 文档化为已知遗留）。

### P1-1. #3 修复漏洞：cloud_close_error 空文本早退仍丢失 🔴

**根因**：#3 修复在 `finalize_cloud` 空文本早退路径调 `update_transcription_raw` 想插空记录，但该函数（`paste.rs:198-200`）在 `transcript.full().is_empty()` 时第一行就 `return Ok(())` 不入队 Insert → `enqueue_cloud_close_error` 的 UPDATE 命中 0 行 → 诊断仍丢失。恰是最该捕获的场景（云端彻底失败）。

**修复**：空分支不借道 `update_transcription_raw`，直接 `sender.send(DbCommand::Insert { text: "", ... })` 强制建空记录，再入队 cloud_close_error。

### P1-2. paste Cmd+V keystroke 失败不还原剪贴板 🔴

**根因**：`paste_via_clipboard` 的三处 keystroke `?`（osascript / paste_to_pid / paste）失败时直接 bail，跳过底部 `restore_clipboard`。AX 权限被撤销时用户原剪贴板（已被 `write_text` 覆盖）永久丢失。#7 只修了 self-webview 早退分支，Cmd+V 主路径漏。

**修复**：keystroke 结果存入 `inject: Result<()>`，失败时先 `sleep + restore_clipboard`（wtc=false 时）再传播错误。

### P1-4. hotword add_word_to_set_at 容量校验顺序错 🔴

**根因**：先 `ensure_within_capacity` 后查 `already_active`（注释意图相反）。词典满（3000）+ 加已存在词 → 撞容量错误而非幂等 `Ok(false)`。

**修复**：查重移到容量校验前——先查 `already_active`，已活跃则 `Ok(false)`，再校验容量。

### P2-1/P2-2. aliyun FunASR/Qwen partial 漏句间分隔符 🟠

**根因**：aliyun 两处 partial 拼接（FunASR `:215` / Qwen `:490`）`format!("{}{}", committed, current_sentence)` 漏 sep。commit 分支有 sep 守卫，partial 漏。#2 只修 tencent，aliyun 同型漏修。仅实时显示粘连，commit 自愈。

**修复**：两处 partial 拼接补 sep 守卫（参照 tencent 的 `format!("{}{}{}", stable, sep, partial)`）。

### P2-5. #9 副作用：scheduler panic 不 mark_run → 每 tick 重试 🟠

**根因**：#9 的 catch_unwind Err 分支只 log 不 mark_run → `is_due()` 下个 tick 仍 true → deterministic panic 每 tick 重跑+刷日志。

**修复**：Err 分支也调 `mark_run()`（避免每 tick 重试，给问题任务 interval_secs 冷却期）。

### P2-6. EXCLUDED_PREFIXES 漏 action_bar_window 🟠

**根因**：`focus_tracker.rs` 的 `focused_self_webview_label` 用黑名单（EXCLUDED_PREFIXES）排除浮窗，漏 `action_bar_window`/`record_*`/`password_generator_window`。action_bar 聚焦（makeKeyAndOrderFront 夺焦）时 ASR paste emit_to 无 listener → 转写文本静默丢失。

**修复**：黑名单改白名单（`has_paste_text_listener`）——仅 `terminal_*`/`terminal_action_agent`/`compact_editor_window` 接收 paste-text。新增窗口默认不接收，防未来漏配。

### P2-3/P2-4. hotword set_words/add_words 容量校验语义错 🟠

**根因**：`set_words_in_set_at`（覆盖语义）误用追加校验（cur+adding）；`add_words_to_set_at`（批量追加）用 unique.len() 含已活跃词。合法操作被误拒。

**修复**：set_words 改用 `unique.len() > MAX` 直接判（覆盖后总量=unique.len()）；add_words 先查已活跃词集合，只对 `only_new.len()` 校验。

### P2-8. download resume.rs 原子写缺 fsync 🟠

**根因**：`resume.rs:save` 的 tmp+rename 无 fsync，与 keychain #11 / store write_atomically 不对称。断电恰在 rename 后 → sidecar 空/半 → JSON 解析失败 → 续传进度丢失。

**修复**：补 `f.sync_all()`（文件本体）+ `dir.sync_all()`（父目录，best-effort），照搬 keychain #11 模式。

### 中低项：PasteMethod::None 忽略 wtc

**根因**：None 分支调 `write_to_clipboard`（不接 wtc），无视 `write_to_clipboard=false` 直接覆盖剪贴板。

**修复**：None 分支也判 `if wtc { write_to_clipboard(...) }`（None 语义=不模拟按键，但是否写剪贴板仍尊重 wtc）。

### 文档化（不修）

- **P1-3 vault permanent_delete + sync 复活**：permanent_delete 只删 DB 行不写 tombstone → 他机 sync push 复活。已有废弃 tombstone spec（`2026-07-26-vault-tombstone-design.md`），当前 is_deleted merge 只解决软删。完整修复需引入 cipher tombstone（对称热词），设计层面大改，留待后续。
- **P2-7 merge_hotwords N×M 全表扫**：每 set 调 `list_all_hotword_words()` 全表扫+filter。3 万词条 × N 词典 ~0.5s，sync 低频可接受。优化方案（入口一次查询按 set_id 分组）留待后续。
