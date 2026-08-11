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
| 19 | `d0378d46` | P2-1 itemId 崩溃 + P2-2 MarkdownPreview listener + BOM 错位补修 + config fail-soft |
| 20 | `e12364d2` | P2-4 opus_mt 超长单句硬切 + P2-5 翻译引擎未知名显式报错 + P2-6 驳回 |
| 21 | `d6b8b138` | P2-v1 resolve attempt_guard + P2-a3 whisper 空音频 + P2-a2 aliyun warn + P2-s5 clipboard 复活保护 |
| 22 | （无 commit） | merge main（`c309b085` concealed hint）+ 复审 watcher.rs，**0 修复**（审查通过） |
| 23 | （本轮，待 commit） | 全代码审查 23 P2 复查：修 12（infra 4 事务 + record thumbnail 事务 + autotype PasswordOnly verify + cloud close catch_unwind + ocr validate 激活 + llm error body 截断 + sync thread-local clear + **scheduler hang 超时兜底** + **download Etag 注释修正** + **server spawn_blocking 超时**）+ 驳回 1（P2-ocr2 误报）+ 留后续 10 |
| 24 | （本轮，待 commit） | 留后续项推进：修 5（spawn .expect 降级 + image size guard + WAV decode spawn_blocking + Etag dead code 清理 + WS engine 校验）+ 留后续 2（P2-srv2 共享 session 重构 + P2-d1 autotype 移线程） |
| 25 | （本轮，待 commit） | P2-sync1/sync2 export 原子化实施：方案 B 先写后清孤儿（clipboard + hotword + vault 三处 export 去 remove_dir_all）+ 方案 C 删 push 冗余写。5 Task + 5 回归测试 |
| 26 | `968a6f3e` | 第二十四轮报告复查：修 5（**P1-1 cloud 编译 regression** + P2-c4 hotword GC 取锁 + P2-c5 vault meta 加锁 + P2-c2 ocr TOCTOU + P3 hotword thread-local/pty 中毒）+ 留后续 2（P2-c1/c3）+ 接受 P2-l3 半修反馈 |
| 27 | `587d16b4` | 第二十七轮报告复查：修 3（P2-3 opus_mt eos bail! + P2-1 tombstone 吞错改 warn + P3-4 谎报推送改 message）+ 留后续 4（P2-1 active 事务 / P2-2 / P3-1 / P3-3） |
| 28 | `260ed000` | 第二十八轮并发专项复查：修 3（**P1-F1 vault meta 覆盖竞态→锁死** + P3-F5 clipboard_cleanup 取锁 + P3-LLM1 正则限定）+ 留后续 2（P2-F2 translate block_on / P2-F3 pty Mutex 跨 write） |
| 29 | `21e70af8` | 第二十九轮数据完整性复查：修 3（P2-F1 paraformer enc_slice 越界离线+流式 + P2-F3 canvas 黑图改 Result + P3-LLM1 单候选契约闭合）+ 留后续系统性短板（P3 F-2~F-8 ONNX 边界） |
| 29b | `409291e4` | 第二十九轮补充（代理 C 补跑）：修 3（P2-C1 md5 补 ref_data/segments/is_rich + P2-C2 voice 吞错致物理删改 fail-safe + P3-CF9 set_sync_md5 改 warn）+ 留后续 P3 群 |
| 30 | `744add56` | 第三十轮全覆盖复查：修 3（**F1 stitch uniform scroll 永久锁定** + **L1-2 热词挖掘 ORDER BY id DESC 迁移回归** + P3 focus_tracker 重复 log）+ 留后续性能群（Zipformer clone / vault 全量 export / pipeline 双读 / hotword 全表 filter） |
| 31 | `426737ea` | 第三十一轮资源/并发/panic 专项：修 5（**P1-1 helper 无超时+kill_on_drop** + P2-1 frame_size=0 除零 3 处 + ptt B1 线程死亡恢复 + ptt B2 recv_timeout + P2-3 cgimage bpr ensure）+ 留后续 4（std Mutex 中毒 / reap 超时 / attempt_guard / dead code） |
| 32 | `43bf2298` | 第三十二轮 sync+db/LLM/commands/FFI 全覆盖：修 4（P2-1 get_model_detail 漏解密 + P2-2 unregister recv_timeout + P2-4 clipboard export 超期过滤 + P2-5 generate_subtitle spawn_blocking）+ 留后续 5（set_config block_on / vault export 过滤 / terminal_list_dir / pin_screenshot / probe_permission） |
| 33 | （本轮，待 commit） | 第三十三轮 coordinator/infra/pty 核实：修 2 P1（**开录音失败残留 mode 5 路径** + **cloud close 重复 append partial**）+ 留后续 P2 群 6 项 |

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

## 22. 第十九轮审查修复（2026-08-05，4 P2 修 + capture P2 留后续）

第十九轮全新核查（4 模块 + 2 自查模块）。报告 8 P2 + ~13 P3。本轮修 4 P2（CompactEditor 崩溃 + listener 失效 + BOM 错位补修 + config fail-soft），capture 4 P2 需更多核实留后续。

### P2-1. prompt_files itemId i64→string（渲染崩溃）🟠

**根因**：`prompt_files.rs:111 "itemId": item_id`（JSON number），`:122` pending 路径用 `item_id.to_string()`（string）。前端 `tab.itemId.slice(-5)` 对 number TypeError → React 子树崩溃 → CompactEditor 白屏。Tauri emit 绕过 TS 类型检查。

**修复**：`:111` 改 `item_id.to_string()`（与 :122 对称）。

### P2-2. MarkdownPreview 条件渲染切换致 click listener 失效 🟠

**根因**：第十二轮 P1-2 修了「空内容 mount 时 articleRef=null」，但两分支条件渲染是不同 DOM 节点 → 分支切换（空→非空）时旧 div 卸载 listener 失效，新 div 无 listener（effect deps `[]` 不重跑）。

**修复**：合并为单容器（始终渲染同一 div），空内容用条件子元素（CSS 切换 article / 提示 span），listener 始终绑同一个 div。

### BOM 错位补修（第十八轮遗漏）

**根因**：第十八轮报的 BOM 问题是 `hotword_commands.rs:296`（用户从 txt 导入，split_whitespace 首词污染），开发修了 `sync/hotword.rs:697`（meta.json 路径）——两个不同函数。`hotword_commands.rs:296` 未修。

**修复**：`:296 read_to_string` 后 `trim_start_matches('\u{FEFF}')` strip BOM。

### P2. config migrate_yaml_to_db fail-soft 🟠

**根因**：`:546 serde_yaml::from_str(&text)?` + `:551 serde_yaml::from_value(value)?`——yaml 解析失败（BOM / 类型不匹配 / typo）→ `?` bail → init_schema 失败 → 应用启动卡死，yaml 不改名下次重复失败。用户按 rm DB 提示反把自己锁死（yaml 仍在）。

**修复**：fail-soft——解析失败时 rename `.yaml.broken` + log warning，让 init_schema 继续走 seed 默认值。含 BOM strip。

### 文档化不修 / 留后续

- **capture P2 ×4**（area.rs monitor 匹配单位错位 / scroll.rs ? 降级 / clone×2 / 双份内存）：需更多核实 monitor_x 单位 + xcap 接口，留后续。
- **P3 ×13**：跨 4 模块性能/边界/设计选择。

### 验证

- `cargo build` desktop(cloud,vault)/infra —— 0 error 0 warning。
- `cargo test`：infra / desktop 520 全过。
- tsc exit 0。

## 23. 第二十轮审查修复（2026-08-05，2 P2 修 + 1 驳回 + 4 P2 留后续）

第二十轮全新核查（15 模块）。报告 7 P2 + ~12 P3。本轮修 2 P2（翻译引擎），驳回 1（pty mutex 已修），4 P2（download ×2 + scheduler + pty spawn）留后续需更多设计。

### P2-4. opus-mt 长无标点单句静默截断 🟠

**根因**：`opus_mt.rs:97-99` translate_chunk 对超 500 tokens 的输入 `truncate(Right)` 静默丢后半段。split_and_translate 按标点分段后逐句调 translate_chunk——但无标点单句（1000+ 汉字）仍超限被截断。对比 m2m100 有 200 字符/chunk + is_boundary 硬切（:158-172），opus_mt 无。

**修复**：split_and_translate 循环里检查每句 token 数，超限时字符级硬切（200 字符/chunk，尽量在 is_sentence_end 或空格处切），对齐 m2m100 策略。

### P2-5. 翻译引擎选择静默回退 m2m100 🟠

**根因**：`engine.rs:38-40` 非 opus-mt 前缀一律加载 m2m100——用户配错引擎名（typo）静默回退，以为用 A 实际跑 B。

**修复**：显式判 m2m100 前缀，其他 bail! 报错（支持：opus-mt-* / m2m100*）。

### 驳回 P2-6（pty waiter mutex unwrap）

报告称 `session.rs:333 lock.lock().unwrap()` 中毒 panic。核实：所有 `lock.lock()` 已是 `unwrap_or_else(|e| e.into_inner())`（8 处），中毒锁安全提取。**报告基于旧代码，当前代码已修**。

### 留后续（需更多设计）

- **P2-1** download If-Range 未发 + etag 不校验 → 新旧内容混合
- **P2-2** sidecar 恢复 + 200 返回 counter 虚高 >100%
- **P2-3** scheduler 单任务 hang 阻塞全部（需架构改动）
- **P2-7** pty waiter spawn 失败留僵尸 child（罕见触发）

### 验证

- `cargo build` translation —— 0 error 0 warning。
- `cargo test`：translation 20 全过。

## 24. 第二十一轮审查修复（2026-08-05，4 P2 修 + sync P2 留后续）

第二十一轮全新核查（sync/vault/asr/clipboard 4 核心模块）。报告 10 P2 + ~23 P3。本轮修 4 P2（vault 安全 + asr 兜底/可观测 + clipboard 复活保护），sync 6 P2 留后续（系统性设计），fbank 热路径留后续（性能）。

### P2-v1. resolve_with_remote/local 绕过暴力破解防护 🟠（安全）

**根因**：`engine.rs:893/965` resolve 两函数全程无 attempt_guard——密码验证失败直接返 Err，无 record_failure/退避。DevTools 反复 invoke 暴力破解无指数退避。对比 unlock 路径有完整 attempt_guard（check_remaining + record_failure + reset）。

**修复**：两函数入口加 remaining_wait check + 密码失败 record_failure + 成功 reset，对齐 unlock 模式。

### P2-a3. Whisper transcribe 无空音频守卫 🟠

**根因**：`whisper.rs:340 transcribe` 无 `audio.is_empty()` 守卫。空音频 → compute_mel 对空 slice fold + len=0 除零 → NaN 传播。对比 qwen3_asr.rs:165 有守卫。

**修复**：transcribe 开头 `if audio.is_empty() { return Ok(String::new()); }`。

### P2-a2. aliyun FunASR/Qwen JSON 解析失败静默忽略 🟠

**根因**：`aliyun_stream.rs:167/465` `Err(_) => return HandleOutcome::Continue`——服务端推非 JSON 帧时静默吞。对比 baidu/tencent 解析失败均 warn。

**修复**：补 `log::warn!`（两处：FunASR + Qwen）。

### P2-s5. clipboard pull_favorite active 分支缺反向复活保护 🟠

**根因**：`clipboard.rs:622-638` active 分支直接 upsert + set is_favorite=true，无「DB 已 tombstone + 远程 active 拒绝」检查。对比 hotword pull_set 有此保护。

**修复**：active 分支前查 DB，已 tombstone 则 Ok(false)（拒绝复活，对齐 hotword + vault P2-2）。

### 留后续（需系统性设计）

- **P2-s1** export_all 非原子清空+重建（remove_dir_all 后崩溃 → 残破）
- **P2-s2** merge 阶段 1/2 文件写被阶段 3 覆盖（无效 IO）
- **P2-s3** pull_set/pull_word 吞 DB 错误（混淆 name 冲突）
- **P2-s4** unwrap_or_default 吞损坏 outline → version 假阳性递增
- **P2-s6** pull_favorite tombstone 两步非事务
- **P2-a1** fbank 热路径每次分配 512-Complex vec（性能）

### 验证

- `cargo build` vault/asr-local/asr-cloud/sync —— 0 error 0 warning。
- `cargo test`：vault 264 / asr-local 170 / asr-cloud 62 / sync 153 全过。

## 25. 第二十二轮审查（2026-08-05，merge main + clipboard concealed hint 复审，0 修复）

### 触发

session 接力续审。fetch origin/main 发现较 HEAD 前进 3 commit（`b01a2aa2` docs / `c309b085` feat clipboard concealed hint 跨平台化 / `b4bc05fa` merge）。其中 `c309b085` 是**新增功能代码**（`watcher.rs` 跨平台 concealed type 检测），属健康确认未覆盖范围，触发本轮复审。

### merge main 验证（审查纪律 §2）

fast-forward 合入（`5793ce1b` → `b4bc05fa`）。merge 后重新核实历史修复存活：

- **第 17 轮 P1-1**（FTS5 JOIN `c.rowid = f.rowid`）—— `crates/clipboard/src/store.rs:138/175/177` + 回归测试 :964，存活。
- **第 17 轮 P1-2**（favorite tombstone pull 远程时间戳）—— `crates/sync/src/clipboard.rs:583-594`（注释标「第十七轮 P1-2」），存活。
- **第 21 轮 P2-s5**（`pull_favorite` active 分支反向复活保护）—— `crates/sync/src/clipboard.rs:626-637`（注释标「第二十一轮 P2-s5」），存活。

merge 未覆盖任何历史修复。

### 审查 `c309b085`：跨平台 concealed hint 检测

**代码位置**：`crates/clipboard/src/watcher.rs:86-112`（`handle_clipboard_change` 开头）。

**审查清单**（全部 CONFIRMED 无 bug）：

| 项 | 核实 | 结论 |
|---|---|---|
| 调用链可达性 | `on_clipboard_change` → `on_change` 闭包 → `clipboard_queue::worker_loop` → `handle_clipboard_change` | ✅ 检测会被执行 |
| 与 suppress 机制时序 | `check_and_clear_suppress`（octopus autotype）在 watcher.rs:58 先判定，concealed 检测在 :108 仅对第三方复制生效 | ✅ 设计自洽（注释 :97-98 明示分工） |
| clipboard-rs 版本 | `Cargo.lock` 锁 `0.3.4`；`Cargo.toml` 声明 `0.3` | ✅ 一致 |
| `ContentFormat::Other(String)` 四后端支持 | 核实 clipboard-rs 0.3.4 源码：macos:274 / win:86,144 / x11:480,591 / wayland:174 均实现 `has(Other)` | ✅ commit message 声称属实 |
| macOS cfg 展开 | rustc 模拟编译 `--target aarch64-apple-darwin`：`CONCEALED_HINTS.len() == 1` | ✅ 非空 |
| 编译 warning | `cargo build -p octopus-clipboard` —— 0 error 0 warning | ✅ 干净 |
| 平台 hint 正确性 | macOS `org.nspasteboard.ConcealedType`（nspasteboard.org 约定）/ Windows `ExcludeClipboardContentFromMonitorProcessing`（MS 官方）/ Linux `x-kde-passwordManagerHint`（KeePassXC 事实约定） | ✅ 三平台常量核实 |

**结论**：新代码**审查通过，无需修复**。逻辑正确、cfg 门控正确、依赖核实、0 warning。

### 观察项（不修，记录留痕）

- **结构性测试缺口**：concealed hint 检测无回归测试。但 `ClipboardHandle` 持有 `Mutex<ClipboardContext>`（非 trait），无法在单测 mock，**单元测试不可行是结构性限制，非疏忽**。如需覆盖，需引入 trait 抽象（`ClipboardProbe`）重构 handle——代价超出收益（检测逻辑极简，4 行 `for hint in CONCEALED_HINTS { if handle.has(...) return }`）。留痕不修。
- **空数组边界**：若未来编译目标三平台均不匹配（如 ios/android），`CONCEALED_HINTS` 会变空数组，`for` 循环成 no-op，concealed 检测静默失效。当前 octopus 仅支持 macOS，无实际影响；新增平台时需补对应 hint。

### 验证

- `cargo build -p octopus-clipboard` —— 0 error 0 warning。
- merge main 后历史修复（第 17/21 轮 clipboard）全部存活（Read 核实）。
- 本轮无代码修复，无新增 commit。

## 26. 第二十三轮审查修复（2026-08-05，全代码审查 23 P2 复查 → 修 12 + 驳回 1 + 留后续 10）

### 触发

收到独立的全代码审查报告（标题"第二十二轮全代码审查报告"，23 P2 + ~40 P3，跨 15 crate）。报告自带警告"吸取上轮 P2-6 沿用代理结论致误报的教训，本轮多处代理定级偏高已下调"——故**逐项亲自 Read 复查**，不沿用报告结论。

### 复查结论总览

| 梯队 | 报告项 | 复查结论 | 处理 |
|---|---|---|---|
| 一（数据一致性） | P2-i1/i2/i3/i4（infra 无事务）| ✅ 全 CONFIRMED | **修**（unchecked_transaction 包裹） |
| 一 | P2-rec1（record thumbnail 无事务）| ✅ CONFIRMED | **修** |
| 一 | P2-sync1/s2（export 原子性 + merge 冗余写）| ✅ CONFIRMED | 留后续（需系统性重构 merge_three_way） |
| 二（安全/可靠） | P2-d1（autotype 主线程阻塞）| ✅ CONFIRMED | 留后续（焦点时序脆弱，移线程高风险） |
| 二 | P2-d2（PasswordOnly 缺二次 verify）| ✅ CONFIRMED | **修**（补 verify_focused 对称） |
| 二 | P2-d3（spawn .expect panic）| ✅ CONFIRMED | 留后续（极端系统状态，fail-fast 可接受） |
| 二 | P2-d4（cloud close spawn 不兜 panic）| ✅ CONFIRMED | **修**（catch_unwind） |
| 二 | P2-srv1（scheduler 无超时）| ✅ CONFIRMED | 留后续（= 第 21 轮 P2-s3，需架构改动） |
| 三（性能/资源） | P2-srv2/srv3/srv4/srv5（WS 路径 4 问题）| ✅ CONFIRMED | 留后续（WS handler 系统性重设计） |
| 三 | P2-ocr1（validate 死代码）| ✅ CONFIRMED | **修**（一行激活） |
| 三 | P2-ocr2（EP 回退丢配置）| ❌ **误报** | 驳回（代码无重建 builder 逻辑，配置保留） |
| 三 | P2-ocr3（image 无 size guard）| ✅ CONFIRMED | 留后续（需 API 重构） |
| 三 | P2-l1（error body 泄漏）| ✅ CONFIRMED | **修**（截断 helper） |
| 三 | P2-l2（无重试退避）| ✅ CONFIRMED | 不修（功能增强，非 bug） |
| 三 | P2-l3（response.text 无上限）| ✅ CONFIRMED | **修**（同 P2-l1 helper，一并解决） |
| 四 | P2-dl1（Etag dead code + 注释撒谎）| ✅ CONFIRMED | 留后续（= 第 17 轮 P2-1，需实现 If-Range） |
| P3 | sync thread-local key 未 clear | ✅ CONFIRMED | **修**（RAII guard） |

### 修复明细

#### P2-i1/i2/i3/i4 · infra 4 处多步写无事务（同型漏网）

**根因**：代码库已有 `conn.unchecked_transaction()?` + `tx.commit()?` 范式（vault.rs:327 `insert_vault_ciphers_batch` / mod.rs:410 v57→v58 迁移），但这 4 个函数漏了：
- **P2-i1** `delete_hotword_set_at`（hotword.rs:191-201）：UPDATE words + UPDATE sets，第二条失败→词全软删但 set 活跃=空词典
- **P2-i2** `set_words_in_set_at`（hotword.rs:591-644）：SELECT + 批量软删 + 批量 upsert，2500 词中途失败→半更新脏状态
- **P2-i3** `move_action_bar_item_at`（action_bar.rs:285-286）：两 sort_order 交换，部分失败→两 item 同序号
- **P2-i4** `set_default_agent_at`（agent.rs:70-71）：清零+置1，部分失败→全表无 default

**修复**：各函数体内 `let tx = conn.unchecked_transaction()?; ... tx.commit()?;` 包裹（`_at` 后缀函数接收 `&Connection`，tx `Deref<Target=Connection>`，传 `&tx` 对齐 vault 范式）。

**回归测试**（hotword.rs test mod 末尾加 2 个）：
- `delete_hotword_set_at_rolls_back_words_when_set_missing`：删不存在 set bail 时，前序词软删必须回滚（验证事务价值）
- `set_words_in_set_at_over_capacity_leaves_db_unchanged`：超容量 bail 时原词不动

P2-i3/i4 靠现有成功路径测试（`move_action_bar_item` :808 / `set_default_agent_is_mutually_exclusive` :291）覆盖核心不变量，事务回滚语义由 rusqlite 保证，不加冗余测试。

#### P2-rec1 · record thumbnail 两表 INSERT 无事务

**根因**：`RecordStore::insert`（store.rs:59/76）recordings + recordings_thumbnails 两 INSERT autocommit。第二条失败→recordings 标 has_thumbnail=true 但 thumbnails 表无行，get_thumbnail 返回 None（前端破图）。

**修复**：`let tx = self.conn.unchecked_transaction()?;` 包两 INSERT + `tx.commit()?`。现有 `insert_with_thumbnail` 测试（:364）覆盖"两表同生"核心不变量，不加冗余测试。

#### P2-d2 · PasswordOnly/UsernameOnly 缺二次焦点校验

**根因**：`autotype_login_with_mode`（macos.rs:163-182）UsernamePassword 在 password 前(:170)有 `verify_focused`，但 PasswordOnly(:174)/UsernameOnly(:178) 在 `activate`(:142) + `sleep(150ms)` 后直接注入。activate 操作可能改变焦点（失败/被抢），PasswordOnly 是默认模式却缺校验。

**修复**：PasswordOnly/UsernameOnly 在注入前各补 `verify_focused(expected_bundle_id)?`（与 UsernamePassword :170 对称）。

#### P2-d4 · coordinator cloud close spawn 不兜 panic

**根因**：`rt.spawn(async { timeout(30s, close_async).await; tx.send(CloudStreamingDone) })`（lifecycle.rs:111-127）。timeout 兜超时但兜不住 close_async 内 panic——panic 终止 task，tx.send 永不执行→stage 永久卡 CloudClosing。

**修复**：`std::panic::catch_unwind(AssertUnwindSafe(|| async { timeout(...).await })).await` 包 timeout，match 加 `Err(_) => Err("cloud close panic")` 分支，panic 后仍 send。对齐 polish.rs:112 / paste.rs:102 范式。

#### P2-ocr1 · EngineConfig::validate() 死代码

**根因**：`RapidOcr::new`（rapid_ocr.rs:67）直接构造 Detector/Classifier/Recognizer，不调 `config.validate()?`。200 行校验逻辑（config.rs:52-136：text_score 范围 / limit_side_len 非零 / shape 非零）dormant。当前 default config 不触发，但未来用户配置反序列化（det.std=[0,0,0] 等）→ NaN/除零注入 ort。

**修复**：`new` 最前加 `config.validate()?;`（一行激活）。default config 经测试验证能过 validate（45 测试全过）。

#### P2-l1/l3 · LLM error body 泄漏 + 无大小上限

**根因**：client.rs:124/:299 `response.text().unwrap_or_default()` 整 body 进 `bail!` message → toast/日志。
- **P2-l1**：某些 provider 4xx body echo 请求头（含 `Authorization: Bearer`）或 stack trace，泄漏
- **P2-l3**：恶意 server 返回超大 body，全量入内存

**修复**：提取 `read_error_body(response)` + `truncate_error_body(text)`（纯逻辑分离便于测试）——截断到 500 chars（按字符不切断 UTF-8 边界）+ `...(truncated)` 标记。两处 `bail!` 改用 helper。

**回归测试**：`truncate_error_body_truncates_long_text`——验证短 body 原样、恰好 500 不截、501 截断、中文按字符截。

#### P3-sync1 · clipboard thread-local key 未 clear

**根因**：`merge_clipboard_favorites`（clipboard.rs:531-544）:533 `set_thread_clipboard_key` 但不在结束时 clear。`ClipboardKey` 含 `Zeroizing<[u8;32]>`（sync 加密密钥），spawn_blocking 线程被 tokio pool 复用时 key 残留到下次无关任务（卫生瑕疵，非安全漏洞）。

**修复**：`ClipboardKeyGuard`（RAII，Drop 调 `clear_thread_clipboard_key`）——保证 merge 任何路径（Ok/Err/panic）都 clear。hotword 无 thread-local key（明文同步），不需对称修。

#### A1 · scheduler 单任务 hang 阻塞全部（= 第 21 轮 P2-s3 / 报告 P2-srv1）

**根因**：`run_due_tasks`（scheduler/lib.rs）`for task in tasks { (task.run)() }` 单线程串行。`catch_unwind` 只防 panic 不防 hang——某任务死循环/锁死锁/网络 IO 无超时 → for 循环卡住 → 后续所有任务（vault sync / clipboard 清理 / bigram / hotword GC）永久饿死。

**修复**：
1. `ScheduledTask.run` 类型 `Box<dyn Fn() + Send> → Arc<dyn Fn() + Send + Sync>`——run_due_tasks 每任务 spawn 独立线程时 clone Arc（API 保持接收 Box，`register_task_inner` 内部 `Arc::from`；setup.rs 4 个调用点机械改 `Box::new → Arc::new`）。
2. `run_due_tasks` 加 `task_timeout: Duration` 参数，每任务 `std::thread::spawn`（包 catch_unwind）+ `mpsc::channel` 回报结果，主线程 `rx.recv_timeout(task_timeout)`。超时 → log error + mark_run + 继续下一任务。孤儿线程无法 cancel（Rust 无 cancel token），让其自然结束（hang 多是 IO 等待，资源释放后自愈）。
3. `Scheduler` 加 `task_timeout` 字段（默认 300s，覆盖最慢任务 vault sync 30s），`spawn` 闭包传入。

**回归测试**：`run_due_tasks_timeout_unblocks_subsequent_tasks`——hang 任务（sleep 10s，远超 TEST_TIMEOUT=200ms）+ 正常任务，验证 ① 总耗时 <2s（不阻塞）；② hang 任务 mark_run；③ 后续正常任务执行（counter 递增）。现有 4 个测试（filter 分流 / panic 吞 / mark_run）全更新签名 + 通过。

**注**：曾尝试用 `unsafe` raw 指针绕借用检查（`task.run.as_ref() as *const dyn Fn()` 进 detach 线程）——这是 use-after-free（detach 线程可能在 run_due_tasks 返回后访问已释放的 task.run）。已废弃，改用 Arc 安全方案。

#### C1 · download Etag 注释撒谎（= 第 17 轮 P2-1 / 报告 P2-dl1）

**根因**：`verify.rs:40-42` Etag 分支 `Ok(true)` 永真，注释称"实际 etag 校验在下载请求层 If-Range 206"，但 downloader **不发 If-Range 头**（grep 确认，只有 Range 分段续传）。注释撒谎，Etag 字段是 dead code。

**修复**（保守——实现 If-Range 是功能性增强，留后续）：修正 verify.rs 文档——`Hash::Etag` 加 doc 注释明示"当前**不校验**（no-op 返回 Ok(true)），需 If-Range 实现，建议 manifest 同时配 Sha256 强校验"；`verify` 函数 doc 同步；模块头注释去掉"If-Range 头构造"误导。**If-Range 续传校验的功能实现仍留后续**（衔接第 17 轮 P2-1）。

#### P2-srv5 · server spawn_blocking 无超时

**根因**：`transcribe` handler（main.rs:142-157）`spawn_blocking(move || transcribe_batch(...)).await` 无超时。ASR 引擎某输入死循环（历史 paraformer drain bug 同型）→ 永久占 blocking pool 线程，512 个挂起 → 耗尽 tokio blocking pool。

**修复**：`tokio::time::timeout(Duration::from_secs(120), spawn_blocking(...)).await`，match 加 `Err(_) => 503`（"ASR inference timeout——引擎可能死循环"）。120s 覆盖长音频（60s 音频 RTF 0.5 = 30s 推理）+ 慢模型冷启动。超时后 blocking 线程仍在跑（无法 cancel），让其自然结束（tokio blocking pool JoinHandle drop 后跑到完成再回收，结果丢弃）——对齐 scheduler A1 范式。

### 驳回明细

#### P2-ocr2 · EP 注册失败回退丢弃优化级/线程配置 ❌ 误报

**报告声称**：session.rs:46 Err 分支 `ort::session::Session::builder()` 新建 builder，丢弃原 builder 的 Level3/intra_threads。

**复查**：亲自通读 session.rs:37-93（`new_with_contract`），**代码无重建 builder 逻辑**。:49 建 builder 设 Level3 + intra/inter_threads，:68-75 `resolve_execution_providers` 返回 fallback 时只换 `provider_chain.providers`（CPU only），:73 `builder.with_execution_providers(provider_chain.providers)` 用的是**同一个 builder**（优化级/线程配置全保留）。报告对代码描述错误，置信度 60 已偏低。**驳回**。

### 留后续（本轮确认成立但未修，衔接历史留后续）

| 项 | 原因 | 衔接 |
|---|---|---|
| P2-d1（autotype 主线程阻塞 ~1.5s）| 焦点时序极脆弱（多处 e2e 血泪修复 :25-38/:101-108/:115-159），移子线程是高风险改动，回报（1.5s UI 冻结，低频）不抵风险 | 新留后续 |
| P2-d3（ptt/clipboard spawn .expect）| spawn 失败是极端系统状态（fd/线程上限、内存耗尽），panic vs 降级差异小，fail-fast 更易诊断 | 新留后续 |
| P2-srv2/srv3（WS 绕过 manager + engine 静默忽略）| 需系统性重设计 WS handler（共享 ONNX session + 独立 decoder state），`active_session` 返回共享 session 会串行化并发 | 新留后续（同源） |
| P2-srv4（WAV decode 漏 spawn_blocking）| 曾尝试合并 decode+inference 到 spawn_blocking，破坏 duration_ms 计算 + 控制流，回退；decode 通常 <10ms（小文件），收益有限 | 新留后续 |
| ~~P2-srv5（spawn_blocking 无超时）~~ | ✅ **本轮已修**（见修复明细 §P2-srv5），从留后续移除 | — |
| P2-ocr3（image 无 size guard）| 需 `ImageReader` + 维度限制 API 重构；OCR 输入受 watcher 40MB 限制兜底 | 新留后续 |
| P2-l2（无重试退避）| 功能增强，非 bug | 不修 |
| P2-dl1（Etag If-Range 实现）| ✅ 注释撒谎已修正（verify.rs 文档明示 Etag 当前 no-op）；**If-Range 续传校验功能性实现仍留后续** | = 第 17 轮 P2-1 |
| ~~P2-sync1（export 非原子清空+重建）~~ | ✅ **第二十五轮已实施**（见 [设计 spec](./2026-08-05-sync-export-atomicity-design.md) + [实施 plan](../plans/2026-08-10-sync-export-atomicity.md)），方案 B 先写后清孤儿落地 | — |
| ~~P2-sync2（merge push 写被 export 覆盖）~~ | ✅ **第二十五轮已实施**（pipeline push_or_skip 删 write_file），与 P2-sync1 合并修复 | — |

注：P2-srv1（scheduler 无超时）**本轮已修**（见上方修复明细 §A1），从留后续移除。

### 验证

- `cargo build --release -p octopus-server -p octopus-cli` + `cargo build -p octopus-infra -p octopus-record -p octopus-paddle-ocr -p octopus-llm -p octopus-sync -p octopus-scheduler -p octopus-download -p octopus-desktop` —— 全 0 error 0 warning。
- `cargo test`：infra 193 / record 50 / paddle-ocr 45 / llm 14 / sync 153 / **scheduler 5（+1 新增 hang 超时）** / download 33 / **server 4** / **desktop 525** 全过。
- 新增 4 回归测试：hotword 2（P2-i1/i2 事务回滚）+ llm 1（truncate_error_body 截断）+ scheduler 1（hang 超时不阻塞后续）全过。

## 27. 第二十四轮——留后续项推进（2026-08-06，5 修 + 2 留后续）

### 触发

第二十三轮留后续 11 项中，用户要求依次推进 6 项（P2-d1/d3/srv2/srv3/srv4/ocr3/dl1）。本轮按风险/复杂度从低到高顺序处理。

### 修复明细（5 处）

#### P2-d3 · ptt/clipboard spawn .expect 优雅降级

**根因**：`start_clipboard_worker`（clipboard_queue.rs:41）+ `ensure_thread`（ptt.rs:375）的 `.expect("failed to spawn ...")`——spawn 失败 panic 整个 app。

**修复**：
- clipboard_queue：`start_clipboard_worker` 返 `Result<(), std::io::Error>`；**先 spawn 成功后才 set OnceLock tx**（原实现先 set 再 spawn，spawn 失败后 tx send 到已 drop rx 静默失败 → 剪贴板历史静默不记录，比 panic 更糟）；setup.rs 调用点 `if let Err(e) = ... log::error` 继续。
- ptt：`ensure_thread` 返 `Result<Sender, String>`，spawn 失败 log error 返 Err；`register_ptt` 用 `?` 传播（用户看到「PTT 启动失败」而非 app crash）。

#### P2-ocr3 · image load 无 size guard

**根因**：`OcrEngine::recognize`（engine.rs:149/160）`image::load_from_memory` 默认无维度限制（max_image_width/height = None）。超大长图 → load + to_rgb8 峰值近 1GB OOM。

**修复**：提取 `load_image_with_limits` helper——用 `ImageReader::with_guessed_format` + `reader.limits(Limits{max_image_width/height=Some(8192)})` 在解码前 check 维度。8192×8192 ≈ 67M 像素（RGBA ~268MB < 默认 max_alloc 512MB），覆盖正常截图（4K）+ 长图。两处 `load_from_memory` 替换。

#### P2-srv4 · WAV decode 漏 spawn_blocking

**根因**：`transcribe` handler（main.rs:101-112）`read_wav_16k_from_bytes`（hound 解析 + 重采样，O(n)）在 async handler 直接跑，阻塞 tokio worker；而 :142 transcribe_batch 却在 spawn_blocking——不一致。

**修复**：decode 独立 spawn_blocking（返回 samples，后续 duration_ms + 400 检查 + inference 控制流不变）。第二十三轮曾尝试合并 decode+inference 到一个闭包，破坏 duration_ms 计算 + 控制流，回退——本次改为 decode 独立 spawn_blocking，不破坏结构。

#### P2-dl1 · download Etag dead code 彻底清理

**根因**：第二十三轮 C1 修正了 verify.rs 注释撒谎，但 `Hash::Etag` enum 变体仍在（verify 返 `Ok(true)` 永真占位）。深入核实发现：**所有 manifest（model_manifests.rs 11 个模型）均配 Sha256，无一配 Etag**——If-Range 实现零价值。

**修复**：彻底删 `Hash::Etag` 变体（比"实现 If-Range"更合理）：
- verify.rs：删 Etag 变体 + verify 的 Etag arm + 模块头注释更新
- downloader.rs:462-463：match 去掉 Etag arm（只留 Sha256）
- probe 探测的 etag + ResumeState.etag 字段保留（探测多读一个 header 无害，存 sidecar 未来可能用，删除涉及 resume.rs 签名改动收益不抵）

#### P2-srv3 · WS ?engine= 静默忽略

**根因**：`handle_ws`（main.rs:264/269）:264 校验 engine 是已知 category，:269 `StreamingSession::new(&engine, ...)` 忽略 `&engine`（内部 `resolve_active_engine("asr")`）。客户端传 `?engine=whisper` 但激活引擎是 paraformer 时静默用 paraformer。

**修复**：handle_ws 加校验——`resolve_active_engine("asr")` 拿激活引擎名，与 `?engine=` 裸名比对，不一致显式报错（"流式引擎必须是激活引擎 X，请先 switch_active_engine 或改用 /transcribe 批处理端点"）。`StreamingSession::new` 的设计（强制激活引擎）不变。

### 留后续（2 项，确认本轮不修）

| 项 | 原因 |
|---|---|
| **P2-srv2**（WS 每连接重载 ONNX / 共享 session 重构） | 需拆分 StreamingSession 为「共享 ONNX Session + 每连接独立 decoder state」——asr-local 核心架构重构（streaming_engine/runner/paraformer/zipformer 多文件）。实际场景：desktop 连 server 单连接串行（每次 transcribe 新开连接），非并发 OOM 而是**每次识别冷启动延迟**。单用户场景收益（冷启动）不抵重构风险，留专项 |
| **P2-d1**（autotype 主线程阻塞 ~1.5s） | 1.5s 是用户主动操作（点自动填充）的预期等待，非后台卡顿。焦点时序极脆弱（5 处 e2e 血泪修复），移子线程的 CGEvent/osascript/RunLoop 边缘行为需 e2e 基础设施验证。回报（低频冻结）不抵风险（autotype 回归 = 密码填错窗口） |

### 验证

- `cargo build --release -p octopus-server -p octopus-cli` + 全改动 crate build —— 0 error 0 warning。
- `cargo test`：ocr 35 / download 33 / server 4 / desktop 525 全过。
- 第二十三轮留后续 11 项 → 本轮修 5（d3/ocr3/srv4/dl1/srv3）+ 留后续 2（srv2/d1）+ 前轮已修 4（sync1/sync2 有设计 spec / srv5 已修 / l2 不修）。

## 28. 第二十五轮——P2-sync1/sync2 export 原子化实施（2026-08-10，方案 B 落地）

### 触发

用户指令实施 P2-sync1/sync2（第二十三轮留后续，设计 spec 已写）。按 [设计 spec](./2026-08-05-sync-export-atomicity-design.md) 方案 B（先写后清孤儿）+ 方案 C（删冗余 push 写）落地。

### 实施（5 Task）

#### Task 1 · clipboard export_all 先写后清孤儿
- `export_all_favorites`（clipboard.rs:427）：删 `remove_dir_all(fav_dir) + create_dir_all` 两步，改为只 `create_dir_all(fav_dir)`（幂等 ensure）；写循环收集 `keep_keys`（active + tombstone 都写文件都保留）；末尾调 `cleanup_orphan_favorite_files(&keep_keys)`
- 新增 `cleanup_orphan_favorite_files`：扫 `favorites/<2hex>/*.json`，删 stem 不在 keep_keys 的孤儿

#### Task 2 · hotword export_all 先写后清孤儿
- `export_all_hotwords_with`（hotword.rs:414）：删 :426-437 循环 remove_dir_all 各 set 目录；写循环收集 `keep_set_ids`（未超期 set）；末尾调 `cleanup_orphan_hotword_files`
- 新增 `cleanup_orphan_hotword_files`：两级清理——set 级（不在 keep_set_ids 的 set 目录，含超期 tombstone set）+ word 级（存活 set 内超期 tombstone word 文件）

#### Task 3 · vault export_all_to_files 先写后清孤儿
- `export_all_to_files`（store.rs:516）：删 :528-533 `remove_dir_all(ciphers/folders)`；改为 `create_dir_all` 幂等；写循环收集 `keep_cipher_ids`/`keep_folder_ids`；末尾调 `cleanup_orphan_files` × 2
- 新增 `cleanup_orphan_files`：通用分片清理（cipher/folder 都是 `<2hex>/<uuid>.json`，folder 也分片——纠正设计 spec §5.1 「folder 扁平」的误判）

#### Task 4 · pipeline push_or_skip 删冗余 write_file
- `push_or_skip`（pipeline.rs:240）：不调 `E::write_file(row)`，只记 `report.pushed += 1` 返 true。export_all 全量重建覆盖 push 的写入，1000 收藏省 1000 次无效原子写
- 不变量注释：export_all 必须全量重建；若未来改增量需恢复 write_file

#### Task 5 · 回归测试（5 个）
- clipboard：`cleanup_orphan_favorite_files_removes_stale_keeps_valid`（孤儿删除 + 合法保留）+ `cleanup_orphan_favorite_files_empty_dir_ok`（空目录幂等）
- vault：`cleanup_orphan_files_removes_stale_cipher` + `cleanup_orphan_files_empty_dir_ok` + `export_all_to_files_cleans_orphan_cipher_files`（端到端：预置孤儿 → export → 孤儿被清）

### 验证

- `cargo build --release -p octopus-server -p octopus-cli` —— 0 error 0 warning
- `cargo test`：**sync 155（+2 新）** / **vault 267（+3 新）** / desktop 525 全过
- 设计 spec 状态改「已实施」；audit spec 留后续表 P2-sync1/sync2 移除

### 设计 spec 与实现的偏差

- **folder 也分片**：设计 spec §5.1 称「folder 扁平 `<uuid>.json`」，实际 store.rs:105-112 folder 也是 `<2hex>/<uuid>.json` 两级分片。Task 3 用通用 `cleanup_orphan_files`（cipher + folder 共用），无需为 folder 单独写扁平遍历
- **vault 未迁移到 trait**：设计 spec §5.1 已预见，vault 单独改 `export_all_to_files`（非 trait 路径），与 clipboard/hotword 的 trait 路径并存

## 29. 第二十六轮——第二十四轮全代码审查报告复查（2026-08-10，修 5 + 留后续 2 + 接受反馈 1）

### 触发

收到第二十四轮全代码审查报告（1 P1 + 5 P2 + ~13 P3，前端 TS / Rust panic / 并发 3 代理）。报告头号发现是 **P1 regression——第二十三轮 P2-d4 修复（我的 commit `4478f38d`）引入 cloud feature 编译失败**。报告自省：「上轮复查只 Read 代码标 ✅ 未 cargo check」。

### 🔴 P1-1 · lifecycle cloud 编译失败（我引入的 regression）✅ 已修

**核实**：`cargo check -p octopus-desktop --features "cloud,embedded,vault"` → `error[E0277]: Result<..., ...>` is not a future（铁证）。

**根因**：第二十三轮 P2-d4 我用 `std::panic::catch_unwind(AssertUnwindSafe(|| async {...}))`——catch_unwind 返 `Result<{async block}, _>`，`.await` 作用在 Result 上非法。cloud feature 分支潜伏至今（默认 features 不编译此分支）。

**修复**：改用 `futures_util::FutureExt::catch_unwind`（直接作用在 Future 上）。match 拆两层（原三层 Result 简化）。

**教训**：修复涉及条件编译/feature 的代码，**必须用对应 feature 跑 cargo check**，不能只用默认 features。我之前 `cargo build | tail` 且只用默认 features 验证——P2-6 教训的延伸。

### 🟠 P2 复查结论

| 项 | 复查结论 | 处理 |
|---|---|---|
| **P2-c1**（record 持锁跨 await :287-293）| ✅ CONFIRMED | **留后续**——b"stop\n" 5 字节 << 64KB pipe 缓冲，正常零延迟；触发需 helper hung（octopus 自有 binary 极罕见）；此段有 3 处历史血泪修复（stop 卡 Stopping 系列），改动风险高。报告自己降 P2 conf 55 |
| **P2-c2**（ocr TOCTOU :146-166）| ✅ CONFIRMED | **已修**——`check_and_release_if_idle` inner.lock() 后加一行 double-checked is_idle（经典双重检查锁定，对齐 :206-207 注释意图） |
| **P2-c3**（download 同步 IO :599-695）| ✅ CONFIRMED | **留后续**——async fn 内同步文件 IO（std::fs write/seek/flush）阻塞 tokio worker。SSD 场景 write_all <1ms 影响小；完整修法（tokio::fs 重构 ~100 行流式下载）复杂度高，易回归。下载是低频操作非持续热路径 |
| **P2-c4**（hotword GC 不取 SYNC_LOCK :215-233）| ✅ CONFIRMED | **已修**——GC 入口加 `#[cfg(feature="vault")] try_sync_lock()`（锁忙跳过本次 GC，下次 tick 再试）。防 GC purge 与 sync_now export 并发→已删 set 复活 |
| **P2-c5**（vault meta 绕 META_WRITE_LOCK :528/953/1441）| ✅ CONFIRMED | **已修**——三处 `db::upsert_vault_meta` 改 `save_vault_meta`（后者内含 acquire_meta_write_lock）。防 sync 进行中（持 SYNC_LOCK）与 change_master_password（持 META_WRITE_LOCK）meta 写交错→security_stamp/app_key_sync_enc 回滚 |

### 🟡 P2-l3 半修反馈（接受，注释诚实化）

报告指出我上轮 P2-l3 是半修——`truncate_error_body` 只截断 message 不防 OOM（`response.text()` 仍全量读内存），注释自称"避免全量入内存"与实现矛盾。

**反馈接受**：注释确实是误导（我上轮教训没吸取好）。修正注释——明示 `response.text()` 仍全量读，OOM 修复（streaming + 上限分块）留 P3。实际威胁低：LLM provider 是用户自配 API，GB 级 body 需主动作恶。

### 🟡 P3 择要修复（2 处，对称/便宜）

- **hotword thread-local set_id 缺 RAII**（:984-987）：对称第二十三轮 clipboard P3-sync1——`merge_three_way?` 早返回时跳过 clear_thread_set_id。加 `SetIdGuard`（RAII Drop 清）
- **pty wait_timeout 中毒不对称**（session.rs:285）：`:285 .unwrap()` 改 `.unwrap_or_else(|e| e.into_inner())`，对称同文件 :280/:291/:333 的中毒处理

### 留后续（3 项，均经评估）

| 项 | 原因 |
|---|---|
| P2-c1（record 持锁跨 await）| helper hung 极罕见 + 血泪修复多，改动风险高 |
| P2-c3（download 同步 IO）| SSD 影响小 + tokio::fs 重构复杂，低频非热路径 |
| P2-l3（LLM response OOM）| 用户自配 provider 威胁低 + streaming 重构复杂 |

### 验证

- `cargo check -p octopus-desktop --features "cloud,embedded,vault"`（cloud 路径）+ `--features "remote-ws,embedded,vault"` + `--features "remote-grpc,embedded,vault"` + 默认 —— **全 feature 组合编译通过**（吸取 P1 教训，关键路径必跑多 feature）
- `cargo test`：vault 267 / ocr 35 / llm 14 / sync 155 / pty 32 / desktop 525 全过

## 30. 第二十七轮——全代码审查报告复查（2026-08-10，修 3 + 留后续 4）

### 触发

收到第二十七轮全代码审查报告（3 P2 + 8 P3，错误处理/状态一致性专项）。A 部分确认第二十六轮 6/6 修复落地（P1-1 真编译验证）。

### 修复明细（3 处）

#### P2-3 · opus_mt eos_token_id unwrap_or(0)

**根因**：`opus_mt.rs:71` `eos_token_id` 缺字段 → `unwrap_or(0)`，:75 else 分支也 eos=0。token 0 通常是 pad/BOS——若首步 argmax 恰为 0 → :158 `if next_token == eos_id { break }` 立即 break 返空串；若 0 不被选中 → 跑满 MAX_DECODER_LENGTH=512 输出垃圾。无错误信号。

**修复**：缺 eos_token_id 时 `bail!`（模型必需元数据，缺即不可用），而非危险默认 0。else 分支（无 generation_config.json）也 bail!（opus-mt 标准模型必有此文件）。

#### P2-1（部分）· pull_favorite tombstone 吞错

**根因**：clipboard.rs:685 `let _ = set_clipboard_is_favorite(history_id, false)` 静默吞错。

**修复**：改 `if let Err(e) = ... log::warn`（不阻断——favorite tombstone 已写入，history.is_favorite 残留不影响 sync 正确性，UI 取 favorite 表为准）。

**注**：P2-1 的 active 分支三步无事务（:715/:722/:723）确认成立但**留后续**——三步跨模块调 with_db 函数（upsert_history / upsert_favorite_sync / set_clipboard_is_favorite），包事务需三个 `_at` 版本 + pull_favorite 重构为单 with_db 闭包，中等重构。触发需 ②成功③失败 精确交错 + busy 超时罕见。

#### P3-4 · sync_now 谎报「已推送到远程」

**根因**：vault engine.rs :827 `push_errors.is_empty()` 为 true 有两种情况：①所有 remote 成功；②remotes 为空（git_remote_list 失败或真无 remote）→ for 不执行 → push_errors 空。情况②时仍报「已推送到远程」，用户误以为有云端备份。

**修复**：记 `remotes_was_empty` bool，message 区分——remotes 空 + push_errors 空 → 「本地已保存，未推送（无 remote 配置或 git remote list 失败）」。

### 留后续（4 项）

| 项 | 原因 |
|---|---|
| **P2-1 active 事务**（pull_favorite 三步无事务）| 跨模块 with_db 函数包事务需三 `_at` 版本 + 重构 pull_favorite，中等改动。触发需精确交错 + busy 超时罕见 |
| **P2-2**（sync_now merge 失败仍 commit+push）| 设计权衡——vault 优先不应被 hotword 拖累（:789 注释）。merge 失败后工作树是「部分新部分旧」（非数据丢失），第三设备下次 sync 收敛（merge 幂等）。更好方案是分离 vault/hotword git commit，大重构 |
| **P3-1**（pipeline 读失败 tombstone 误判）| :169-180 读失败→unwrap_or(false) 视为非 tombstone，push 分支（现 export_all）可能覆盖远程 tombstone。触发需读失败+时间戳方向+第三设备三条件叠加。修法（读失败 skip 整个 key）改控制流影响面大 |
| **P3-3**（now_secs unwrap_or(0)）| 时钟异常（早于 1970）主线不可触发，纯理论加固。6 处改 unwrap_or(1) 价值极低 |

### 验证

- `cargo build` translation/vault/sync —— 0 error 0 warning
- `cargo test`：translation 25 / vault 267 / sync 155 全过

## 31. 第二十八轮——并发/锁/竞态专项审查复查（2026-08-10，修 3 + 留后续 2）

### 触发

收到第二十八轮全代码审查报告（并发/锁/竞态专项，1 P1 + 2 P2 + 5 P3 + 9 低置信度）。A 部分确认第二十七轮 3/3 修复落地。

### 🔴 P1-F1 · vault meta 覆盖竞态——改密被 sync 旧值覆盖→永久锁死 ✅ 已修

**逐行核实**（报告置信度 88，亲自 Read 确认触发链）：
1. sync merge_vault 阶段 A :1342 `db::load_vault_meta()` 读 local_meta（stamp=S0）——短读不持锁
2. 期间用户改密（unlock.rs :286 持 META_WRITE_LOCK RMW）→ DB stamp S0→S1 + 新 protected_key K1
3. sync 阶段 stamp 校验 :1372——local(S0) vs 远程(S0) 通过（不读 DB 当前 S1）
4. 阶段 D :1442-1456 用远程值（S0/K0）`save_vault_meta` 覆盖 DB（已 S1/K1）→ 回退 → **新密码失效，永久锁死**

**为什么上轮 P2-c5 挡不住**：P2-c5 让 sync 写走 save_vault_meta（持 META_WRITE_LOCK），但 change 的 _guard 只在函数内持有；sync 阶段 A 读 + 阶段 D 写是两次独立短锁，中间不持外层锁——sync 基于阶段 A 旧快照 + 远程值覆盖 change 新值。

**修复**（方向 2——写前重读 stamp 校验）：阶段 D 写前 :1418 重读 DB 当前 meta，若 stamp 与阶段 A 快照不同 → 期间有 change → bail（用户重试 sync）。不长时间持锁（方向 1 的 merge_vault 整段持 META_WRITE_LOCK 会阻塞所有 meta 写数秒）。

**注**：resolve_with_remote/local 同型但不加保护——它们是前台用户主动操作（输密码等结果），用户不会同时改密（两 modal 互斥）。

### 🟠 P2 留后续（2 项）

| 项 | 原因 |
|---|---|
| **P2-F2**（block_on(do_translate) 同步阻塞 coordinator）| :103 block_on 在 coordinator 线程同步等 LLM 翻译（云端 2-10s），期间 ESC/Toggle 无响应。修复需新 Translating stage + Command::TranslateDone（架构改动大）。触发场景是录音停后的翻译等待（非交互中），回报有限 |
| **P2-F3**（pty_write 持 std Mutex 跨 write_all）| :102 持锁跨 write_all，PTY stdin 写满时阻塞。与 P2-c1（record 同型）一并设计（owned handle 锁外 write）。触发需用户灌满 PTY stdin，且不卡 tokio runtime（spawn_blocking 隔离） |

### 🟡 P3 修复（2 处）

- **P3-F5**（clipboard_cleanup 漏取 SYNC_LOCK）：对称第二十六轮 P2-c4 hotword GC——clipboard_cleanup 任务加 `#[cfg(feature="vault")] try_sync_lock()`（锁忙跳过），防 cleanup 物理删 history 行与 sync set_clipboard_is_favorite(true) 竞态
- **P3-LLM1**（strip_edited_markers 正则过宽）：`<[^<>]*>` 匹配任意 `<...>`，误删 `<div>` / `a<b` / 代码 `i<5`。改为 `<[^<>|]*\|[^<>]*>` ——要求内部含 `|`（hotwords 多候选分隔符），排除 HTML/代码。单候选场景 LLM 返回选定词本身（无 `<>` 包裹），不需此正则

### 验证

- `cargo check` vault + desktop（vault feature）—— 0 error 0 warning
- `cargo test`：vault 267 / llm 14 全过

## 32. 第二十九轮——数据完整性/数值边界专项复查（2026-08-10，修 3 + 留后续系统性短板）

### 触发

收到第二十九轮全代码审查报告（数据完整性 + 数值/索引边界 + 字符串/编码安全 + 资源泄漏，4 代理 fan-out 其中 2 个 429 失败由报告者补审）。A 部分确认第二十八轮 3/3 修复落地。

### 修复明细（3 处）

#### P2-F1 · paraformer enc_slice 越界（离线漏保护 + 流式循环越界）

**根因**：
- 离线 paraformer.rs:169 `enc_tensor.slice(s![0, ..enc_len_scalar, ..])`——enc_len_scalar 来自 ONNX 标量输出，正常 == dim1，但模型损坏/int8 异常时 enc_len_scalar > dim1 → slice 越界 panic。流式版 :608 有 `.min(enc_dim1)` 保护但**修复不完整**——:618 循环 `for i in 0..enc_len` 仍用原始 enc_len，循环到 `i >= enc_dim1` 时 enc_data 越界 panic。

**修复**：3 处（离线 1 + 流式 2）统一 `let effective_enc_len = enc_len.min(enc_tensor.shape()[1]);`，slice 和循环都用 effective_enc_len。

#### P2-F3 · canvas 数据不一致静默返 1×1 黑图

**根因**：capx stitch/mod.rs canvas() :485 + into_canvas() :512 的 `from_raw` 返 None（canvas_buf.len() != w*h*4，数据严重损坏）时，仅 log error + 返 `RgbaImage::new(1, 1)`（1×1 黑图）。调用方（scroll.rs:748/789）拿到后继续编码/入库/剪贴板——用户得空白图且根因被掩盖。

**修复**：canvas() / into_canvas() 改返 `Result`，数据损坏时 `Err` 传播。scroll.rs 两处 match——Err 时 log error + emit `scroll://error` 事件 + return（中止截图流程，不再掩盖）。

#### P3-LLM1 契约闭合 · 单候选标记漏清

**根因**：上轮 P3-LLM1 修复（正则要求含 `|`）引入契约脆弱性——单候选 `vec!["词"]` 经 prompt.rs:124 `cands.join("|")` 产生无 `|` 的 `<词>`，新正则无法清理。当前被 corrector 「>1 候选才 push」隐式挡住，但无编译期保证。

**修复**：prompt.rs:122 注入点闭合——`if cands.len() >= 2 { 包裹<> } else { 原样 push }`。单候选不包裹（无需 LLM 选），契约在注入点显式闭合。

### 留后续（系统性短板）

| 项 | 原因 |
|---|---|
| **P3 F-2~F-8**（ONNX 输出异常 panic）| 6+ 处 ASR 引擎的 argmax/切片无边界检查（qwen3 空 logits / CTC greedy offset 切片 / whisper usize 下溢 / vad 无长度校验 / opus_mt+m2m100 logits offset）。正常模型不触发，异常模型（损坏/int8/ORT 版本差异）才 panic。系统性改造（try_extract_tensor + is_empty bail + offset+len<=buf.len() bail 统一模式）工作量大，留专项 |
| **P3 F4-F8 capx**（u32 乘法溢出 / GrayBuf 无 bounds / 除零）| 当前调用方安全，理论边界加固 |

### 验证

- `cargo build` asr-local/capx + `cargo check` desktop —— 0 error 0 warning
- `cargo test`：asr-local 170 / capx 55 / llm 14 / desktop 525 全过

## 33. 第二十九轮补充——代理 C 补跑复查（clipboard/sync/vault 编码与完整性，修 3 + 留后续）

### 触发

第二十九轮报告的代理 C（clipboard/sync/vault 编码与完整性）补跑成功。2 P2 + 7 P3。整合优先级 1/4/5（P2-F1/P2-F3/P3-LLM1）上轮已修，本轮处理 2/3/6（P2-C1/P2-C2 + C-F9）。

### 修复明细（3 处）

#### P2-C1 · history_row_md5 漏 ref_data/segments/is_rich

**根因**：clipboard.rs:404 md5 只含 4 字段（id/item_type/content/meta_info），漏 ref_data/segments/is_rich。Image/File 的 content 恒空（实际内容在 ref_data），voice 的 segments（润色/编辑段模型）——这些字段变化时 md5 不变 → outline 不 diff → sync 不 push → 远端拿不到新内容，静默数据不一致。

**修复**：md5 补 ref_data + is_rich + segments（7 字段拼接）。注：补字段后已 sync 设备 md5 全变 → 首次 sync 触发全量 conflict（DB 赢 push）→ 最终收敛，非数据丢失。

#### P2-C2 · is_voice_worth_keeping 吞 DB 错误致 voice 物理删

**根因**：clipboard store.rs:316 `unwrap_or(false)` 把 DB 错误（锁竞争/IO/损坏）并入 false → delete_item 走 permanent_delete_item → voice 永久删除（不可恢复，失去 bigram 语料）。删除操作应 fail-safe。

**修复**：match 区分——`QueryReturnedNoRows` → false（行不存在，物理删合理）；其他 DB 错误 → log warn + true（保守软删，宁可多保留）。delete_item + delete_items 两调用点都受益。

#### P3-CF9 · set_sync_md5 吞 DB 写错

**根因**：clipboard.rs:483/513 `let _ = set_sync_md5(...)` 吞错。失败 → 文件 md5 已新但 DB 旧 → 下次 merge 误判 conflict（无效 push + 日志噪声，不影响正确性）。

**修复**：改 `if let Err(e) = ... log::warn`（对齐 P2-1 tombstone 分支 :685 的范式）。

### 留后续（P3 群）

| 项 | 原因 |
|---|---|
| C-F3（iso_to_unix_ms parse 失败默认 1970）| 负时间戳排序异常，正常 ISO 不触发 |
| C-F4（时钟异常 unwrap_or(0)）| 同上轮 P3-3，主线不可触发 |
| C-F5（LIKE 未转义 %/_）| <3 字符回退路径，触发需搜极短含通配符词（罕见） |
| C-F6/C-F7（.ok()/filter_map 吞错无日志）| 观测性改进，不影响正确性 |
| C-F8（序列化 unwrap_or_default 空串）| 序列化失败极罕见 |

### 验证

- `cargo build` clipboard/sync —— 0 error 0 warning
- `cargo test`：clipboard 24 / sync 155 全过

## 34. 第三十轮——ASR/desktop/sync 全覆盖复查（2026-08-10，修 3 + 留后续性能群）

### 触发

收到第三十轮全代码审查报告（3 代理覆盖 ASR 热路径 / desktop·record·pty / sync·vault·clipboard）。A 部分确认第二十九轮 6/6 修复落地。

### 修复明细（3 处）

#### F1 · stitch 均匀滚动第 4 帧起被永久锁定

**根因**：capx stitch/mod.rs:315-351 周期性假匹配检测——连续 3 次 dy 相同时进 stationary check。uniform 分支（:342-345，画面在动→合法）**缺 same_dy_count = 0 复位**。第 3 帧 uniform 后 same_dy_count 仍=3 → 第 4 帧 :318 `same_dy_count >= 3` 命中 + :319-320 dy 匹配 → :321 return Ok(false) **永久锁定**，画布不再增长。注释 :343「not locking」与下一帧 :318 锁定矛盾。

**修复**：uniform 分支末尾补 `self.same_dy_count = 0`。复位后每帧重新走 stationary check（多一道防线防 uniform 误判）。

#### L1-2 · 热词挖掘 ORDER BY id DESC 迁移回归

**根因**：hotword.rs:714/740 `ORDER BY id DESC`——2026-08-05 schema v59 把 clipboard_history.id 从 INTEGER 毫秒戳（有序）改 TEXT UUID v4（随机）后，`ORDER BY id DESC` 变随机字典序。热词挖掘候选取随机历史片段而非「最近输入」（INV-C1 语义破坏）。

**修复**：两处改 `ORDER BY created_at DESC, id DESC`（idx_clip_created 索引现成）。注释同步更新。

#### P3 尼特 · focus_tracker 重复 log

**根因**：focus_tracker.rs:120 if 分支打成功日志，:124 在 if-let 外又打一遍（else 警告分支也打成功日志——误导）。

**修复**：删 :124 重复 log。

### 留后续（性能群，均优化非 bug）

| 项 | 说明 |
|---|---|
| perf-1/2 Zipformer clone | 流式 Zipformer 未跟进 Paraformer 的零拷贝优化（chunk.clone ~24KB + encoder states Tensor::from_array clone）。优化路径明确（照抄 CTC/Paraformer 范式），需 z_perf 性能验证 |
| perf-3 vault 全量 export | merge_vault 末尾无条件 export_all_to_files（1000 cipher SSD ~1s）。有意权衡（注释 :1480-1483），需重新设计 incremental_export 协作 |
| perf-4 pipeline tombstone 双读 | :169 tombstone check read_file → :222 pull_entity 再 read_file。需缓存 file 传 pull_entity，改 trait 签名 |
| perf-5 hotword list_db_rows 全表 | SyncEntity::list_db_rows 全表 list + 内存 filter，schema 有 idx_hotword_words_set 索引却未用。需改 trait 为带 set_id 分桶查询 |

### 验证

- `cargo build` capx/infra + `cargo check` desktop —— 0 error 0 warning
- `cargo test`：capx 55 / infra 193 / desktop 525 全过

## 35. 第三十一轮——资源/并发/panic 专项复查（2026-08-11，修 5 + 留后续 4）

### 触发

收到第三十一轮全代码审查报告（3 代理 cloud-asr/desktop-platform/横向 panic·资源·并发 + 亲审 vault 安全核心）。A 部分确认第三十轮 3/3 修复落地。

### 🔴 P1-1 · run_helper_subcommand 无超时 + 无 kill_on_drop ✅ 已修

**根因**：record/platform/mod.rs:70-76 `Command::new(helper).spawn()?.wait_with_output().await?`——无 timeout、无 kill_on_drop。macOS 权限弹窗等用户确认时 helper 阻塞 → wait 永不返回 → 前端 invoke 永久 await（UI loading 永转）。5 个调用方（list-displays/windows/microphones/check-permission/request-permission）。

**修复**：`tokio::select!` 包 `wait_with_output` + 30s `sleep`（timeout 返 HelperError）+ `.kill_on_drop(true)`（select cancel drop future → drop child → kill）。对齐 session.rs:174 kill_on_drop + :314 timeout 范式。

### 🟠 P2 修复（4 处）

#### P2-1 · filter_speech/segment_audio_vad frame_size=0 除零

**根因**：preprocess.rs:156/205/325 `frame_size * 1000 / 16000` → frame_size=0 时 frame_duration_ms=0 → :157/206 除零 + :162/210 `chunks(0)` / `len()/0` panic。pub API 暴露（audio/mod.rs:17 pub use），内部传 480/512 不触发。

**修复**：3 处（filter_speech + segment_audio_vad + segment_audio_vad_with_offsets）入口加 `if frame_size == 0 || samples.is_empty() { return Vec::new() }`。

#### ptt B1 · manager 线程死亡后 PTT 永久不可恢复

**根因**：ensure_thread :370 只查 PTT_STATE.is_some()，不查 thread_handle.is_finished()。线程死亡（HotkeyManager 创建失败/panic）后保留 stale sender，send() 立即 Disconnected → PTT 永久失效至重启。

**修复**：ensure_thread 加 `is_finished()` 检测——线程死亡则清空 PTT_STATE 重新 spawn。

#### ptt B2 · register_ptt recv() 阻塞主线程

**根因**：register_ptt :447 `rx.recv()` 无超时，是同步 pub fn（set_config Tauri command 调）。manager 卡住时主线程冻结。

**修复**：recv() 改 `recv_timeout(5s)`——正常 register <100ms，5s 充裕。

#### P2-3 · cgimage_to_rgba 假设 bpr>=width*4

**根因**：capture.rs:289 `&raw[row_start..row_start + width*4]`——若 bpr<width*4（理论不变量违反），slice 越界 panic。缺显式 ensure!。

**修复**：入口加 `ensure!(bpr >= width*4, ...)`。

### 留后续（4 项）

| 项 | 原因 |
|---|---|
| P2-2（std Mutex 中毒传染 6 模块）| 系统性 std→parking_lot 替换（6 文件机械改），触发需持锁 panic（当前持锁代码无 panic 路径），防御性加固 |
| P2-4/P2-5（reap 无超时 / PTY 僵尸 child）| 罕见（SIGKILL 通常即效），安全姿态对称 |
| P3 attempt_guard fetch_max | vault 安全核心，需充分测试 |
| P3 dead code crop_region_rgba 删 | 清理非 bugfix |

### 验证

- `cargo build` record/capx —— 0 error 0 warning
- `cargo test`：record 50 / asr-local 170 / capx 55 / desktop 525 全过

## 36. 第三十二轮——sync+db/LLM/commands/FFI 全覆盖复查（2026-08-11，修 4 + 留后续 5）

### 触发

收到第三十二轮全代码审查报告（3 代理 sync+db / LLM-polish-translation-ocr / tauri-commands + 亲审 FFI/安全边界）。A 部分确认第三十一轮 5/5 修复落地。

### 修复明细（4 处）

#### P2-1 · get_model_detail 漏解密 secret_key

**根因**：model_commands.rs:964 `get_model_source_key(id)` 返回 raw secret_key 直接返前端。vault 启用后 DB 存 v1: 密文——前端编辑表单拿到密文（UX 困惑 + trim 损坏密文）。同文件 edit_cloud_model:774 已显式 `try_decrypt_secret_global` 解密，此处对称遗漏。

**修复**：get_model_detail 加 `try_decrypt_secret_global(&secret_key)?`（与 :774 对称）。

#### P2-2 · ptt B3 unregister_ptt recv 无超时

**根因**：ptt.rs:491 `rx.recv()` 无超时——第三十一轮 B2 只修了 register_ptt :461 的 recv_timeout，unregister 对称遗漏。unregister_ptt 也是同步 pub fn（set_config:176 调），manager 卡住冻结主线程。

**修复**：recv() 改 recv_timeout(5s)（对称 B2）。

#### P2-4（clipboard）· export 不过滤超期 tombstone

**根因**：clipboard.rs export_all_favorites 循环不过滤超期 tombstone（对比 hotword export_all_hotwords_with :444 有 is_tombstone_expired 过滤）。GC 启用后 A 机硬删超期 tombstone，B 机 export 仍写文件 → A 机 pull 复活。当前潜伏（clipboard GC 未注册 scheduler）。

**修复**：export 循环加 `is_tombstone_expired(retention, fav.is_deleted, now)` 过滤（对称 hotword）。超期 tombstone 不写文件 + 不进 outline + 不进 keep_keys → 孤儿清理删其文件。

**注**：vault export（store.rs:542-570）同型但留后续——vault 用独立路径（非 trait），改动较复杂，当前同样潜伏（GC 未启用）。

#### P2-5 · generate_subtitle ASR 未 spawn_blocking

**根因**：postprocess.rs:518 `transcribe_segments_with_timestamps` 同步调（async 函数内），注释自称「通常 < ffmpeg 暂不 spawn_blocking」——但长录屏 ASR 可远超 ffmpeg（几十秒）。同文件 :490 ffmpeg 已 spawn_blocking，不一致。

**修复**：包 `tokio::task::spawn_blocking`（对齐 :490 ffmpeg 范式）。

### 留后续（5 项）

| 项 | 原因 |
|---|---|
| P2-3（set_config block_on 主线程）| 改 set_config 为 async 影响面大（多处调用 + 前端 invoke），录屏中改快捷键低频，block_on <1s |
| P2-4 vault export 超期过滤 | vault 独立路径（非 trait），改动较复杂；当前潜伏（GC 未启用） |
| P2-6 terminal_list_dir sync fs 遍历 | 深目录/网络卷阻塞，改 async 影响调用方 |
| P2-7 pin_screenshot recv_timeout 阻塞 | 同步阻塞 tokio worker，置信度 75 |
| P2-8 probe_permission osascript 轮询 | spawn + sleep 阻塞 worker，置信度 75 |

### 纠正第三十一轮 P2-2 名单

实际 std-Mutex 中毒面仅 3 模块（focus_tracker / paste_stack / action_hotkey），非 6——activation/ptt/action_bar_commands 都是 parking_lot（误报）。维持 P3 防御性批量替换。

### 验证

- `cargo check -p octopus-desktop --features "cloud,embedded,vault"` —— 0 error
- `cargo test`：sync 155 / desktop 525 全过

## 37. 第三十三轮——coordinator/infra+actor/pty 核实复查（2026-08-11，修 2 P1 + 留后续 P2 群）

### 触发

收到第三十三轮审查阶段核实报告（3/5 agent 完成：coordinator / infra+actor / pty）。A 部分确认第三十二轮 4/4 修复落地。

### 🔴 P1-1 · 开录音失败 INSTANT_MODE/recording_mode 残留 ✅ 已修

**根因**：set_recording_mode(1/2/3) 在 begin_recording 之前设（mod.rs:466/789/841），但 begin_recording 内部 5 条失败 return 路径（audio.start 失败 / cloud pipeline / streaming engine×3 / vad init）全不清 INSTANT_MODE + recording_mode。残留 mode → ptt.rs next_on_keydown 读残留 mode 走停止分支 → PTT 按键卡死；INSTANT_MODE 残留致下次 Toggle 走错浮窗。对比：cancel/discard/PasteDone 等出口全清，唯独开录音失败路径漏。

**修复**：提取 `reset_mode_flags_on_start_failure()` helper（清 INSTANT_MODE + set_recording_mode(0)），5 处失败 return 前调。

### 🔴 P1-2 · cloud close finalize_cloud 重复 append partial ✅ 已修

**根因**：cloud close 路径——close_async 返回完整文本（含在途 partial，provider Text=stable+sep+partial）。handle_cloud_streaming_done :619 apply_engine_full(text) 把 sep+partial 追加进 transcript，但未清 current_partial。:627 take current_partial 传给 finalize_cloud :506-511，又 append_segment(current_partial) → partial 重复。触发：cloud + 用户快速停止（PTT/instant 松开话音未落）—— cloud PTT 主场景，高频。

**修复**：:619 Ok(text) 非空分支后 `*current_partial = String::new()`——清空防 finalize_cloud 重复 append。

### 留后续（P2 群 6 项）

| 项 | 原因 |
|---|---|
| P2-1 cloud 看门狗断流不 finalize | 需设计断流后的 finalize/restart 路径 |
| P2-2 paste_stack 持锁 with_db | <1ms DB 读持锁，低影响 |
| P2-3 pty Drop 不检查 exited | 边界场景 |
| P2-4 hotword SystemTime unwrap_or(0) | 时钟异常主线不可触发（纯理论加固） |
| P2-5 command_index 子进程无超时 | spawn+wait_timeout 改动 |
| P2-6 pty kill 仅 SIGHUP 无 SIGKILL | portable-pty 上游限制 |

### 验证

- `cargo check -p octopus-desktop --features "cloud,embedded,vault"` —— 0 error
- `cargo test`：desktop 525 全过

## 38. 第三十四轮——download/dlp/capx/paddle-ocr/translation/capx 复审（2026-08-11，修 1 P2 + 2 P3 + 多项证伪）

### 触发

对前 33 轮覆盖较薄的几个独立 crate（download / dlp / capx / paddle-ocr / translation）派 5 个并行 agent 做深度复审。逐条核实 finding 真伪后再动手——这轮重点是「证伪比修复重要」，避免照单全收 agent 的误报。

### 🟠 P2-1 · download 206 路径无 clamp 致段间数据污染 ✅ 已修

**根因**：`download_segment_once_with_client`（downloader.rs）200 路径对每 chunk 做 `write_len = chunk.len().min(remain)` clamp（line 648/670），但 206 路径（line 679-694 旧）裸 `writer.write_all(&bytes)` 无 clamp。非 RFC 合规的服务端/代理/CDN 返回超出 `Range` 请求的字节时，206 路径会把多出的字节写穿 `seg.end` 覆盖**下一段**的区域 → 段间数据污染（最终文件局部错位，hash 校验可能也不报，因为污染发生在段边界处）。

**修复**：206 路径对齐 200 路径范式——计算 `seg_capacity_206 = end - start + 1`，循环条件改 `while written_this_call < seg_capacity_206`，每 chunk `write_len = bytes.len().min(remain)`，写满即退出。同时补回归测试 `download_segment_206_overshoot_clamps_to_segment_range`（mock 服务端对 `bytes=0-9` 返回 20 字节，断言仅写 10 字节 + [10,20) 区间保持预分配 0 未被写穿）。

### 🟡 P3-1 · dlp yt-dlp 调用无超时无 playlist 防护 ✅ 已修

**根因**：`dlp/src/main.rs` 两处 yt-dlp 调用（metadata `--dump-json` line 236 / download line 275）无 `--socket-timeout` / `--retries`，也无 `--no-playlist`。hung TCP 连接或需交互认证的站点会让 yt-dlp 无限挂起 → 整个 ASR 管线（dlp 是 CLI 音频提取前置）无限阻塞。播放列表 URL 会让 `--dump-json` 输出多行 JSON → `serde_json::from_slice` 失败报模糊错（line 252）。

**修复**：两处调用统一加 `--no-playlist`（强制单视频）+ `--socket-timeout 30` + `--retries 3`（网络层超时与重试，比 tokio::time::timeout 包裹更可靠，覆盖 yt-dlp 内部的所有 HTTP 交互）。

### 🟡 P3-2 · dlp 子进程无 kill_on_drop 致 reparent 孤儿 ✅ 已修

**根因**：3 处 `Command` spawn（yt-dlp metadata / yt-dlp download / ffmpeg）无 `.kill_on_drop(true)`。正常完成路径 `wait().await` 会回收，但当 dlp 进程被父进程（CLI/desktop）强制 kill 时，tokio runtime 销毁不走 Drop → yt-dlp/ffmpeg 被 reparent 到 launchd 继续运行（ffmpeg 流式路径 `Stdio::inherit()` 尤甚——stdout fd 不关，ffmpeg 收不到 SIGPIPE 可长时间空跑）。

**修复**：3 处 spawn 统一加 `.kill_on_drop(true)`。注：Ctrl+C 场景整个进程组收 SIGINT，ffmepg 默认 handler 即退，本修复主要覆盖「父进程单方面 kill dlp」的非对称终止路径。

### 🟡 P3-3 · capx 尺寸校验 u32 乘法溢出 ✅ 已修

**根因**：`crop_region_rgba_direct`（capture.rs:224）校验 `rgba_bytes.len() != (full_width * full_height * 4) as usize`——`full_width * full_height * 4` 是 u32 乘法，极端分辨率下 wrap 后可能恰好等于 `rgba_bytes.len()`，骗过校验，后续 `clamp_rect` + 索引拿到错误尺寸 → 越界 panic。现实屏幕 ≤8K×8K 远不到溢出阈值，纯加固。

**修复**：改 u64 算 `expected = (full_width as u64) * (full_height as u64) * 4` 再与 `len() as u64` 比。

### 证伪项（agent 报但经核实非 bug，避免误改）

| 项 | agent 报告 | 证伪依据 |
|---|---|---|
| paddle-ocr P2 `apply_vertical_padding` 当 h>target_h 时 abs_diff 反向 | image_ops.rs:55-71 早返条件 `!use_limit_ratio && h > min_height` 过滤了「h>min_height 且非限宽」分支；可达路径上 `use_limit_ratio=true` → `target_h = (w/ratio)*2 > 2h`，或 `h<=min_height` → `target_h = min_height*2 >= 2h`。**所有可达路径 target_h >= h**，abs_diff 永远是 `target_h - h`，不会反向 |
| paddle-ocr P2 `order_points_clockwise` x 平局错配 | det box 来自 minAreaRect 实际旋转矩形，4 点 x 一般互异；退化的零宽框已被 `rect_width <= 3` 过滤（filter.rs:19）。纯理论 |
| translation P3 opus-mt truncate 丢 EOS | MarianMT encoder 对缺 EOS 鲁棒（attention_mask 而非 EOS 决定边界），500 token 截断丢末尾 `</s>` 不影响输出质量。且 truncate 保守为 500 < 512 上限，是为防 ONNX 位置越界。改了反而风险 |
| download P3 resumed sidecar vs no-range counter 虚高 | 最终持久化 progress（line 491）发正确 total，仅实时进度流瞬态 >100%，cosmetic 非 bug |

### 留后续（agent 报，暂不修）

| 项 | 原因 |
|---|---|
| translation m2m100 缺 repetition penalty（仅 8-token 循环检测） | 功能增强非 bug，opus-mt 路径有完整 penalty，m2m100 是备用引擎 |
| translation m2m100 MAX_DECODER_LENGTH=200 可能截断长译 | 需实测真实翻译 token 长度分布决定是否调到 512 |
| paddle-ocr `get_word_info` debug_assert only（chars/cols 不匹配） | 仅自定义字典场景，stock PP-OCRv6 字典不会触发 |
| capx `canvas_buf_slice` pub 但无 bounds check | 内部调用均先校验，pub 接口暂无外部消费方 |

### 验证

- `cargo build -p octopus-download -p octopus-dlp -p octopus-capx` —— 0 error 0 warning
- `cargo test -p octopus-download --lib` —— 34 过（含新增 `download_segment_206_overshoot_clamps_to_segment_range`）
- `cargo test -p octopus-capx --lib` —— 55 过
- `cargo build -p octopus-cli` —— 0 error（dlp flag 变更不影响 CLI 调用契约，flag 透传 yt-dlp）

### 第三十四轮补充——coordinator session/lifecycle + translation ONNX 边界（2026-08-11，修 1 P1 + 1 P2 + 4 P3）

收到第二轮审查报告（coordinator 全链亲自审 + translation/ocr/llm/record agent）。核实后全部成立。

#### 🔴 P1-补充 · 第三十三轮 P1-1 修复漏 2 处 VAD init 失败分支 ✅ 已修

**核实**：第三十三轮 `reset_mode_flags_on_start_failure()` 只插了 5 处失败分支，漏了 2 处 VAD init 失败分支（报告坐实）：
- `session.rs:169-173`（`prepare_cloud_streaming_session` 的 `create_silero_vad()` Err 分支）
- `session.rs:329-333`（`prepare_vad_segmented_session` 的 `VadSegmentedPipeline::new()` Err 分支）

两处原代码只 `audio.stop()`，不清 `recording_mode`/`INSTANT_MODE`、不 `hide_result`、不 `tray Idle`。且注释谎称 "falling back to VadSegmented/offline"——实际直接 return 无 fallback（误导）。

**后果全链**：`recording_mode(2/3)+INSTANT_MODE` 残留 → `ptt.rs on_keydown(mode)` 走停止分支 → `InstantStop` → `mod.rs:814 "ignored: not recording"` → **PTT 按键卡死（P1-1 原症状未消除）**；识别框/托盘 UI 残留"录音中"。触发条件：silero VAD 模型加载失败（模型未下载/磁盘错误/ONNX init 失败）——现实场景，cloud 路径前置依赖 VAD 尤其脆弱。

**修复**：两处 Err 分支对齐其他 5 处已修分支的完整清理模式（`audio.stop()` + `hide_result` + `tray Idle` + `reset_mode_flags_on_start_failure()`），同时修正误导注释（"abort (no fallback)"）。

#### 🟡 P2-补充 · AgentBridge 分支不清 INSTANT_MODE（双重不对称） ✅ 已修

**核实**：`lifecycle.rs` 两处 AgentBridge 分支（`finalize_after_stop:439-443` + `finalize_cloud:558-562`）的 `dispatch_by_record_type` 返回 true 分支只 `set_recording_mode(0)`，不清 `INSTANT_MODE`（报告坐实）。

**双重不对称**：①同函数空文本分支（`:424-434`/`:515-543`）清了 `INSTANT_MODE`+`hide`+`tray`；②非 AgentBridge 路径经 `do_paste`→`PasteDone` handler（`mod.rs:602 INSTANT_MODE.swap(false)`）清，而 AgentBridge 走 `execute_agent_task`（`agent.rs`，全程不碰 `INSTANT_MODE`），既不经 `do_paste` 也不经 `PasteDone`。

**后果**：instant 模式 + AgentBridge 录音 → `INSTANT_MODE` 残留 → 下次快捷键仍走 instant 浮窗（会话已结束应回普通模式）。

**修复**：两处补 `INSTANT_MODE.swap(false, Ordering::Relaxed)`（2 行）。

#### 🟡 P3-补充 · opus_mt/m2m100 ONNX 输出索引改 `.get()`+bail ✅ 已修

**核实**：`opus_mt.rs:131/152` + `m2m100.rs:80/103` 用 `enc_outputs["last_hidden_state"]` / `dec_outputs["logits"]` 字符串索引（`SessionOutputs::Index<&str>`，缺键 panic）。模型损坏/版本不匹配/输出键名不符时直接 panic（报告坐实，属第三十一轮留后续 P3「ONNX 边界 panic 群」的一部分）。

**修复**：4 处改 `.get("key").ok_or_else(|| anyhow!(...))?` —— 模型不匹配时返回明确错误而非 panic。错误信息标明「模型损坏/版本不匹配」便于诊断。

#### 前端 P3（报告核实，未修，记留后续）

报告自行将前端 5 项降为 P3（经亲自核实 agent 报告偏重）：
- Screenshot interval cleanup 漏 `clearInterval`（`:206-209`）→ P3（Tauri 截图窗销毁时 JS context 清 timer，无实际泄漏）
- Result `enterTranslateMode` setTimeout 无 cleanup（`:360-362`）→ P3（React 18 卸载 setState 无害）
- HotwordPanel IntersectionObserver deps 频繁重建 / ActionBar useEffect 无 deps / Result listen cleanup 无 `.catch` → P3（性能/防御）

#### 验证

- `cargo build -p octopus-desktop --features "cloud,embedded,vault"` —— 0 error 0 warning（P1 cloud-gated 分支覆盖）
- `cargo test -p octopus-desktop --features "cloud,embedded,vault"` —— 541 过 0 失败
- `cargo test -p octopus-translation` —— 0 失败（real-model 测试 ignored）

## 39. 第三十五轮——coordinator pending_prepare 状态泄漏 + download etag dead code + m2m100 truncate 兜底 + backoff/save 加固（2026-08-11，修 1 P2 + 4 P3 + 更新留后续）

### 触发

收到第三十五轮审查报告（coordinator 全链 + translation/download 全 crate，2 agent 并行 + 亲自核实全部 P2）。上轮（第三十四轮）修复核实全部落地（报告用 git blame 确认 `Not Committed Yet` → 已在本轮 commit `bb53e77c`）。本轮新发现逐条核实。

### 🟡 P2-1 · pending_prepare 取消未清 recording_mode（状态泄漏） ✅ 已修

**核实**：`mod.rs:420-424`——进入 pending_prepare 态时 `set_recording_mode(1)`（:466），但取消分支（二次 Toggle 在 200ms 看门狗窗口内）只 `pending_prepare = None`，漏 `set_recording_mode(0)`。坐实：`ptt.rs:306-318` 读 `recording_mode()`，mode==1 残留 → `PttFsm::Idle` 收 keydown 走 `ToggleInWait`（:150-151）→ 与实际 stage=Idle 失步（FSM 错乱，第三次 Toggle 自愈但中间错乱）。

**修复**：取消分支补 `set_recording_mode(0);`（1 行）。

### 🟡 P2-3 · download etag 半成品 dead code（存了不校验） ✅ 已修

**核实**：`ResumeState.etag`（resume.rs:17）被 probe 探测（downloader.rs:141-145）→ new_state 存储 → save 写 sidecar，但 `load` 三重校验（:75-78）只比对 type/total/url_hash，不比对 etag；Range 请求也不发 If-Range。是纯 dead code。

**先例**：`verify.rs` 注释记录第二十三轮 P2-dl1 已删 `Etag` 变体（同样理由：downloader 从未实现 If-Range，所有 manifest 配 Sha256 无一配 Etag）。ResumeState.etag 是同型遗漏。

**修复**：方案 B（删字段）对齐 verify.rs 先例——删 `ResumeState.etag` 字段 + `ProbeResult.etag` 字段 + probe 的 etag 探测 + new_state 的 etag 参数 + 相关测试。旧 sidecar（含 etag 字段）反序列化时多余字段被 serde 忽略，向后兼容。

### 🟡 P3-1 · m2m100 translate_chunk 缺 truncate 兜底 ✅ 已修

**核实**：`m2m100.rs:65-71` translate_chunk 无 truncate（opus_mt.rs:110-113 有）。`split_into_chunks`（:138）理论上保证 chunk ≤ MAX_ENCODER_TOKENS=900，但单句超限（>898 tokens，:159 仅 warn 不拒绝）会穿透到 translate_chunk → encoder 超 900 → ONNX 位置越界。概率极低（m2m100 tokenizer 中文 ~600 字/句、英文 ~4000 字符/句才触发），但补 truncate 兜底对齐 opus_mt 防御。

**修复**：translate_chunk encode 后检查 `input_ids.len() > MAX_ENCODER_TOKENS`，超限时截断保留首（lang_id）+ 尾（eos）。

### 🟡 P3-2 · backoff sleep 不响应 cancel ✅ 已修

**核实**：`downloader.rs:558`（原）`tokio::time::sleep(backoff(...)).await` 不响应 cancel——用户取消时最多等 backoff 时长（指数退避，attempt 1=1s, 2=2s...）才返回（下次循环开头才检测 cancel）。

**修复**：`tokio::select!` 包 sleep + `c.cancelled()`，cancel 时立即返回 `DownloadError::Cancelled`。cancel=None 时裸 await（无 select 开销）。

### 🟡 P3-3 · sidecar save 吞错 ✅ 已修

**核实**：`downloader.rs:240`（原）`let _ = crate::core::resume::save(d, &snapshot)` 吞掉 sidecar 持久化错误——磁盘满/权限失败时用户无感知，断点续传进度可能丢失（下载仍继续，仅续传失效）。

**修复**：改 `if let Err(e) = save(...) { log::warn!(...) }`——错误可观测，不影响下载主流程（sidecar 失败非致命）。

### 留后续更新

#### P2-2 cloud watchdog 死循环（更新根因描述，仍留后续）

第三十三轮 P2-1（spec :1664）的根因已精确化：cloud_pipeline 的 onset 检测（`cloud_pipeline.rs:195-225`）依赖 `tick(samples, ...)` 喂入的 samples。cpal 断推后 samples 空 → onset 永不触发 → WSS 永不重开。watchdog（`lifecycle.rs:246-251`）每 tick 命中 stall→skip restart→还原 stage→return→循环，stage 永久 Streaming，托盘常亮。修法仍需产品决策（cloud 走 audio 重连 or 文档化手动恢复），留后续。

#### 未修 P3 群（低优先）

- `discovery.rs:17` DB Err 静默返空 + `:26` size_mb 永远 0（仅影响 UI 展示）
- `downloader.rs:234` poisoned mutex unwrap_or_else(into_inner) 强取（理论，panic 不应发生）
- `opus_mt.rs:140-178` decoder 锁全程持有无 KV cache（已知性能代价）
- `opus_mt.rs:194-196` 二次 encode 取 token_count（冗余计算）
- `engine.rs:24-29` 缓存 TOCTOU 并发重复加载（wasteful 非 corrupting）
- `scroll.rs:716/778` phys_height 除零（scale=0 理论）+ `:708/765` preview_h u32 溢出（canvas 高 >10M 理论）
- coordinator P3：tick.rs WaitingCompletion 无超时兜底 / lifecycle.rs TRANSLATION_ACTIVE 语义 / agent.rs spawn hide/show 不对称 / cloud close 30s timeout 偏长

### 验证

- `cargo build -p octopus-download -p octopus-translation` —— 0 error 0 warning
- `cargo build -p octopus-desktop --features "cloud,embedded,vault"` —— 0 error 0 warning
- `cargo test -p octopus-download --lib` —— 34 过 0 失败
- `cargo test -p octopus-translation` —— 0 失败（real-model ignored）
- `cargo test -p octopus-desktop --features "cloud,embedded,vault"` —— 541 过 0 失败
