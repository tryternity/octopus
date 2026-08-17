# 全量审查「留后续」技术债清单

> **来源**：从 [2026-08-03-full-audit-bugfix.md](../superpowers/specs/archived/2026-08-03-full-audit-bugfix.md)（48 轮滚动审查，已归档）提取的全部未实施项。
> **条目格式**：`[主题] 问题（轮次，归档 spec 行号）`——行号指向归档 spec，便于回溯原始分析。
> **维护约定**：修复后在此划掉（~~删除线~~）+ 注明轮次/commit；新审查轮次发现的留后续项追加到这里，不再回写归档 spec。
> **已闭环**（曾记留后续、后在后续轮次修掉，无需再列）：scheduler 超时、Etag/If-Range（删）、export 原子化（P2-sync1/sync2）、md5 漏字段、push 冗余写、m2m100 无 penalty、vault GC 无事务、merge_hotwords N×M 等。

## A. 并发 / 锁 / 阻塞

- [锁跨await] P2-c1 record stop() 持锁跨 stdin write（helper hung 才触发，3 处血泪修复区改动风险高）（R26，:1308）
- [锁跨await] P2-F3 pty_write 持 std Mutex 跨 write_all（与 P2-c1 一并设计 owned handle）（R28，:1405）
- [阻塞] P2-F2 do_translate block_on 同步阻塞 coordinator 2-10s，ESC/Toggle 无响应（需新 Translating stage，架构改动）（R28，:1404）
- [阻塞] P2-3 set_config block_on 主线程（改 async 影响面大）（R32，:1627）
- [锁] P2-2 paste_stack 持锁 with_db（<1ms 低影响）（R33，:1665）
- [锁] vault_set_lock_timeout 持锁跨 DB IO（R40，:2069）
- [阻塞] P2-d1 autotype 主线程阻塞 ~1.5s（焦点/keystroke 时序极脆弱，移线程高风险）（R23/R24，:1174）
- [阻塞] P2-6 terminal_list_dir 同步 fs 遍历（深目录/网络卷）（R32，:1629）
- [阻塞] P2-7 pin_screenshot recv_timeout 阻塞 worker（R32，:1630）
- [阻塞] P2-8 probe_permission osascript 轮询阻塞 worker（R32，:1631）
- [阻塞] P2-c3 download async fn 内同步文件 IO（tokio::fs 重构 ~100 行，低频非热路径）（R26，:1310）
- [阻塞] #10 ASR transcribe 未 spawn_blocking（代码注释自承认的性能债）（R2，:97）
- [锁] P2-2 std Mutex 中毒传染 3 模块（focus_tracker/paste_stack/action_hotkey，parking_lot 批量替换防御性）（R31/R32，:1579）
- [阻塞] P3-1 settings_commands block_on（R14，:687）

## B. 数据完整性 / 事务 / schema 迁移

- [事务] clipboard pull_favorite active 分支三步 DB 写无事务（跨模块 with_db 需三个 `_at` 版本；自愈性）（R21/R27/R39/R40，:998/:1370/:2019/:2070）
- [事务] P2-K 迁移链 v55/v56/v59 未包事务（断电留不一致，WHERE 幂等兜底）（R36，:1888）
- [事务] v56→v57 迁移无事务（非破坏性、重启幂等，留技术债）（R11/R40，:484/:2066）
- [事务] v57→v58 的 PRAGMA user_version 未入事务（R17 P3-10，:853）
- [事务] P2-2 sync_now merge 失败仍 commit+push（vault 优先不被 hotword 拖累的设计权衡；分离 git commit 需大重构）（R27，:1371）
- [迁移] P2-1 Transcript.id i64→String schema v59 迁移收尾（Transcript + DbCommand + cancel_discard 反推时长，工作量大）（R17，:846）
- [sync] P2-s3 pull_set/pull_word 吞 DB 错误（混淆 name 冲突）（R21，:996）
- [sync] P2-s4 unwrap_or_default 吞损坏 outline → version 假阳性递增（R21，:997）
- [sync] P3-1 pipeline 读失败 tombstone 误判（读失败→当非 tombstone→可能覆盖远程 tombstone；改控制流影响面大）（R27，:1372）
- [sync] P2-H hotword 同秒冲突 ping-pong（datetime('now') 秒精度，需跨设备同秒编辑）（R36，:1885）
- [背压] P3-11 db_queue unbounded mpsc 无背压（R17，:854）

## C. GC / tombstone / 删除语义

- [tombstone] P1-3 vault permanent_delete 只删 DB 不写 tombstone → 他机 sync 复活（需 cipher tombstone 设计，对称热词）（R3，:166）
- [GC] vault export 不过滤超期 tombstone（独立路径改动大；现以 upsert 超期守卫 defense-in-depth 代替，设计取舍留痕）（R32/R37，:1628/:1935）
- [删除] P2 delete_transcriptions_at 物理删 voice 不走软删（设置页删 voice = bigram 语料丢失；跨 crate 架构）（R44/R45，:2237/:2270）
- [GC] vault permanent_delete 单项缺 sync lock（R42，:2158）
- [sync] P2-I vault_sync_now spawn drop 不 await（closure panic → catch_unwind 兜底 emit，UI 永久 syncing）（R36，:1886）
- [sync] P2-J git.rs 全模块无超时（远程挂起 → SYNC_LOCK 永占）（R36，:1887）

## D. ONNX / ASR 数值边界（系统性短板群）

- [ONNX] P3 F-2~F-8 ONNX 输出异常 panic 群残余：qwen3 空 logits / CTC greedy offset 切片 / whisper usize 下溢 / vad 无长度校验（系统性 try_extract_tensor + bail 模式留专项；opus_mt/m2m100 索引与 paraformer CIF 部分已修）（R29，:1448）
- [ONNX] streaming_zipformer log_probs_shape/enc_shape 无 rank 校验（对比 firered.rs 有 bail）（R40/R45，:2067/:2271）
- [ONNX] streaming_paraformer decoder_num_blocks 越界（建议 .get() Option）（R17 P3-4，:848）
- [ONNX] qwen3 shape 裸索引 / whisper n_tokens=0（R45，:2271）
- [ASR] streaming_runner finish_with_tail 在 vad.is_none() 绕过 seen_speech 门控（R40，:2068）
- [ASR] paraformer 缓冲累积依赖外部 reset 契约（5 个调用点正确，建议 debug_assert）（R38，:1983）
- [ASR] streaming cross-chunk dedup / streaming flush 丢尾帧（R42，:2158）
- [ASR] A2 Whisper Hann 窗 symmetric vs periodic（改前需 A/B 量化 WER + 重生成 golden）（R5，:247）

## E. 翻译 / cloud ASR

- [翻译] opus_mt/m2m100 分段翻译单段失败 `?` 丢全部已译段（需产品决策：部分成功 vs 全失败重试）（R41，:2102）
- [翻译] m2m100 MAX_DECODER_LENGTH=200 可能截断长译（需实测 token 长度分布）（R34，:1720）
- [翻译] opus_mt decoder 锁全程持有无 KV cache（已知性能代价）（R35，:1824）
- [翻译] opus_mt 二次 encode 取 token_count 冗余计算（R35，:1825）
- [翻译] P3-E engine_dispatch is_cloud 不对称（ByteDance/Tencent/Baidu 落 embedded.transcribe 会失败；实际 streaming 不走 dispatch，不触发）（R15，:729）
- [cloud] cloud watchdog 断流死循环（cpal 断推→onset 永不触发→WSS 永不重开→stage 永久 Streaming 托盘常亮；修法需产品决策）（R33/R35，:1664/:1816）

## F. 性能（优化非 bug）

- [性能] P2-a1 fbank 热路径每次分配 512-Complex vec（R21，:999）
- [性能] perf-1/2 流式 Zipformer clone（chunk ~24KB + encoder states Tensor clone；照抄 Paraformer 零拷贝，需 z_perf 验证）（R30，:1527）
- [性能] perf-3 vault merge 末尾无条件全量 export（1000 cipher SSD ~1s，需 incremental_export 协作重设计）（R30，:1528）
- [性能] perf-4 pipeline tombstone 双读（需缓存 file 传 pull_entity，改 trait 签名）（R30，:1529）
- [性能] perf-5/P2-G hotword list_db_rows O(N²) 全表 filter（schema 有 idx_hotword_words_set 索引未用；10×3000 词 sync 卡秒级）（R30/R36，:1530/:1884）
- [性能] P2-l3 LLM response 无上限全量入内存（truncate 只救 message 不防 OOM，streaming 重构留 P3）（R23/R26，:1180/:1318）
- [性能] P2-srv2 WS 每连接重载 ONNX / 共享 session + 独立 decoder state 重构（单用户冷启动收益不抵 asr-local 核心重构风险，留专项）（R23/R24，:1176/:1240）
- [性能] P3 群：export 批量化 / pull 重复解密 / discovery size_mb=0 / now_string UTC / scroll clone churn（R36，:1889）

## G. 安全 / 防御加固

- [转义] L4 热词候选 join("|") 无转义（中文热词几乎不含 `|`，影响极低）（R5，:248）
- [转义] LIKE 未转义 `%`/`_`（<3 字符回退路径才触发）（R14/R29b，:493/:1488）
- [安全] P3 attempt_guard fetch_max（vault 安全核心，需充分测试）（R31，:1581）
- [边界] capx canvas_buf_slice pub 但无 bounds check（内部调用均先校验，暂无外部消费方）（R34，:1722）
- [边界] capx F4-F8：GrayBuf 无 bounds / 除零（当前调用方安全，理论加固；u32 乘法溢出已修）（R29，:1449）
- [防御] paddle-ocr get_word_info debug_assert only（chars/cols 不匹配，仅自定义字典场景）（R34，:1721）
- [时间] now_secs/SystemTime unwrap_or(0)（6 处改 unwrap_or(1) 价值极低，时钟异常主线不可触发）（R27/R29b/R33，:1373/:1487/:1667）
- [时间] C-F3 iso_to_unix_ms parse 失败默认 1970（负时间戳排序异常）（R29b，:1486）
- [观测] C-F6/C-F7 .ok()/filter_map 吞错无日志；C-F8 序列化 unwrap_or_default 空串（R29b，:1489）

## H. 行为决策 / 设计取舍 / 已知限制

- [行为] P3-C tencent final=1 stable 空发 Finished（全静音发 Finished 比 Failed 合理，设计取舍）（R15，:763）
- [行为] P3-2 record reader task EOF 静默退出 state 卡 Recording（ESC 可靠恢复；主动崩溃通知是行为变更）（R13，:600）
- [行为] P2-l2 LLM 无重试退避（功能增强非 bug，不修）（R23，:1180）
- [行为] P3-3 capx finalize footer content_tail（注释↔实现不符，需开发者核实产品意图）（R17，:847）
- [pty] P2-7 waiter spawn 失败留僵尸 child（罕见触发）（R20，:957）
- [pty] P2-3 Drop 不检查 exited（边界场景）（R33，:1666）
- [pty] P2-6 kill 仅 SIGHUP 无 SIGKILL（portable-pty 上游限制）（R33，:1669）
- [pty] P2-4/P2-5 reap 无超时 / PTY 僵尸 child（罕见，安全姿态对称）（R31，:1580）
- [pty] reader/flusher spawn 失败路径与 waiter 不对称（session Arc drop killer 兜底）（R38，:1981）
- [子进程] P2-5 command_index 子进程无超时（R33，:1668）
- [截图] capture P2 ×4：area.rs monitor 匹配单位错位 / scroll.rs `?` 降级 / clone×2 / 双份内存（需核实 monitor_x 单位 + xcap 接口）（R19，:923）
- [FFI] P3-1 NSScreen 裸指针无 autoreleasepool / P3-2 SendWindow unsafe Send 无运行时主线程断言 / P3-3 NSPasteboard worker 无 autoreleasepool（形式问题当前不触发）（R16，:796）
- [前端] Screenshot interval cleanup 漏 clearInterval / Result enterTranslateMode setTimeout 无 cleanup / HotwordPanel IntersectionObserver deps 重建 / ActionBar useEffect 无 deps / Result listen 无 .catch（均 P3）（R34 补充，:1765）
- [前端] SyncPanel 中文硬匹配后端错误消息（脆弱契约，待错误码化一并处理）（R13，:609）
- [前端] clipboard_dock POLL_ACTIVE dock 切换边旧 poll 线程（R13，:608）
- [测试] concealed hint 检测无回归测试（ClipboardProbe trait 重构代价超收益，结构性限制留痕）+ CONCEALED_HINTS 未来平台空数组静默失效（R22，:1042）
- [已知限制] L1-a strip_edited_markers 字面花括号误抹/嵌套泄漏（ASR 文本几乎不含代码语法，测试钉死）（R6，:271）
- [竞态] L5 detect_selection restore 前 clear_suppress 微秒竞态（窗口极窄，仅多存一条记录）（R5，:249）
- [死代码] upsert_vault_folder_sync 写 datetime('now')（零生产调用，误用致 ping-pong，建议删除）（R40，:2065）
- [设计] vault md5 故意不含 is_deleted（设计取舍，P2-2 守卫兜底）（R14，:692）
- [autotype] P3-2 copy_concealed 吞错（罕见双失败）（R14，:688）
- [coordinator 杂项] tick WaitingCompletion 无超时兜底 / agent spawn hide/show 不对称 / cloud close 30s timeout 偏长 / discovery DB Err 静默返空 / downloader poisoned mutex 强取 / engine 缓存 TOCTOU 并发重复加载 / scroll phys_height 除零 + preview_h u32 溢出（R35，:1822）
- [P3 群] R42 13 项：Auto-Type AX 失败仍继续 / verify 不 reset 计数器 / verify_ssh_key stdin null / send_command Starting 态 / InstantStop 覆写 PolishNow / restart_capture 尾音丢弃 / capx canvas_heal 边界 / paddle-ocr unclip 多边形 / clear_voice_aware unwrap_or(0) 等（R42/R45，:2158/:2271）
