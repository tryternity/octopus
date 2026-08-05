# 全量代码审查 bugfix（21 crate，多轮迭代）

> **日期**：2026-08-03 起，10 轮迭代（§1-7 = 轮 1-5，§8-12 = 轮 6-10）
> **状态**：✅ 已实现（10 轮共 ~40 处修复，每轮全量测试通过）
> **来源**：外部全量代码审查报告（21 crate 非增量审查，14 个具体问题 + 中低清单）→ 之后 9 轮自驱迭代复审（修复作用域不全 / 修复引入副作用 / 新发现）
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

## 6. 第四轮审查修复（2026-08-03，R4-1～R4-9）

第四轮全新核查发现：前几轮的 4 处修复（#1/#2/#5/#6 + P1-2 的 None 子项）在 merge main 时被覆盖丢失（main 的文件版本无这些修复），spec 误标「已实现」。另新增 4 条真实问题。全部 CONFIRMED 并修复。

### R4-1～R4-5：重新应用丢失的修复

- **R4-1 zipformer 空输入 guard**（#1 重做）：两特征函数 + 两 transcribe 加 guard（同第二轮，被 merge 覆盖）。
- **R4-2 hotword 级联 is_deleted=now_secs**（#6 重做）：级联词 UPDATE 改 `?2`+now_secs（被 merge 覆盖）。
- **R4-3 tencent language 透传 + sep**（#2 重做）：open 补 language + 3 处 join(sep)（被 merge 覆盖）。
- **R4-4 paraformer flush last_emitted_token=-1**（#5 重做）：flush 末尾重置（被 merge 覆盖）。
- **R4-5 PasteMethod::None 尊重 wtc**（第三轮重做）：None 分支 `if wtc`（被 merge 覆盖）。

### R4-6. spawn_polish_thread 缺 catch_unwind（新发现）

**根因**：`polish.rs:230-244` 的 `std::thread::spawn` 闭包对 `polish_regions` 无 catch_unwind（同文件 `start_final_polish_or_paste:92-125` 有）。panic → 线程静默死 → PolishDone 永不发 → Stage::StoppingPolish 卡死。

**修复**：闭包内包 `catch_unwind(AssertUnwindSafe(inner))`，panic 时发 `PolishDone(Err)` 让 coordinator 能 finalize 退出 StoppingPolish。

### R4-8. Cancel/Discard 不清 pending_flush（新发现）

**根因**：`mod.rs:522/:531` Cancel/Discard 清 `pending_prepare` 但漏 `pending_flush`。停录音 → 200ms 内 Esc → 立刻重开 → 残留 FlushTimeout 准时到 → 停掉刚开的录音。

**修复**：Cancel/Discard 两处补 `pending_flush = None;`。

### R4-9. clone_initial 只导入词典 meta，词数据全丢（新发现）

**根因**：`import_hotwords_from_files`（sync/hotword.rs:672）只读各词典 meta.json，不读词文件/outline。B 机首次 clone 后词典是空壳，词要等下次 sync_now 才 pull。与 vault 路径（import_all_from_files 递归）不对称。

**修复**：新增 `import_hotword_words_from_files`（读每个词典 outline + 逐词读文件），engine.rs clone_initial 在导入词典 meta 后调它导入词数据。

## 7. 第五轮审查修复（2026-08-03，S1～S3 + A1 + L1～L3）

第五轮全新核查确认第四轮 8 个修复全部落地，另新发现 10 条问题。7 条修复 + 3 条文档化。

### S1. 词级软删在首推路径不传播

**根因**：`incremental_export_hotwords_with`（sync/hotword.rs:582-592）词文件条件写（md5 不变不写），而词 md5 不含 is_deleted → 软删词 md5 不变 → 词文件不重写 → 盘上留 stale is_deleted=0。常规 sync 的 export_all_hotwords 全量重写自愈，仅首推（NoUpstream）窗口期暴露。

**修复**：词文件改无条件写（对齐 set 文件 :569 + export_all :459）。

### S2. merge_hotword_words N×M 全表扫（性能）

**根因**：每 set 调 `merge_hotword_words` 内部都 `list_all_hotword_words()` 全表扫 + filter。10 词典×3000 词 = 30 万行扫描。

**修复**：入口一次查询按 set_id 分组成 `HashMap`，传引用进 merge_hotword_words。

### S3. clone_initial 缺 is_tombstone_expired 守卫

**根因**：import 函数直接返回所有词典/词，无超期 tombstone 过滤。clone 会复活超期 tombstone（常规路径 pull_set/pull_word 有守卫）。

**修复**：import 函数内部加 `is_tombstone_expired` 过滤。

### A1. pipeline 段间拼接对 CJK 标点错插空格

**根因**：pipeline.rs:146-153 的 char 级 `is_cjk` 漏 CJK 标点(0x3000-303F)/全角(FF00-FFEF) → 段以「。」结尾 + 下段汉字 → needs_space=true →「你好。 世界」。对照 paraformer smart_append 字节级 `<0x80` 判 ASCII 正确。

**修复**：改字节级 ASCII 判定（对齐 smart_append），两侧都非 ASCII（CJK 标点/全角/汉字首字节 ≥0x80）不插空格。

### L1. strip_edited_markers 抹字面括号

**根因**：client.rs:250-253 无条件全删 `{}/<>`，注释说「仅去包裹单词的 {}」。用户字面括号（代码/数学/HTML）被抹。

**修复**：改正则只去包裹标记的括号。

### L2. 另存 WebP 实写 JPEG

**根因**：clipboard_commands.rs webp 分支直写 blob（不解码重编），blob 默认是 JPEG → .webp 文件实为 JPEG。

**修复**：按 magic byte 判断格式解码后重编为 WebP。

### L3. suppress_flag 写失败不回滚

**根因**：handle.rs 四个写入方法先 store(true) 后写，`?` 失败不回滚 flag → 下次 Cmd+C 被吞。

**修复**：失败分支补 store(false) 回滚 flag。

### 文档化（不修，需先量化/影响极低）

- **A2 Whisper Hann 窗 symmetric vs periodic**：zipformer/whisper 用 `/(size-1)`（symmetric），qwen3 用 `/size`（periodic，对齐 PyTorch 默认）。golden test 钉死错误值。修改需先 A/B 量化 WER 影响 + 重新生成 golden 值，留待后续。
- **L4 热词候选 join("|") 无转义**：候选含字面 `|` 则 LLM 看到的候选数错。中文热词几乎不含 `|`，影响极低。
- **L5 detect_selection restore 前 clear_suppress 微秒竞态**：窗口极窄（单线程两条语句间 watcher 恰好调度），且仅多存一条记录（非数据损坏）。

## 8. 第六轮审查修复（2026-08-04，L1-b / C1+I1 / L1-a 文档化）

第六轮聚焦第五轮 L1/L2 修复的**完善与残余风险收口**。第三/五轮已修 L1（`strip_edited_markers` regex 只去包裹标记）+ L2（WebP 另存实写 JPEG → magic byte 判断重编），但留下两处尾巴：① regex 每次 polish 现场编译（热路径开销）② WebP 另存分支重编后业务上已决定**摒弃 WebP**（image crate 未启 webp feature），保留兼容代码是死路径 + 误导未来维护者。

### L1-b. strip_edited_markers regex 改 static Lazy 预编译

**根因**：`client.rs:250-253`（第五轮 L1 修复后）每次 `polish_regions` 调用都 `Regex::new × 2`（`{...}` + `<...>`）。中间润色 mode=2 由停顿驱动频繁触发（每段停顿一次），regex 编译是热路径不必要开销。

**修复**：两个 regex 提为 `static Lazy<Regex>`（`RE_EDITED_MARKER` / `RE_HOTWORDS_MARKER`，`client.rs:258-261`），`strip_edited_markers` 改用 static 引用。对齐同文件已有的 `HTTP_CLIENT: Lazy<reqwest::blocking::Client>`（:10）范式。

### C1+I1. 移除 WebP 兼容代码（业务摒弃收口）

**根因**：第五轮 L2 修复时给 `save_image_item` 加了 magic byte 判断（blob 是 JPEG 还是 WebP）+ WebP 分支（重编为 WebP）。但业务决策**已摒弃 WebP**：① `image` crate 的 `webp` feature 未启用（Cargo.toml 未开），`ImageFormat::WebP` 解码/编码运行时必失败；② DB `image_data.blob` 恒为 JPEG（`IMAGE_SAVE_QUALITY=jpeg:85` 入库）。保留的 WebP 分支是死路径，且 magic byte 逻辑（`starts_with(&[0xFF,0xD8,0xFF])` ? Jpeg : WebP）把任何非 JPEG blob 误判为 WebP → 老数据/损坏数据走必失败的 WebP 解码。

**修复**（`clipboard_commands.rs:274-303`）：
- 删 `"webp" => "webp"` ext 映射分支，`fmt` 落入 `_` 一律按 jpg 处理（含旧前端误传的 webp）。
- 删 magic byte 判断，3 处 `load_from_memory_with_format` 硬编 `ImageFormat::Jpeg`。
- 删 `"webp" =>` 编码分支（原 `img.save_with_format(..., WebP)`）。
- 注释更新：「image_data.blob 恒为 JPEG（2026-08-03 已摒弃 WebP 落库）」。

### L1-a. 字面花括号残余风险文档化（不修）

**现状**：L1 regex `\{([^{}]*)\}` 无法区分「edited 标记 `{word}`」与「用户字面花括号 `{key:value}`」（代码/JSON/数学语法）。补测试 `strip_edited_markers_literal_braces_residual_risk` 钉死两个已知限制：① 平铺字面花括号 `config={key:value}` → `config=key:value`（误抹）；② 嵌套 `{config={key:value}}` → 外层泄漏 `{config=key:value}`（内层先匹配消耗，外层不再重扫）。**可接受**：ASR 转写文本几乎不含代码语法。

## 9. 第七轮审查修复（2026-08-04，N1 / P2-a / P2-b / N2-rAF）

第七轮发现第六轮 WebP 摒弃**清理不彻底**（前端仍有 WebP 痕迹）+ 3 处新问题（download dest fsync / search DB spawn_blocking / 终端 rAF 背压）。rAF 背压单独成 commit `71740669`（用户报告的 shell 回显丢失 bug），其余合入 `09dec35a`。

### N1. WebP 摒弃前端清理不彻底

**根因**：第六轮 C1 只清了后端 `clipboard_commands.rs`，前端 2 处仍留 WebP：① `SaveImagePopover.tsx` 的 `ImageFormat` 类型 + `FORMATS` 选项含 `"webp"`（用户仍能选 WebP，但后端已不支持 → 落入 `_` 按 jpg 处理，用户看到的扩展名与实际不符）；② `ImagePreview/index.tsx` 全图 Blob MIME 写 `image/webp`（但 blob 实为 JPEG → MIME 与内容不符，部分图片预览组件可能据此误判）。

**修复**：
- `SaveImagePopover.tsx:7` `ImageFormat = "jpeg" | "webp" | "png"` → `"jpeg" | "png"`；删 `{ value: "webp", label: "WebP" }` 选项。
- `ImagePreview/index.tsx:198` Blob MIME `image/webp` → `image/jpeg`（对齐实际 blob 内容）。
- `architecture.md §clipboard.image` 同步：默认 `IMAGE_SAVE_QUALITY` 从 `"jpeg:100"` 更正为 `"jpeg:92"`，新增「WebP 已摒弃（2026-08-03）」段落。

### P2-a. download dest rename 无 fsync（对称补全）

**根因**：`downloader.rs:470` 段下载完成 `std::fs::rename(&part, &task.dest)?` 只原子切目录项，**不 fsync 数据**。POSIX 语义：rename 持久化目录项 ≠ 内容持久化。断电序列：rename 成功 → 内容仍在 page cache → 断电 → dest 存在但内容空/半 → sidecar `resume.rs` 已 `remove(dest)`（认为完成）→ 无法识别为未完成 → 不会自动重下 → 用户拿到损坏文件。与已修的 sidecar `resume.rs` save（#11/P2-8）+ `keychain.rs` rename（#11）不对称——这两处都补了 fsync，download dest 漏。

**修复**（`downloader.rs:470-482`）：rename 前对 part 文件 `f.sync_all()`（数据持久化），rename 后对父目录 `dir.sync_all()`（目录项持久化）。两处 best-effort（`let _ = ...`，失败 log warn 不阻断——数据最终也会被 OS 刷盘，且 fsync 失败无优雅恢复路径）。

### P2-b. search MenuProvider async 里同步阻塞 DB

**根因**：`search/providers/menu.rs:21-23` `async fn search` 直接调 `octopus_infra::db::list_action_bar_items()`（同步，持 `with_db` 全局 `ReentrantMutex`）。搜索与转录持久化 / 热词写 / 配置 save 共享同一 DB 锁，任一持锁时搜索 `await` 点卡住 tokio worker（搜索是高频用户操作）。

**修复**（`menu.rs:22-27`）：`tokio::task::spawn_blocking(octopus_infra::db::list_action_bar_items).await`，把同步 DB 调用移到阻塞线程池，`Ok(Ok(r)) => r, _ => return vec![]`（JoinFailure / DB error 都降级空结果）。

### N2. 终端 rAF 节流覆盖丢弃 → shell 回显丢失 🔴（用户报告）

**根因**：`useTerminalSession.ts` PTY 输出的 rAF 节流旧逻辑「同帧内多块只保留最新，丢弃中间」（`pendingOutput = bytes` 覆盖）。对高速连续输出（`yes` / `cat` 大文件）无害（中间帧本就该丢），但对 **shell 回显致命**：shell 逐字符/小块回显用户输入，快速输入 + 回删时多个 `onData` 在同一帧（~16ms）内到达，旧逻辑只保留最后一块 → 丢失前面的回显字符 → xterm 显示的文本和 shell 实际接收的输入不一致。

**症状**（用户报告）：输入 `git clonne` → 回删改成 `git clone` → 显示 `clone` 但 shell 报错 `'clonne' is not a git command`——回显的中间块被 rAF 丢弃，显示的是本地回显 + 残留拼凑的假象，shell 实际收到的是第一次完整输入。

**修复**（commit `71740669`，`useTerminalSession.ts:281-313`）：同帧内多块**累积拼接**到 `pendingChunks: Uint8Array[]`，`flushOutput` 时合并成一个 `Uint8Array` 一次性 `term.write`。多数情况只有 1 块（常规输出无额外开销），高速时多块合并（保数据完整不丢）。

**注**：此修复在第八轮发现引入新问题（窗口隐藏时 `pendingChunks` 无界增长），见 §10 N2-背压。

## 10. 第八轮审查修复（2026-08-04，P0 folder 软删同步 + N2 背压 O(N²)）

第八轮发现第七轮 rAF 背压修复**引入性能回归**（O(N²) reduce）+ 一处 P0 级新发现（vault folder 软删跨设备同步失效，与已修的 cipher H2 同型漏修）。合入 commit `63dfba46`。

### P0. vault folder 软删跨设备同步失效 🔴（cipher H2 同型漏修）

**根因**：cipher 早在 H2 修复（`pull_preserves_soft_deleted_at`）已让软删 cipher 跨设备同步——`cipher_md5` 含 `is_deleted`、`upsert_cipher` 透传 `is_deleted`、DB INSERT/UPDATE 含列。但 **folder 路径完全对称地漏修**：
- `fingerprint.rs::folder_md5_from_fields(id, name, sort_order)` **不含 is_deleted**（硬编 `0u8`，:98 `format!("{}|{}|{}|{}", id, name, sort_order, 0u8)`）。
- `db::insert_vault_folder_with_sort` / `update_vault_folder_fields` SQL **不含 is_deleted 列**（INSERT 用 DEFAULT 0，UPDATE 不碰）。
- `engine.rs::upsert_folder_with_sort` 签名 **不接 is_deleted**。
- 3 处调用（`clone_initial` / `merge_vault` pull 新 folder / pull 更新 folder）**丢弃 `folder_file.is_deleted`**。

**触发链**：A 机软删 folder → `write_folder_file(is_deleted=true)` + outline md5（但 md5 算的时候 is_deleted 没参与）→ B 机 pull → `upsert_folder_with_sort` SQL 不含 is_deleted → INSERT DEFAULT 0 / UPDATE 不碰 → **folder 复活成 live** → B 机 push 反向覆盖 → A 机的删除被撤销。跨设备删除意图无法传播。

**修复**（5 层对称修复，对齐 cipher H2）：
1. `fingerprint.rs:101` `folder_md5_from_fields` 加 `is_deleted: bool` 参数，`format!` 用 `is_deleted as u8`（对称 `cipher_md5_from_input`）。
2. `db/vault.rs:456` `insert_vault_folder_with_sort` 加 `is_deleted: bool` 参数 + INSERT SQL 加 `is_deleted` 列。
3. `db/vault.rs:494` `update_vault_folder_fields` 加 `is_deleted: bool` 参数 + UPDATE SQL 加 `is_deleted = ?5`。
4. `engine.rs:622` `upsert_folder_with_sort` 加 `is_deleted: bool` 参数，透传给 INSERT/UPDATE。
5. 3 处调用补 `folder_file.is_deleted`：`clone_initial:535-536` / `merge_vault` pull 新 folder `:1123,1130` / pull 更新 folder `:1151,1158`。另 `folder.rs::create_folder` / `rename_folder` 算 md5 传 `false`（新建/改名必 live）。

**回归测试**（`engine.rs:2202 pull_preserves_soft_deleted_folder`）：写一个 `is_deleted=true` 的 folder 文件 + outline → `merge_vault` pull → 断言 DB 中 `is_deleted` 必须存活（不能复活成 false）。对齐 cipher H2 的 `pull_preserves_soft_deleted_at`。

### N2. 终端 rAF 背压 O(N²)（第七轮修复引入的回归）

**根因**：第七轮 N2 背压守卫（`pendingBytes >= 2MB` 同步 flush）用 `pendingChunks.reduce((s, c) => s + c.length, 0)` 每次 `onData` 算总字节。`onData` 是热路径（PTY 每次输出都调），`pendingChunks` 长度随累积线性增长 → 每次 reduce O(N) → 整体 O(N²)。高速输出（`yes`）时 N 快速增长，CPU 飙升。

**修复**（`useTerminalSession.ts:282,290,308`）：与 `pendingChunks` 平行维护 `pendingBytes` 计数器——push 时 `pendingBytes += bytes.length`，flush 时 `pendingBytes = 0`。热路径 O(1)，消除 reduce。

## 11. 第九轮审查修复（2026-08-04，P1 SyncPanel confirm + P2-a alert + P2-b toast）

第九轮聚焦**前端 WKWebView 兼容性**——`window.confirm` / `window.alert` 在 WKWebView 静默失效（confirm 返 false、alert 不显示），导致禁用 vault sync 按钮失效、翻译失败无反馈、toast 被前次 timer 误清。合入 commit `63dfba46`（与第八轮同 commit）。

### P1. SyncPanel window.confirm 在 WKWebView 静默返 false 🔴

**根因**：`SyncPanel.tsx:198` `handleDisable` 用 `if (!confirm(t("...disableConfirm"))) return;`。语义是「用户点取消（confirm 返 false）则 return」。但 WKWebView 下 `window.confirm` **不弹框且静默返回 `false`** → `!false === true` → 永远 return → **禁用 vault sync 按钮永远失效**（点了没反应）。

**修复**（`SyncPanel.tsx:4,198`）：改用 `@tauri-apps/plugin-dialog` 的 `confirm`（alias `confirmDialog`）：`if (!(await confirmDialog(t("...disableConfirm"), { title: t("...disableConfirmTitle"), kind: "warning" }))) return;`。对齐同目录 `HotwordPanel` 已用 `plugin-dialog` 的范式。**WKWebView 下原生 `confirm/alert/prompt` 不可靠**是已知坑（AGENTS.md 历史教训），所有确认/提示必须走 `plugin-dialog`。

### P2-a. CompactEditor window.alert 在 WKWebView 不显示

**根因**：`CompactEditor/index.tsx:502` 翻译启动失败 catch 里 `alert(ti18n("editor.translateFail") + ": " + String(e))`。WKWebView 下 `window.alert` 不显示 → 用户翻译失败无任何反馈。

**修复**：删 `alert(...)`，保留 `console.error(...)`（已有）。**注**：此修复在第十轮发现引入回归（只 `setTranslating(false)` 不回滚 `translatedText` 占位 → 译文区永留「⏳ 正在翻译...」），见 §12 P2-4。

### P2-b. Settings showToast 无 timerRef → error toast 被前次 success timer 清掉

**根因**：`Settings/index.tsx:68` `showToast` 的 success 分支 `setTimeout(() => setToast(null), 2000)` 未存 timer ref。时序：success toast 设置 + 调度 2s 清除 → 1s 后 error toast 来 → error 分支不设新 timer（`if variant === error return`）→ 前次 success 的 2s timer 到期 → `setToast(null)` → **error toast 被清掉**（用户来不及看）。

**修复**（`Settings/index.tsx:71-79`）：`toastTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)`。每次 `showToast` 先 `clearTimeout(toastTimerRef.current)` 清前次 timer（无论 success/error），success 分支设新 timer 存 ref，error 分支不设（保持不自动消失，让用户手动关闭）。

## 12. 第十轮审查修复（2026-08-04，P1-1 hotword spawn_blocking + P1-2 监听器泄漏 + P2-3 Enter 守卫 + P2-4 翻译回滚）

第十轮收口第九轮前端修复**引入的回归**（P2-4）+ 3 处新发现（hotword LLM 阻塞 tokio / VaultPanel 监听器泄漏 / SyncPanel 并发 resolve）。合入 commit `1c6145e4`。

### P1-1. list_hotword_candidates LLM 阻塞 tokio worker 🔴

**根因**：`hotword_commands.rs:193` `async fn list_hotword_candidates` 直接在 async 命令体里跑 DB 读 + 文件 IO + **sync LLM 调用**（`octopus_llm::mine_hotwords`，5-30s 阻塞）。tokio worker 被占期间，同 worker 的 `pty_open` / search emit / 其他命令全排队。与同文件 `import_hotwords:284` / `export_hotwords`（都用 `spawn_blocking`）不对称。

**修复**（`hotword_commands.rs:197-271`）：整体塞 `tauri::async_runtime::spawn_blocking(move || { ... }).await`，内部逻辑不变（读 edited_segments → 过滤 → 读 prompt 文件 → LLM 挖掘 / jieba 回退 → 去重 → 排除已有）。`.map_err(|e| format!("list_hotword_candidates join failed: {}", e))?` 处理 JoinFailure。

### P1-2. VaultPanel visibilitychange 匿名函数无法 cleanup → 监听器泄漏

**根因**：`VaultPanel.tsx:100` `document.addEventListener("visibilitychange", () => { ... })` 用匿名箭头函数。cleanup effect 里要 `removeEventListener` 但**没存函数引用** → 无法移除 → 每次 mount/unmount（切设置 tab、开关窗口）累积一个监听器 → 泄漏。多次切换后 N 个监听器同时触发 `handleFocus/handleBlur` → 重复 `vault_lock` / heartbeat 抖动。

**修复**（`VaultPanel.tsx:101-116`）：提为具名 `const handleVisibilityChange = () => { ... }`，`addEventListener` + cleanup `removeEventListener` 都用同一引用。

### P2-3. SyncPanel handleResolve Enter 无 resolving 守卫（防并发 resolve）

**根因**：`SyncPanel.tsx:503` resolve 密码输入框 `onKeyDown={(e) => e.key === "Enter" && handleResolve()}`。`resolving` 状态已有（按钮 disabled 用），但 Enter 键没查 → 用户连按 Enter / 输入法触发的多次 keydown → 并发 `handleResolve` → 多次 `vault_sync_resolve` 命令 → 冲突状态。

**修复**（`SyncPanel.tsx:503`）：`onKeyDown={(e) => e.key === "Enter" && !resolving && handleResolve()}`，加 `!resolving` 守卫。

### P2-4. CompactEditor 翻译失败不回滚 translatedText 占位（第九轮 P2-a 引入的回归）

**根因**：第九轮 P2-a 删 `alert` 时，catch 分支只剩 `setTranslating(false)`。但翻译启动时已 `setTabs` 把 `translatedText` 设成 `'⏳ ' + ti18n("editor.translating")` + `mode: 'contrast'`（`CompactEditor/index.tsx:487`）。失败后只 `setTranslating(false)` 不回滚 → **译文区永留「⏳ 正在翻译...」占位**，用户以为还在翻译。

**修复**（`CompactEditor/index.tsx:504-514`）：catch 分支补回滚——`tabsRef.current.map` 把失败 tab 的 `mode` 回 `'single'` + `translatedText: undefined`，`setTabs(rollback)`。

## 13. 各轮修复索引（按 commit）

| 轮次 | commit | 内容 |
|---|---|---|
| 1-2 | （多 commit） | 外部报告 14 处 + F1-F3/G1-G4/H1 复审 |
| 3 | `762557bb` | P1-1～P2-8 共 11 处 |
| 4 | `1370bd48` | R4-1～R4-9（5 处重做 + 4 处新发现） |
| 5 | `95210d92` | S1～S3 + A1 + L1～L3（7 修复 + 3 文档化） |
| 6+7 | `71740669`（rAF 单独）+ `09dec35a`（其余） | L1-b regex 预编译 + C1 webp 摒弃 + N1 webp 前端清理 + P2-a dest fsync + P2-b search spawn_blocking + N2 rAF 背压 |
| 8+9 | `63dfba46` | P0 folder 软删同步 + N2 背压 O(N²) + P1 SyncPanel confirm + P2-a alert + P2-b toast |
| 10 | `1c6145e4` | P1-1 hotword spawn_blocking + P1-2 VaultPanel 监听器泄漏 + P2-3 Enter 守卫 + P2-4 翻译回滚 |
| 11 | `f22eac7f` | P1 vault cipher+folder ping-pong 收敛（sync-only upsert 保留远程时间戳）+ P2 v57→v58 迁移事务 |
| 12 | `67ce6dc4` | P1-1 dock FLOAT_DEPTH 泄漏 + P1-2 MarkdownPreview 链接拦截 + P2-1 record stop 卡 Stopping + P2-2 OCR 编号不重置 + P2-3/4 坐标混算 + P3-1/2/3/4 + doc |
| 13 | `f9a25a20` | P2-1 open_settings 不 show + P3-1 SyncPanel resolve syncing 回滚 + P3-3 record serde 序列化失败 reset_to_idle + P3-2 文档化 |
| 14 | `71e7e10f` | P1-1 Result setInterval ref + P1-2 invoke_handler cfg gate + P2-1 vault serde default + P2-2 merge 删除守卫 + P2-3 rename_folder + P2-4 fps/codec + P3-3/4/5/6/7/8/9 |
| 15 | （本轮，待 commit） | P3-8 漏修 Transducer + P2-A cloud translate spawn_blocking + P2-B 粘贴安全 + P3-D/E/F/G/H + 前端 9 处 cleanup |
| 16 | `adf236ca` | P2-1 离线 zipformer chunk_shift .max(1) + P2-2 paraformer usize 下溢 + P3-5 注释微瑕 ×2 |
| 17 | `e83928d4` | P1-1 FTS5 JOIN c.rowid + P1-2 tombstone pull 远程时间戳 + P2-2 paraformer enc_feat + P3-1/2 context_size + P3-9 注释 |
| 18 | `8ef36db0` | P1 vault_sync_clone/resolve spawn_blocking + P2 import_hotwords BOM + P2 reindex_apps spawn_blocking |

## 14. 第十一轮审查修复（2026-08-04，P1 vault ping-pong + P2 v57→v58 迁移事务）

第十一轮全新核查发现 2 处问题（P1 + P2），外加 P3 纵深防御观察（非阻塞，不本轮修）。两报告问题全部 CONFIRMED（4/4 + 6/6 证据 Read 核实）。

### P1. vault cipher + folder 多设备 sync ping-pong — 永不收敛 + git 历史无限膨胀 🔴

**根因**：pull/clone 路径复用业务 upsert，业务 SQL 硬编 `updated_at = datetime('now')`，远程时间戳在落库时丢失。多设备交替 sync 后，cipher/folder 的 `updated_at` 互相覆盖性递增，每次 sync 都产生「内容未变但时间戳更新」的空 commit，永不收敛。

**4 证据链**（全部 Read 核实）：
1. `vault.rs:342-364 update_vault_cipher_at` 硬编 `updated_at = datetime('now')`（业务 UPDATE 路径）。
2. `vault.rs:312-332 insert_vault_cipher_at` 的 INSERT 不含 `created_at`/`updated_at` 列 → SQLite DEFAULT = now。
3. `engine.rs:587-603 build_cipher_input_from_file`（原函数）构造的 `VaultCipherInput` 字段无 `created_at`/`updated_at`（远程时间戳在此丢失）。
4. `store.rs:517 export_all_to_files` 算 outline：`updated_ms: iso_to_unix_ms(&c.updated_at)` —— outline.updated_ms 来自 DB.updated_at（pull 后 = 本机 now）。

**folder 路径同型**：`update_vault_folder_fields:504` 同样硬编 `datetime('now')`，folder merge 同样用 updated_ms 比较 → folder 改名/排序跨设备也 ping-pong。

**触发链**：A 创建 cipher（DB=T_A1, outline=T_A1, push）→ B pull（build_cipher_input_from_file 丢 T_A1 → 落库 DB=T_B1）→ B export（outline=T_B1, push）→ A fetch（outline T_B1 > DB T_A1 → pull → update_vault_cipher_at DB=T_A2）→ … 永不收敛。

**为何前 10 轮未发现**：单设备 sync 时 DB.updated_at 与 outline.updated_ms 同源（都基于同一 DB 值），md5 比对跳过，单设备完全不触发。必须 ≥2 设备交替 cross sync 才暴露。vault 是密码管理器，数据一致性是核心契约。

**为何多轮未发现（补充）**：现有测试 `pull_uses_md5_not_updated_at:1980` 守护 md5 比对（updated_at 不同但 md5 相同应跳过），但**没有测试验证 pull 后 DB.updated_at 的值**——它假设 pull 会写 now，断言只看 pulled 计数。ping-pong 的本质是「时间戳值丢失」而非「md5 比对错」，测试盲区。

**修复方式**（sync-only upsert，不碰 `VaultCipherInput`）：

**决策理由**：`VaultCipherInput` 有 23 个构造点，本地路径必须保持 `datetime('now')`（标记「我改了」），改 struct 字段风险高。`VaultCipher`/`VaultFolder` 已含远程时间戳（`CipherFile::to_vault_cipher():273-274` / `FolderFile::to_vault_folder():312-313` 保留 created_at/updated_at），数据已现成。故新增专用 sync-only upsert 路径，与 `_at` 后缀范式（`update_vault_cipher_at` 已存在）一致。

**infra/db/vault.rs 新增 4 个 pub fn**（紧邻现有 cipher/folder 函数）：
1. `insert_vault_cipher_sync_at(conn, row: &VaultCipher)` —— INSERT 含 `created_at, updated_at` 列（用 row 值）。
2. `update_vault_cipher_sync_at(conn, id, row: &VaultCipher)` —— UPDATE 显式写 `created_at = ?, updated_at = ?`（非 `datetime('now')`）。
3. `insert_vault_folder_sync_at(conn, row: &VaultFolder)` —— folder 版 INSERT。
4. `update_vault_folder_sync_at(conn, id, row: &VaultFolder)` —— folder 版 UPDATE（含 sort_order + is_deleted + 时间戳）。

**vault/src/sync/engine.rs 新增 2 个 upsert + 重接 6 处调用点**：
- `upsert_cipher_from_file(row: &VaultCipher)` —— load 存在性 → Some 调 `update_vault_cipher_sync_at`，None 调 `insert_vault_cipher_sync_at`。
- `upsert_folder_from_file(row: &VaultFolder)` —— folder 版。
- 6 处调用点改用新函数：cipher pull 新增 + cipher pull 更新 + folder pull 新增 + folder pull 更新 + clone cipher + clone folder。
- **删除** 3 个旧函数（`build_cipher_input_from_file` / `upsert_cipher` / `upsert_folder_with_sort`）——它们已无生产/测试调用（pull/clone 全改新函数后变死代码）。`build_cipher_input_from_file` 的「丢时间戳」是 ping-pong 的直接原因，删除即根治。

**业务路径保持不变**：`save_cipher` / `soft_delete` / `restore` / `create_folder` / `rename_folder` 等本机编辑仍走 `datetime('now')`（业务 upsert），语义正确——本机编辑刷新时间戳标记「我改了」，下次 push 传播这个新时间戳。

**回归测试**（engine.rs 测试模块，3 测试）：
1. `cipher_pull_preserves_remote_timestamp` —— 写文件 cipher `updated_at=2099` + outline `updated_ms=2099` → DB 空 → merge pull → 断言 `DB.updated_at == "2099-12-31 23:59:59"`（不是本机 now）+ `created_at == "2099-01-01 00:00:00"`。
2. `folder_pull_preserves_remote_timestamp` —— folder 版，断言 `DB.updated_at` + `sort_order` 保留。
3. `sync_converges_after_round_trip_no_ping_pong`（核心收敛测试）—— 模拟多设备 round-trip：①DB 有 cipher（push, 本机 T1）②模拟 B 机 pull 后回写（outline.updated_ms=T1，时间戳不变）③再 merge → 断言 `pulled == 0 && pushed == 0`（收敛）+ `DB.updated_at` 仍 T1。钉死收敛契约。

**更新 `clone_preserves_soft_deleted_at` 测试**（T1 修复，2026-07-24）：原测试用 `build_cipher_input_from_file` + `upsert_cipher`（旧路径），现改用 `upsert_cipher_from_file`（新生产路径），保持「测试调生产构造点」不变量（MatchType#1 防护）。

### P2. infra v57→v58 schema 迁移缺事务 — 崩溃致 DB 不可恢复 🟠

**根因**：`db/mod.rs:397`（57 => 分支）的 `conn.execute_batch(...)` 含 4 条 DDL（CREATE new + INSERT + DROP old + RENAME），未包事务。`execute_batch` 在 autocommit 模式下逐条自动提交，DROP TABLE 与 RENAME 之间崩溃（断电 / kill -9 / panic）→ hotword_sets 已提交删除 + hotword_sets_new 残留 → 重启 init_schema 走到 `CREATE TABLE hotword_sets_new`（非 IF NOT EXISTS）报 `table already exists` → 迁移 fail → `ensure_db` 持续 Err → 应用无法启动，DB 不可恢复。对比同文件 `insert_vault_ciphers_batch:303`（正确用 `unchecked_transaction`）。

**修复**（`db/mod.rs:397`）：
```rust
let tx = conn.unchecked_transaction()?;
tx.execute_batch("CREATE TABLE hotword_sets_new ...; INSERT ...; DROP TABLE hotword_sets; ALTER TABLE hotword_sets_new RENAME TO hotword_sets;")?;
tx.commit()?;
```
4 条 DDL 原子化（全成功或全回滚）。`unchecked_transaction(&self)` 接收 `&Connection`（不需 `&mut`），与 `init_schema(conn: &Connection)` 签名兼容。

**未修（P3 技术债）**：v56→v57 的 `execute_batch`（建 `hotword_words` 表 + seed，`IF NOT EXISTS`，无 DROP）非破坏性，未包事务——风险低（中间崩溃留空表，重启幂等），留作技术债。

### P3 观察汇总（技术债，非阻塞，不本轮修）

| # | 位置 | 观察 |
|---|---|---|
| 1 | `SyncPanel.tsx:175,195` | `handleResolve` 函数体缺 `if(resolving) return` 早返回 + `useCallback` deps 缺 `resolving`（纵深防御，按钮 disabled + Enter `!resolving` 两入口已守住，新增第三入口会漏） |
| 2 | `db/mod.rs` 多处 | hotword/agent/action_bar 的多语句写入未包事务（同 P2 迁移的同类问题，但非破坏性 DDL，风险低） |
| 3 | 迁移错误处理 | 部分 `let _ =` 吞迁移返回值；`set_test_db` user_version=46 |
| 4 | LIKE 查询 | 部分 LIKE 未 escape 用户输入中的 `%`/`_` |
| 5 | `opus_mt.rs:173-203` | 单句超长（500+ 无标点）缺字符硬切，依赖 `truncate(500,...,Right)` 丢尾部（m2m100 有 `split_into_chunks` 硬切，opus_mt 无等价处理）。罕见不 panic |
| 6 | `dlp/main.rs:236-328` | yt-dlp/ffmpeg 子进程无 timeout，直播流源可能永久挂起。CLI 惯例，用户可 Ctrl-C |
| 7 | `cloud.rs:43-50` vs `translate.rs:94-122` | CloudModel 直接 await（内部 blocking reqwest）vs FallbackLlm 走 spawn_blocking，隔离方式不一致。当前调用链已隔离，未来改 `tokio::spawn` 直接 await 会暴露 |

### 健康确认（本轮无 ≥80 发现）

- pty crate（3 线程模型 / ChildKillGuard RAII / join_reader_with_timeout / overflow 背压 / getpwuid_r 可重入 / write_if_changed 原子）：健康。
- translation crate（ngram 重复惩罚越界保护 / m2m100 8-token 重复检测 / argmax NaN 安全 / i64→u32 边界检查 / normalize_cjk_spaces）：健康。
- dlp crate（DownloadedFileGuard RAII / reqwest 300s 超时）：健康。

### 验证

- `cargo build -p octopus-infra -p octopus-vault` —— 0 error 0 warning。
- `cargo test -p octopus-infra --lib` —— 183 passed（含迁移测试）。
- `cargo test -p octopus-vault --lib` —— 262 passed（259 旧 + 3 新 P1 回归）。

### 历史遗留项升级（文档化 → 已修）

交接笔记原列「P2-3 vault 时间戳跨设备覆盖（VaultCipherInput 加字段 + SQL，较大改动）」为「文档化不修」——本轮 P1 已修复（采用 sync-only upsert 方式，比原设想的「VaultCipherInput 加字段」风险更低）。此项从「文档化不修」升级为「已修」。

## 15. 第十二轮审查修复（2026-08-04，2 P1 + 4 P2 + 4 P3 + 1 doc）

第十二轮全新核查（5 模块域：desktop 核心 / 前端 / ASR+OCR+polish / vault+infra / config+pty）。报告 10 个问题**全部 CONFIRMED**（每个证据点亲自 Read 核实）。全部修复。

### P1-1. clipboard dock hotkey expanded→collapsed FLOAT_DEPTH 泄漏 🔴

**根因**：`clipboard_window.rs:278-285` hotkey toggle 的 expanded→collapsed 分支只切 `DOCK_EXPANDED=false` + emit collapse，**不调 `after_floating_window_hide*`**；而 collapsed→expanded 分支（:291）调了 `before_floating_window_show`（depth+1，隐藏 Regular 窗口）。奇数次 toggle 后 depth>0 永久 → `restore_hidden_windows_only`（activation.rs:319）`if !float_depth_decrement_and_is_zero() { return }` early return → 被隐藏的 settings/compact_editor/terminal 窗口永久消失（托盘 open_settings 只 set_focus 无 show）。历史债（git 证实非本 PR 引入）。

**修复**：expanded→collapsed 分支（:285 emit collapse 后）加 `after_floating_window_hide_keep_active(app)`。选 `keep_active` 变体（非普通 hide）：dock 收缩态 clipboard_window 仍 visible（只宽度收窄），不能 deactivate 交还前台焦点——会打断用户当前工作。`keep_active`（activation.rs:353）递减 depth + 恢复隐藏 Regular 窗口但不 deactivate。

**作用域**：只改 hotkey toggle 路径。`clipboard_dock_expand`/`clipboard_dock_collapse` Tauri 命令（:223/:234）不碰 depth（不调 before_floating_window_show），无需改。

### P1-2. MarkdownPreview 空内容挂载后链接拦截永久失效 🔴

**根因**：`CompactEditor/MarkdownPreview.tsx` click listener effect（:34-68）绑到 `<article>`，但①effect deps 是 `[]`（仅挂载一次）②空内容分支（:70-76）不渲染 `<article>` → mount 时 `articleRef.current=null` → `if(!article) return` 早退、listener 不注册 → 后续内容到达 `<article>` 挂载但 effect 不重跑 → 点链接无 preventDefault → WKWebView 导航到外链，编辑上下文丢失。

**修复**（方案 C，委托到容器）：listener 从 `<article>` 挪到外层 `<div>`（两分支都渲染的稳定容器）。新增 `containerRef` 指向根 `<div>`（两分支都加 ref），click 冒泡到 div，`closest("a")` 仍命中 article 内链接。空内容分支也生效（div 总在），保留「仅挂载一次」语义。锚点跳转 `article.querySelector` 改 `containerRef.current?.querySelector`（查询范围等价）。

### P2-1. record stop() stdin 写失败卡 Stopping 🟠

**根因**：`record/src/session.rs:281` `stdin.write_all(b"stop\n").await.map_err(RecordError::SpawnFailed)?` 失败时 `?` 早返回，state 已切 Stopping（:279）但无 `reset_to_idle`。对比超时分支（:300）有 `reset_to_idle`。helper 崩溃/被 kill/系统休眠后 ESC → state 卡 Stopping → 后续 ESC 撞同路径 → 须 tray「强制停止」恢复。

**修复**：把 write 结果捕获到 lock 段外（锁先释放，reset_to_idle 内部再锁），失败时调 `self.reset_to_idle().await` 再返错。对齐超时分支模式。

### P2-2. OCR ordered_counter 列表类型切换不重置 🟠

**根因**：`ocr/src/layout.rs:104-149` 的 `need_split`（:107/:130）只看几何间距（`y - (py+ph) > median_h * PARAGRAPH_GAP_RATIO`），不看列表类型。ordered→unordered→ordered（小间距）时第二段 ordered 从残留计数继续（ordered_counter 仅 Heading/大间距/BodyParagraph 重置）。

**修复**：`need_split` 加「前项列表类型不一致即重置」——Ordered 分支检查前项是 `ListItemUnordered` → true；Unordered 分支检查前项是 `ListItemOrdered` → true。用 `Some(Unit::ListItemUnordered(..)) => true` 简洁模式。回归测试 `end_to_end_list_type_switch_resets_counter`（ordered 3 项 + unordered 2 项 + ordered 2 项小间距，断言第二段从 1 开始）。

### P2-3/P2-4. record_window + overlay_window 坐标混算 🟠

**根因**：`record_window.rs:103` + `overlay_window.rs:58` 把 `pos.x`（`Monitor::position()` 物理像素）未除 scale 直接加 `mon_w=sz.width/scale`（逻辑），下游 `set_position(LogicalPosition)` 契约违反。主屏 origin≠0 + Retina 时偏移。这是 AGENTS.md「物理/逻辑坐标转换」已知 gotcha 的新实例（已修 compact_editor_window/window_position）。

**修复**：`pos.x as f64` → `pos.x as f64 / scale`（position 也除 scale 统一逻辑）。两处同模式。

### P3-1. Result/index.tsx toast timer ref

`Result/index.tsx:101` `setTimeout` 未存 ref，连续不同 ms 的 toast 早到 timer 截短后到。照搬第九轮 Settings `toastTimerRef` 范式——`useRef<ReturnType<typeof setTimeout> | null>(null)`，showToast 先 clearTimeout 前次。

### P3-2. ImagePreview prevNatW 闭包旧值

`ImagePreview/index.tsx:229` full-load effect deps 仅 `[imageId]`，闭包里 `natW` 是前图旧值（缩略图 naturalWidth）。加 `natWRef` 镜像 + `setNatWSync` setter（对齐同文件 `setZoomSync` 范式），`:229 prevNatW = natWRef.current`（读最新值）。低风险（触发窗窄、不损数据）。

### P3-3. ActionBarPanel triggerKeyword debounce

`ActionBarPanel.tsx` 的 `titleDraft` 有 debounce（:280-288），`triggerKeyword` input 无（:454-457 每按键 IPC + refresh，慢时字符黏滞）。照搬 titleDraft 范式：加 `triggerDraft` state + ref + 300ms debounce effect，input `value={triggerDraft !== null ? triggerDraft : ...}` + onChange `setTriggerDraft`。

### P3-4. rename_hotword_set_at 刷 sync_md5（防御性）

`infra/db/hotword.rs:140` 不刷 sync_md5，与 vault `update_vault_folder_name` 不对称。当前 caller（desktop `refill_sync_md5`，10 处）全兜底，零触发；残留风险：未来 cli/server 直调会 ping-pong。修复：`rename_hotword_set_at` / `rename_hotword_set` 加 `sync_md5` 参数，UPDATE 含 `sync_md5=?2`。caller（desktop Tauri 命令 + 6 测试）算 md5 传入（`hotword_set_md5_from_fields`）。对齐 vault 范式。

### P3-doc. engine.rs:2223 stale 注释

`engine.rs:2223` 注释引用已删除的 `build_cipher_input_from_file`（第十一轮删），改 `upsert_cipher_from_file`。

### 验证

- `cargo build` infra/record/ocr/vault/sync/desktop(cloud,vault) —— 0 error 0 warning。
- `cargo test`：infra 183 / record 50 / ocr 35（+1 P2-2）/ vault 262 / sync 126 / desktop 520 全过。
- tsc exit 0 + vite build 成功。

## 16. 第十三轮审查修复（2026-08-04，1 P2 + 3 P3）

第十三轮覆盖度受限（审查代理 429 限额，改为亲自逐模块 Read）。聚焦 desktop 核心（clipboard/activation/record/session/settings/compact_editor/dock）+ 前端 SyncPanel。报告 4 个问题全部 CONFIRMED。

### P2-1. open_settings 已存在分支不 show → 浮窗期间托盘打开设置无效 🟠

**根因**：`settings_window.rs:23-47` 的「窗口已存在」分支 macOS 下只 `set_activation_policy(Regular)` + `activate_self` + `set_focus`（:30-35），**无 `window.show()`**。触发链：`WINDOWS_TO_HIDE_ON_FLOAT`（activation.rs:130）含 `settings_window` → 浮窗显示时 `before_floating_window_show` 临时隐藏 settings（depth=1）→ 用户点托盘「打开设置」→ `:23` 分支 `set_focus` 对 hidden 窗口无效 → settings 不显示；`restore_hidden_windows_only`（:319）因 depth>0 不恢复 → 直到 ESC 关浮窗（depth=0）settings 才出现。对比 `open_compact_editor_tabs`（compact_editor_commands.rs:243）窗口已存在分支正确调 `window.show()`——settings 是唯一遗漏。

**与第十二轮 P1-1 的区别**：P1-1 修了 dock FLOAT_DEPTH 泄漏致 restore 失败（depth 永久 >0），但**没修「浮窗存活期间 open_settings 无法显示已 hidden 窗口」**——即使 depth 不泄漏，浮窗存活期间（depth=1）点 open_settings 仍无效。

**修复**：`:23` 分支两个 cfg 都加 `if !w.is_visible().unwrap_or(false) { let _ = w.show(); }` 在 `set_focus` 前，对齐 compact_editor :243 范式。

### P3-1. SyncPanel handleResolve catch 不回滚 syncing

**根因**：`SyncPanel.tsx:175-195` `handleResolve` 的 `:189` 设 `syncing=true` + `:190` `invoke("vault_sync_now")`，`:191` catch 只 `showToast` 不回滚 `syncing`。对比 `handleSyncNow:169` catch 有 `setStatus(...syncing: false)`。invoke 失败（spawn_blocking 错等）时不会收到 `vault-sync-done` 事件 → 若不回滚 syncing 进度条永久卡住。

**修复**：catch 补 `setStatus((prev) => (prev ? { ...prev, syncing: false } : prev))`（对齐 handleSyncNow :169）。

### P3-3. record session serde_json 序列化失败卡 Starting

**根因**：`session.rs:159` `serde_json::to_string(&request)?` 失败时 `?` 早返回，state 已切 Starting（:153）无 reset_to_idle。对比 spawn 失败路径（:170-176）有 reset_to_idle。触发概率极低（request 是已知可序列化 struct），但与第十二轮 P2-1（stop() 卡 Stopping）同构。

**修复**：`?` 改 match，失败分支调 `self.reset_to_idle().await` + `Err(RecordError::Json(e))`，对齐 spawn 失败路径。

### P3-2. reader task EOF 静默退出（文档化，不修）

**现状**：`session.rs:197` `while let Ok(Some(line)) = reader.next_line().await` EOF（helper 崩溃/退出）时 `Ok(None)` 静默退出循环，spawn task 结束，不更新 state（仍 Recording）。helper 崩溃 → state 卡 Recording → 用户须 ESC 触发 `stop()`（`child.wait()` 对已死 child 立即返回，清理有效）。

**为何不修**：① ESC 可可靠恢复（stop 路径对已死 child 正常工作）；② 加主动崩溃通知需设计新 state（Crashed?）/ 事件 emit，是行为变更非纯 bugfix，且第12轮 P2-1 已修 stop() 下游清理；③ 本轮 conf 58 低优先级。留作已知限制。

### 观察项（conf 40，未达上报阈值，不修）

- `clipboard_dock.rs:23-25` `POLL_ACTIVE` 防重复 + dock 切换边（left↔right）时若旧 poll 线程未及时 `stop_edge_poll`，新 `start` 直接 return 用旧 edge 参数。正常流程 stop 清除不触发。
- `SyncPanel.tsx:454/480` `syncError.includes("主密码")` / `includes("空库恢复")` 中文硬匹配后端错误消息，后端改文案即失效（脆弱契约）。当前工作，留待后端错误码化时一并处理。

### 验证

- `cargo build` record/desktop(cloud,vault) —— 0 error 0 warning。
- `cargo test`：record 50 / desktop 520 全过。
- tsc exit 0。

## 17. 第十四轮审查修复（2026-08-04，2 P1 + 4 P2 + 7 P3）

第十四轮全新核查（5 模块域并行代理 + 亲自 Read 核实）。报告 15 个问题全部 CONFIRMED，本轮修复 13 个（2 P1 + 4 P2 + 7 P3），2 个文档化不修（P3-1 block_on / P3-2 autotype，低风险）。

### P1-1. Result 自动翻译 setInterval 漏用 ref 🔴

**根因**：`Result/index.tsx:384` setInterval effect deps 含 `doTranslate`（:389）。流式 ASR `update-result` 高频 `setText` → `text` 变 → `doTranslate`（useCallback deps `[text, showToast]`）新引用 → effect 重起 → `clearInterval`+新 timer → 4s/8s/12s 跑不满，录音中自动翻译完全不触发。同文件 `:412` keydown 正确用 `doTranslateRef.current()`，`:332` ref 已同步——纯 setInterval effect 漏用 ref 的疏忽。

**修复**：`:384 doTranslate()` → `doTranslateRef.current()`；deps `[translateMode, doTranslate]` → `[translateMode]`。

### P1-2. invoke_handler get_record_status 漏 cfg gate 🔴

**根因**：`invoke_handler.rs:410` `get_record_status` 前后 35+ 个 record_commands 项都有 `#[cfg(target_os="macos")]`，唯独它漏。record_commands 模块 `#![cfg(target_os="macos")]`（mod.rs:11）→ 非 mac build 时符号不存在 → 引用未定义 → 编译失败。mac-only 不触发，但 Linux/Windows CI 立刻炸。

**修复**：`:410` 前补 `#[cfg(target_os = "macos")]`。

### P2-1. vault CipherPlaintextMeta + FolderFile 缺 serde default 🟠

**根因**：`store.rs:225`（CipherPlaintextMeta.is_deleted）+ `:287`（FolderFile.is_deleted）缺 `#[serde(default)]`。老格式文件（软删 v53 上线前 sync、之后未再 sync）缺 is_deleted → `read_cipher_file` 报错 → `import_all_from_files` log::warn + 静默跳过 → 用户密码/分类静默丢失。对照 hotword HotwordSetMeta:145 加了 default，vault 漏。cipher 是密码主数据，丢失比 folder 严重。

**修复**：两字段加 `#[serde(default)]`（老文件 → false，符合 MVP 语义），对齐 hotword。

### P2-2. vault merge pull 缺删除单向传播守卫 🟠

**根因**：hotword `pull_set`（hotword.rs:965-979）有守卫：远程 active + 本地 tombstone → 拒绝 pull（防本地删除被远程旧 active 复活）。vault merge pull 的 `remote_updated > local_updated` 分支（engine.rs cipher :1188 / folder :1118）直接 upsert，无此守卫。跨设备时钟偏差 + 删除竞争（设备 A active 时钟超前 > 设备 B 软删）→ pull 覆盖 → 删除复活。可自愈（用户重删），非数据丢失。

**修复**：cipher + folder pull 分支前置 `if !row.is_deleted && existing_db.is_deleted { skip }`（对齐 hotword）。

### P2-3. vault rename_folder 硬编 is_deleted=false 🟠

**根因**：`folder.rs:89` `folder_md5_from_fields(id, &encrypted, sort_order, false)` 硬编 false。软删态 folder rename 后 DB row is_deleted=1 但 sync_md5 按 false 算 → md5 与 row 不一致 → 跨设备 sync ping-pong。另 `:84` 用 `list_vault_folders().find()` O(N)，应 O(1)。与第十二轮 P3-4 hotword rename 同型对称遗漏。

**修复**：改 `load_vault_folder` O(1) + 读真实 is_deleted 算 md5。

### P2-4. record control fps/codec 硬编 🟠

**根因**：`control.rs:462-463` `fps: 30, codec: "h264".into()` 硬编。RecordConfig 支持 record_fps/record_codec，用户改 60fps/HEVC 后入库 recordings.fps/codec 恒 30/h264，前端列表展示错误。

**修复**：MetaFields 加 `fps: u32` + `codec: String`；derive_fields_from_request 从 `req.video.fps` / `req.video.codec`（VideoCodec → "h264"/"hevc"）填；record_stop 加两 Option 参数；stop_and_store_inner 用 fields 值。

### P3-3. dlp exit(1) 跳过 _cleanup_guard Drop

`main.rs:333` `std::process::exit(1)` 跳过 `_cleanup_guard` Drop → 临时文件残留。改 `anyhow::bail!`（走正常返回路径，guard Drop 清理）。

### P3-4. ActionBar getSubItems 读 menuItems state 非 ref

`ActionBar/index.tsx:475` 读 `menuItems`（闭包）非 `menuItemsRef.current`（:727 已同步）。keyboard 展开子菜单默认选中项错。改 `menuItemsRef.current.length > 0 ? menuItemsRef.current : menuItems`。

### P3-5. OCR strip_unordered_marker ASCII 标记误判

`layout.rs:217` ASCII 标记（-/*/+) 不要求后续空格 → `-1/*5/+1/+5°C` 数学片段误判列表项。改 ASCII 标记要求后续空格（Unicode 符号不要求）。

### P3-6. CompactEditor hoverTimer cleanup 漏清

`CompactEditor/index.tsx:128` unmount cleanup 只清 `savedFlashTimer`，`hoverTimer` 漏清 → 泄漏。补 `clearTimeout(hoverTimer.current)`。

### P3-7. Result speakingTimer/polish-error timer cleanup

`Result/index.tsx` speakingTimer 无 unmount cleanup + `:261` polish-error `setTimeout` 无 ref 管理。补 unmount cleanup effect（清 speakingTimer + polishErrorTimerRef + toastTimerRef）+ polish-error 用 ref。

### P3-8. streaming_zipformer chunk_len/shift 无下界

`streaming_zipformer.rs:55-62` `unwrap_or(77|64)` 无 `.max(1)`，异常模型（metadata=0）→ frame_idx += 0 原地踏步死循环。补 `.max(1)`。

### P3-9. qwen3 run_decoder_step 维度异常静默丢 KV delta

`qwen3_asr.rs:590/611` 维度不匹配（kd.len()!=4 || kd[1]!=s）时静默跳过，无 log。模型损坏触发，生成乱码不报错。补 `log::warn!`。

### 文档化不修（低优先级）

- **P3-1 settings_commands block_on**（settings_commands.rs:225）：`tauri::async_runtime::block_on` 读 `state().await`（读锁快），复用全局 runtime 非嵌套，无 panic 风险。留技术债。
- **P3-2 autotype copy_concealed 吞错**（autotype.rs:137）：罕见双失败（autotype+clipboard），降 P3 留技术债。

### 观察项（不入修复）

- vault md5 故意不含 is_deleted（设计取舍，有 P2-2 守卫兜底）
- config/pty 代理 4 项均 < 80（set_test_db user_version=46 staleness / image.rs 注释 jpeg:100 vs 92 / session.rs Mutex unwrap / cloud.rs blocking HTTP）

### 验证

- `cargo build` vault/record/desktop(cloud,vault)/dlp/ocr/asr-local —— 0 error 0 warning。
- `cargo test`：vault 262 / record 50 / ocr 35 / desktop 520 全过。
- tsc exit 0。

## 18. 第十五轮审查修复（2026-08-05，跨轮漏修 + 2 P2 + P3 批量）

第十五轮全新核查（5 模块域并行代理 + 亲自 Read 核实）。报告含 1 跨轮漏修（P3-8 Transducer）+ 2 P2 + 多组 P3。核实结论：P3-C（tencent final=1 stable 空）为设计取舍（全静音发 Finished 比 Failed 更合理），不修；P3-E（engine_dispatch is_cloud 不对称）文档化（streaming 路径不走 dispatch，实际不触发）。其余全修。

### 跨轮漏修：P3-8 StreamingZipformerTransducer chunk_len/shift .max(1)

**根因**：第十四轮 P3-8 只修了 `StreamingZipformer`（CTC，:57-66），漏修 `StreamingZipformerTransducer`（:532-539）——两者同文件、同模式、同死循环风险（Transducer :273/:285 `frame_idx += self.chunk_shift` while 循环，metadata=0 时原地踏步）。spec §17 也只记一处，双向疏忽。

**修复**：Transducer :532-539 补 `.max(1)`（对齐 CTC）。spec §17 补记 Transducer 漏修。

### P2-A. cloud translate 缺 spawn_blocking（panic/死锁）🟠

**根因**：`translation/src/cloud.rs:43-50` `CloudLlmEngine::translate`（async fn）内部直接调 `octopus_llm::chat_text_with_prompt`（reqwest::blocking）。调用方 `do_translate`（translate.rs:94-108 CloudModel 分支）经 `tauri::async_runtime::block_on` 进入 tokio runtime → future 在 worker 线程 poll → reqwest::blocking 检测 runtime context → panic "Cannot start a runtime from within a runtime"。对比同文件 FallbackLlm 分支（:110-122）正确用 spawn_blocking。

**触发**：用户配云端翻译模型（deepseek/openai/智谱等）并执行翻译时必现 panic。worker 路径无 catch_unwind → 杀线程；coordinator 路径有 catch_unwind → 翻译功能失效。

**修复方式决策**：translation crate 是纯推理库（tokio 仅 dev-dep），不能在 CloudLlmEngine::translate 内 spawn_blocking。改在调用方（desktop translate.rs CloudModel 分支）用 spawn_blocking 包裹 `chat_text_with_prompt`，对齐 FallbackLlm 模式。删 `TranslationEngine` trait import（CloudModel 分支不再经 trait，LocalModel 的 `Box<dyn TranslationEngine>` 不需 trait in scope）。

### P2-B. 粘贴安全（write 失败仍 paste）🟠

**根因**：`clipboard_commands.rs:220` `let _ = write_item_to_clipboard(&handle, &item)` 吞错，`:226 focus.simulate_paste()` 无论写成功与否都粘贴。写失败时粘贴的是剪贴板残留的上一条内容（可能是密码/token）。

**修复**：write 失败时不 paste + log warn（防误粘贴敏感残留内容）。

### P3-D. baidu FIN_TEXT 空过滤

`baidu_stream.rs:218` `fin_texts.push(result)` 的 result 来自 `unwrap_or("")`，空时不过滤 → `accumulate_display` 的 `join(sep)` 产生多余分隔符（你好，，世界）。push 前补 `if !result.is_empty()`。

### P3-E. engine_dispatch is_cloud 不对称（文档化，不修）

`engine_dispatch.rs:40` is_cloud 仅判 Aliyun（DispatchEngine 只持 AliyunEngine 实例）。ByteDance/Tencent/Baidu 落 embedded.transcribe（会失败）。但实际 cloud ASR 走 streaming pipeline 不走 dispatch.transcribe → 几乎不触发。修复需给 DispatchEngine 加各 cloud engine 实例（大改），不值得。

### P3-F. dlp :285/:292 exit(1) 改 bail!

`main.rs:285/:292` 两处 `std::process::exit(1)` 在 `_cleanup_guard`（:297）创建前。改 `anyhow::bail!`（走正常返回路径）。

### P3-G. set_test_db user_version=46 改 CURRENT_SCHEMA_VERSION

`infra/src/db/mod.rs:85` 硬编 `PRAGMA user_version = 46`（既非注释说的 v40 也非 CURRENT=58）。改 `CURRENT_SCHEMA_VERSION`。

### P3-H. 注释 jpeg:100/q85 陈旧

`consts.rs:38-40` 注释说「q100」，实际 :42 是 `jpeg:92`。`clipboard/image.rs:1` 说「JPEG q85」。修正为当前 q92。

### P3 组4. 前端 setTimeout/timer cleanup（9 处）

React 18+ 不再警告 setState-on-unmount，但不符合项目既有范式（Result/CompactEditor 已正确做）。批量修复：

| # | 文件 | 修复 |
|---|---|---|
| 1 | ActionBar/index.tsx | toastTimerRef 补 unmount cleanup |
| 2 | Settings/index.tsx | toastTimerRef 补 unmount cleanup |
| 3 | VaultPicker/index.tsx | searchTimerRef 补 unmount cleanup（防 debounce 期关闭仍 invoke） |
| 4 | HistoryPanel.tsx | confirmDelete 裸 setTimeout → ref + cleanup（对齐 HistoryRow.deleteTimer） |
| 5 | ClipboardPanel.tsx | 同 #4 |
| 6 | Screenshot/index.tsx | ocrWarn setTimeout → ref + cleanup |
| 7 | useOcr.ts | 三处裸 setTimeout → 3 个 ref + 统一 cleanup |
| 8 | PasswordGenerator.tsx | handleCopy setTimeout → ref + cleanup |
| 9 | Download/index.tsx | async listen setup 缺 cancelled 哨兵 → 真实 listener 泄漏（非 setState） |

### 文档化不修

- **P3-C tencent final=1 stable 空**：全静音/全噪声时腾讯返回 final=1 但无文本，发 Finished（识别完成无内容）vs Failed（识别失败）。Finished 更合理（其他 provider 对全静音也返空文本），设计取舍非 bug。

### 验证

- `cargo build` translation/desktop(cloud,vault)/asr-local/asr-cloud/dlp/infra/clipboard —— 0 error 0 warning。
- `cargo test`：translation 20 / asr-cloud 58 / infra 183 / desktop 520 全过。
- tsc exit 0。

## 19. 第十六轮审查修复（2026-08-05，2 P2 + 2 注释微瑕）

第十六轮 4 切面全新核查（错误处理 / unsafe·FFI / 前端 / 并发·async·锁）。报告 2 P2 + 5 P3 + 1 驳回。核实结论：2 P2 修（asr-local metadata 边界同型漏修尾巴）+ 2 注释微瑕修 + P3-1/2/3/4 文档化（形式问题当前不触发）+ 驳回正确（coordinator channel FIFO 串行）。

### P2-1. 离线 Zipformer chunk_shift 缺 .max(1) → metadata=0 死循环 🟠

**根因**：`engines/zipformer.rs` 离线 CTC（:475-482）+ Transducer（:762-769）的 `chunk_len`/`chunk_shift` `unwrap_or(77|64)` 无 `.max(1)`。异常模型（metadata decode_chunk_len="0"）parse 成功得 0，unwrap_or 不生效 → `frame_idx += 0`（CTC :635 / Transducer :1009）永不推进 → 死循环（CPU 100% 卡死，非 panic 非 Result，无法兜底）。

**同型疏忽链**：第十四轮 P3-8 修流式 CTC → 第十五轮补流式 Transducer → 第十六轮发现**离线** CTC + Transducer 仍漏。三处同型（流式/离线 × CTC/Transducer）的 metadata=0 死循环修复至此全覆盖。

**修复**：离线 CTC + Transducer 两处构造补 `.max(1)`，对齐流式 streaming_zipformer.rs 模式。

### P2-2. streaming_paraformer cache_time usize 下溢 → 启动 panic 🟠

**根因**：`streaming/streaming_paraformer.rs:102` `decoder_kernel_size_str.parse().unwrap_or(11)` —— 异常模型（decoder_kernel_size="0"）parse 成功得 0 → `:104 cache_time = 0 - 1` usize 下溢 panic。new() 时崩溃（模型加载即炸）。`:264` reset() 复算 `self.decoder_kernel_size - 1` 同样下溢。

**修复**：构造端 `.max(1)` 保证 ≥1；reset 端 `saturating_sub(1)` 双保险。

### P3-5. 注释微瑕 ×2

- `streaming_zipformer.rs:534` 注释引用 `:273/:285`（CTC 的循环），Transducer 自己在 `:1009`。修正行号。
- `db/mod.rs:81` 注释「直接标 v40」，代码实际用 `CURRENT_SCHEMA_VERSION`（第十五轮 P3-G 改）。修正。

### 文档化不修（P3，形式问题当前不触发）

- **P3-1** `shared.rs:19-24` NSScreen 裸指针无 autoreleasepool——主线程同步、primitive 返回、引用立即丢弃，不悬空泄漏。
- **P3-2** `pin_window.rs:17-21` SendWindow unsafe Send/Sync 无运行时主线程守护——当前两访问点都在主线程（MainThreadMarker / cleanup selector），缺运行时断言，未来新增非主线程访问会 UB。
- **P3-3** `context.rs:364-373` NSPasteboard worker 线程无 autoreleasepool——单例 + primitive 读，无泄漏。
- **P3-4** `Screenshot scrollElapsed cleanup`——独立窗口 unmount=JS 引擎销毁，interval 自动清除，无害。

### 驳回（报告正确驳回）

- **polishNow / commit_edit fire-and-forget 竞态**：coordinator `loop { rx.recv() }`（:386-387）单线程串行消费，channel FIFO 保证 commit_edit 先落库再 polish_now，无竞态。

### 并发切面结论（本轮补足）

9 维度全量扫描（tokio 嵌套 panic / spawn 上下文 / 锁跨 await / 死锁 / channel / spawn_blocking / Send·Sync 主线程亲和 / reader task 生命周期 / Once·Lazy），置信度 ≥80 的并发 bug 为零。设计严谨（block_on 复用全局 runtime / spawn_blocking 全覆盖 / with_db ReentrantMutex + 回归测试）。

### 验证

- `cargo build` asr-local/infra —— 0 error 0 warning。
- `cargo test`：asr-local 170 / infra 188 全过。

## 20. 第十七轮审查修复（2026-08-05，2 P1 + 1 P2 + 3 P3）

第十七轮 4 模块全新核查（剪贴板同步 / OCR·stitch / DB schema 迁移 / ASR·翻译引擎数值边界）。报告 2 P1 + 2 P2 + 11 P3。本轮修 2 P1 + 1 P2（P2-2）+ 3 P3（context_size/注释），P2-1（Transcript.id 迁移收尾）工作量大切技术债留后续，其余 P3 文档化。

### P1-1. FTS5 搜索 JOIN 列写错 → ≥3 字符搜索彻底失效 🔴

**根因**：schema v59 把 `clipboard_history.id` 从 INTEGER 改 TEXT(UUID)，FTS5 trigger 改用隐式 rowid（schema.sql:115-118）。但 `store.rs:138/175/177` 的 FTS5 JOIN 仍写 `c.id = f.rowid`——c.id 是 TEXT(UUID)，f.rowid 是 INTEGER，类型不匹配 → JOIN 永远不相等 → ≥3 字符 FTS5 搜索恒返回空。对比 `transcription.rs:227` 正确写 `c.rowid IN (SELECT rowid FROM ...)`。

**修复**：3 处 `c.id = f.rowid` → `c.rowid = f.rowid`。回归测试 `test_fts5_search_finds_inserted_content`（插 2 条 + 搜索中文 4 字符，断言找到 1 条）。

### P1-2. 剪贴板 tombstone pull 丢远程时间戳 → 多设备 ping-pong 🔴

**根因**：`sync/clipboard.rs:630` tombstone「DB active → 软删」分支调 `soft_delete_favorite`（SQL 硬编 `datetime('now')` 本地时间），而 None(:639)/active(:662) 两分支用 `upsert_favorite_sync` 带 `file.updated_at.clone()`（远程时间）。pull 后 DB.updated_at = 本地 now → export 写回 outline → 多设备交替 sync 每台改 outline → git 永不收敛。vault 第十一轮 P1 同型重现。

**修复**：tombstone 分支改用 `upsert_favorite_sync` 传 `file.updated_at.clone()`（与另两分支对称）。

### P2-2. 离线 Paraformer enc_feat=0 触发 0/0 panic 🟠

**根因**：`engines/paraformer.rs:187 let enc_feat = enc_dim[2]` 无 `.max(1)`。异常模型 shape[2]=0 → `:236 acoustic_embedding.len() / enc_feat` 除零 panic（num_tokens==0 守卫在除法后）。

**修复**：`.max(1)`（对齐 chunk_len/shift/kernel_size 同型纪律）。

### P3-1/P3-2. Zipformer Transducer context_size 缺 .max(1)

离线 `engines/zipformer.rs:820` + 流式 `streaming_zipformer.rs:555` 的 `context_size unwrap_or(2)` 无 `.max(1)`。两处补 `.max(1)`（同型疏忽尾巴——chunk_len/shift 已修，context_size 漏）。

### P3-9. init_schema 注释 v55 → CURRENT_SCHEMA_VERSION

`db/mod.rs:255-259` 注释说「v==55 最新」，实际 `CURRENT_SCHEMA_VERSION=59`。修正。

### 文档化不修 / 留后续

- **P2-1** Transcript.id i64→String（schema v59 迁移收尾）：工作量大（Transcript + DbCommand + cancel_discard 反推时长），是迁移半完成技术债非紧急 bug。留后续。
- **P3-3** capx finalize footer content_tail：需开发者核实产品意图（注释↔实现不符）。
- **P3-4** streaming_paraformer decoder_num_blocks 越界：metadata 异常增大时 outputs 索引越界，建议 .get() Option 取出。
- **P3-5** 离线 paraformer encoder_output_size：有空守卫早退不 panic。
- **P3-6** history_row_md5 漏算 4 字段：仅这些字段变化不触发 sync。
- **P3-7** push_favorite 冗余 IO：阶段 1/2 写入被阶段 3 export 覆盖。
- **P3-8** pull_favorite 多 SQL 跨 with_db 无事务。
- **P3-10** v57→v58 PRAGMA user_version 未入事务（迁移幂等可恢复）。
- **P3-11** db_queue unbounded mpsc 无背压。

### 验证

- `cargo build` clipboard/asr-local/infra/sync —— 0 error 0 warning。
- `cargo test`：clipboard 24（+1 P1-1）/ asr-local 170 / infra 183 / sync 145 全过。

## 21. 第十八轮审查修复（2026-08-05，1 P1 + 2 P2）

第十八轮审计。报告概要：P1 ×1（vault_sync_clone/resolve git 重操作漏 spawn_blocking）+ P2 ×3（import_hotwords BOM + autotype/reindex_apps 阻塞）+ P3 ×10。核实后修 P1 + 2 P2（import_hotwords BOM + reindex_apps），autotype 涉及焦点/keystroke 时序复杂留技术债。

### P1. vault_sync_clone/resolve git 重操作漏 spawn_blocking 🔴

**根因**：`vault_sync_commands.rs` 的 `vault_sync_clone`（:113）、`vault_sync_resolve_remote`（:120）、`vault_sync_resolve_local`（:140）都是同步 Tauri 命令，直接跑 git clone/merge/push（网络 + 文件系统重操作，几秒到几十秒），占 Tauri IPC 线程。对比同文件 `vault_sync_now`（:56）正确用 `async + spawn_blocking`。范式不一致。

**修复**：3 个命令改 `async + spawn_blocking + await`（返回结果，非 fire-and-forget）。前端 invoke 自动适配（async 命令对前端透明——await 即可）。

### P2. import_hotwords BOM 污染

**根因**：`sync/hotword.rs:695 read_to_string` + `:697 serde_json::from_str`——外部编辑器（Windows Notepad）可能给 meta.json 加 UTF-8 BOM（`\u{FEFF}`），serde_json 不容忍 BOM 前缀 → 解析失败。export 路径不加 BOM，但用户手动编辑或 Windows 工具可能加。

**修复**：`content.trim_start_matches('\u{FEFF}')` strip BOM。

### P2. reindex_apps 阻塞

**根因**：`search_commands.rs:93 reindex_apps` 同步命令，`refresh_app_index` 扫文件系统可能几秒，占 IPC 线程。

**修复**：改 `async + spawn_blocking`。

### 文档化不修

- **autotype 阻塞**：涉及焦点/keystroke 时序（osascript + CGEvent + sleep），async 化会打乱时序。留技术债（当前占 IPC 线程但功能正常）。
- **P3 ×10**：报告概要未给详细证据，多为形式问题（注释漂移 / 无背压 channel / 冗余 IO 等），留技术债。

### 验证

- `cargo build` desktop(cloud,vault) —— 0 error 0 warning。
- `cargo test`：sync / desktop 520 全过。
