# 2026-07-21 工程性能审计报告（静态扫描，假设清单）

> 方法论：z_perf（measurement-first）。本次为**静态代码扫描 + 模式识别**，所有条目均为**假设**，需 profile 验证才能定性。
> 覆盖范围：Rust ASR 核心 / Rust 桌面端 IPC+并发 / 前端 React+CM6 / DB+IO+内存
> 工作目录：`/Users/wudarui/workspace/agent/octopus/.worktrees/daily-bug-fix`

## P0 执行结果（2026-07-21）

9 条 P0 假设的处理决策与 commit：

| P0 | 决策 | commit | 备注 |
|---|---|---|---|
| P0-1 DB 连接池 | ⏭️ **跳过** | - | 用户决策：不引入 r2d2 依赖，P0-2 spawn_blocking 已隔离主要阻塞 |
| P0-2 async spawn_blocking | ✅ 完成 | `7c9b6bf4` | query/stats/get_image_thumb/get_image_full 4 命令包 spawn_blocking |
| P0-3 paste sleep → polling | ⏭️ **跳过** | - | 用户决策：低 ROI 高风险（无可靠 polling 信号源） |
| P0-4 FTS rebuild 增量化 | ✅ 完成 | `7c9b6bf4` | 移除启动时无条件 rebuild（触发器事务原子性已保证一致） |
| P0-5 watcher 编码后台 actor | ✅ 完成 | `a654afb9` | mpsc + worker 线程，watcher 回调只 enqueue（<1μs） |
| P0-6 Result Toolbar memo | ✅ 完成 | `7c9b6bf4` | 抽取 Toolbar.tsx + memo，流式期间省全树 reconcile |
| P0-7 Screenshot RAF 节流 | ❌ **撤销 + 跳过** | `10ba7e05`（revert）| 实施后用户实测破坏功能（选区不高亮、滚动截图不滚动、预览不一致），已 revert。**重新深度调研后发现原假设错误**：P0-7 报告声称的"双绘"是误判——实际每条 mousemove 路径都是单绘（L410 标注绘制走 ref + 手动 draw 无 setState、L428/L453/460 走 setState + useEffect draw 无手动 draw，互不叠加）。且 Screenshot idle 时完全静止（无 setInterval/RAF/事件循环），用户主动操作的短暂高 CPU 是交互反馈必要代价。结论：**无代码改动**，需要先用 React Profiler / Chrome Performance trace 实测证明是真热点再考虑优化。教训：未实测就断言热点违反 z_perf 护栏 |
| P0-8 fbank mel 稀疏化 | ✅ 完成 | `f96d7223` `e822fac6` | 全 5 路径：fbank/paraformer/streaming_paraformer/zipformer 标准 mel + whisper mel |
| P0-9 Zipformer fbank 增量化 | ⏭️ **跳过** | - | ROI < 3%（fbank 占 9% × P0-8 已省 65% = 剩 3%），midpoint+edge-reflect 边界复杂、CTC+Transducer 双路径风险高 |

**额外修复**（审计外发现）：
- 🔥 微信 osascript Cmd+C 失效：polling 超时按 dispatch 路径动态化，微信 300ms 常量（commit `b813c13d`，已同步 main）

**统计**：9 条 P0 中 5 条实施、1 条实施后 reverted（P0-7 破坏功能）、3 条经评估后跳过（均有明确理由）。

---

## 核心发现（5 大优先级方向）

| # | 方向 | 跨范围重叠 | 预估总收益 | 验证成本 |
|---|---|---|---|---|
| **A** | **DB 单连接读路径阻塞**（架构级） | IPC + DB 两路都命中 | 高并发下尾部延迟 -50% | 高（需基准） |
| **B** | **固定 sleep 等焦点稳定**（GUI 时序） | IPC + 桌面并发 | 每次粘贴 -150~300ms 体感 | 中（端到端） |
| **C** | **fbank 可优化空间集中在 Zipformer 路径** | ASR 核心 | Zipformer 整体 -3~7% | 中（criterion） |
| **D** | **前端鼠标高频事件全量重绘** | 前端 | 拖拽/标注流畅度 +30-50% | 低（React Profiler） |
| **E** | **启动时同步 IO 链**（FTS rebuild + 图片迁移） | DB + IPC | 冷启动 -200ms~数秒 | 低（启动日志） |

---

## P0 — 高优先级假设（证据等级高 + 收益大）

### P0-1. DB 全局单连接 ReentrantMutex 串行化所有读写（架构级）

**位置**：`crates/infra/src/db.rs:121, 243-262`（`static DB: OnceLock<ReentrantMutex<Connection>>` + `with_db`）
**症状**：所有 IPC 命令、后台 cleanup、ASR 写 actor、watcher 写图、读查询全部在同一把锁上排队。WAL 模式本可支持"一写多读并发"，但单连接把它压回串行。
**证据**：高（架构事实）
**影响场景**：剪贴板 watcher 编码 4MB WebP INSERT 期间，前端 `query_clipboard_history`、`clipboard_stats`、`get_image_thumb` 全部排队
**建议方向**：读路径开独立只读连接池（`r2d2`/`deadpool-sqlite`），写仍走单连接 actor
**预估收益**：高负载下尾部延迟 -50%

### P0-2. async Tauri command 直接调 `with_db` 未走 spawn_blocking

**位置**：`crates/desktop/src/clipboard_commands.rs:13,164,30-86`（query/stats/toggle/delete/clear）、`with_db` 同步阻塞
**症状**：`#[tauri::command] async fn query_clipboard_history` 在 Tokio worker 上同步等 DB 锁，阻塞整个 worker 线程
**证据**：高
**建议方向**：所有走 with_db 的 async command 用 `tokio::task::spawn_blocking` 包装（与 `copy_clipboard_item` 同模式）
**预估收益**：避免 Tokio worker 被阻塞导致其他 IPC 抖动
**关联**：与 P0-1 配套（连接池 + spawn_blocking）

### P0-3. 固定 sleep 等焦点稳定（GUI 时序反模式）

**位置**：
- `crates/desktop/src/clipboard_commands.rs:203-214` `paste_clipboard_item` 固定 sleep 300ms
- `crates/desktop/src/paste.rs:132,160` `paste_via_clipboard` sleep 50ms + `PASTE_RESTORE_DELAY`
- `crates/desktop/src/autotype/macos.rs:143,191,202,218,289` 多处 30-150ms
- `crates/desktop/src/clipboard_window.rs:312-327` sleep 100ms 等窗口 ready

**症状**：粘贴/自动输入路径用固定 sleep 等 macOS 焦点稳定，无 polling 反馈，最差情况叠加 300-500ms
**证据**：高
**建议方向**：用 changeCount polling 模式（参考 `action_bar_commands.rs:201-214` 已实施的范式）
**预估收益**：每次粘贴 -150~300ms 体感延迟

### P0-4. 启动时同步执行 `rebuild_fts_index`，每次冷启动全表重建 FTS5

**位置**：`crates/desktop/src/main.rs:490-494`（setup 同步闭包）+ `crates/clipboard/src/store.rs:9-12`
**症状**：注释说"清理上次运行遗留的空洞"，但 `INSERT INTO clipboard_history_fts VALUES('rebuild')` 是全表扫描 + 重建 token。10MB DB 每次冷启动都跑。setup 在主线程同步执行，延后窗口首帧
**证据**：高（无 version gate，每次必跑）
**建议方向**：仅在 cleanup 实际删除行后才 rebuild；或改增量回填
**预估收益**：冷启动 -50~200ms（视条目数）

### P0-5. 剪贴板 watcher 同步编码 WebP + 串行写两次 DB

**位置**：`crates/clipboard/src/handle.rs:126-183`（`handle_clipboard_change` image 分支）；watcher.rs:57-66 回调
**症状**：image 分支在 watcher 回调线程同步执行 `encode_to_webp`（含主图 JPEG + 缩略图 JPEG 两次编码，大图 50-200ms）→ `with_db(insert_image_data)` → `with_db(insert_clipboard_item)` 两次独立锁。回调期间系统剪贴板下一次变更通知被排队
**证据**：高
**建议方向**：watcher 回调入队 mpsc channel，由后台 actor 串行编码 + 单事务写两表
**预估收益**：连续截图/拷图时剪贴板响应延迟 -100~300ms

### P0-6. Result 窗口每次 `update-result` 触发整个组件树重渲染

**位置**：`crates/desktop/frontend/src/pages/Result/index.tsx:191-195` + 整个 Result 函数（18 个 useState）
**症状**：流式事件每次 `setText(payload.text)` 触发 Result 重渲染，含 18 useState + inline 创建的 `tools` 数组（8 个对象 + 8 个 inline onClick），整个 JSX 树每次 reconcile。流式期间高频
**证据**：高
**建议方向**：toolbar 拆独立 `memo` 子组件；text 用 ref，让 AsrEditor 直接订阅 `update-result`
**预估收益**：流式期间每事件省 1 次全树 reconcile，-30~60% 主线程时间

### P0-7. Screenshot 鼠标事件全量重绘 Canvas，无 RAF 节流

**位置**：`crates/desktop/frontend/src/pages/Screenshot/index.tsx:396-464`（onMouseMove）+ `:193-281`（draw）
**症状**：`onMouseMove` 直接 `draw()`，draw 内部 `ctx.drawImage(bg, 0, 0, cssW, cssH)` + 4 次 fillRect 暗遮罩 + 遍历所有 annotations 全量重画。60-120Hz 鼠标事件每次都触发全屏 Canvas 合成。叠加 `draw` useCallback deps 含 `annotations` + useEffect → 每帧重绘 2 次
**证据**：高
**建议方向**：onMouseMove 用 RAF 合批；标注拖动走 imperative path（ref + RAF），mouseUp 时一次性 setAnnotations
**预估收益**：拖拽/标注期间 CPU -30~50%，消除双绘

### P0-8. fbank mel 滤波 `Vec<Vec<f64>>` 全扫描 + 99% 乘 0

**位置**：`crates/asr-local/src/fbank.rs:121-128` + paraformer.rs:515-522 + streaming_paraformer.rs:363-370 + zipformer.rs:1228-1234
**症状**：80×257=20560 次乘加/帧，实际 mel 三角滤波器每行非零区域只有 ~3-4 bins（80×4=320）。`Vec<Vec<f64>>` 行主序访问，缓存不连续。fbank 占整体 9%（z_perf 已知大头）
**证据**：高
**建议方向**：(a) 预计算每行 `(start, end, weights)` 稀疏表只扫非零段；(b) 扁平化 `Vec<f64>` (80*257 连续)
**预估收益**：fbank 省 50-80%（整体 -4~7%），约 1-3ms/chunk

### P0-9. Zipformer fbank 未增量式（对比 Paraformer 已做）

**位置**：`crates/asr-local/src/streaming_zipformer.rs:246-255, 698-707`
**症状**：每 tick 都 `input_samples = history + sample_buffer` 然后 `compute_fbank_features(&input_samples)` 从第 0 帧重算。history 跨 tick 保留但已处理过的帧仍重复计算
**对比**：`streaming_paraformer.rs:283-372` 的 `compute_new_fbank_frames` 是增量式（只算新增帧，已有帧保留在 fbank_cache）——AGENTS.md 已记录
**证据**：高（架构级浪费，对称优化未移植）
**建议方向**：把 Zipformer 的 `history_samples` 升级为 `fbank_cache`
**预估收益**：长录音稳态下 fbank 调用从 O(N) 降到 O(1) per tick；fbank 总开销 -50%+

---

## P1 — 中优先级假设（证据明确但收益次之）

### P1-1. Zipformer state 每 chunk 全量 clone + 重建 Tensor

**位置**：`crates/asr-local/src/streaming_zipformer.rs:766-767, 910-911`（CTC + Transducer）；离线 `zipformer.rs:564-565`
**症状**：每 chunk 都 `arr.clone()` 全部 encoder states（12-16 个 tensor）+ `Tensor::from_array(...)?.into_dyn()`。状态更新时又 `data.to_vec()` + 重建
**对比**：streaming_paraformer.rs:735 已用 view + 预分配
**证据**：高
**预估收益**：每 chunk -0.5~2ms

### P1-2. Zipformer RNN-T 内层 loop 每次 `to_vec()` + 重建 Array2

**位置**：`crates/asr-local/src/streaming_zipformer.rs:806, 832, 844-863`
**症状**：RNN-T emit 内层 loop（上限 20 次/chunk）每次 decoder/joiner 输入输出 to_vec + 重建 Array2
**证据**：高
**预估收益**：RNN-T decode 时间 -5~15%

### P1-3. Zipformer `sample_buffer[consumed..].to_vec()` 每 chunk 重建

**位置**：`crates/asr-local/src/streaming_zipformer.rs:299, 745`
**症状**：用 to_vec 切片重建 buffer，应用 drain 替代
**证据**：高
**预估收益**：省 50% sample_buffer 分配

### P1-4. `zipformer::compute_fbank_features` 每帧重新 `vec![0.0; Z_FRAME_LEN]` 等

**位置**：`crates/asr-local/src/zipformer.rs:1182, 1204, 1211`
**症状**：每帧分配 frame (1.6KB) + preemph (1.6KB) + buf (4KB)。50-100 帧/chunk → ~500 次堆分配
**对比**：streaming_paraformer.rs:308-309 和 fbank.rs:65-66 已用栈数组 + 循环外预分配——zipformer.rs 是唯一遗漏
**证据**：高
**预估收益**：fbank -10~20%

### P1-5. 3 套独立 tick 线程（VAD 100ms / streaming 200ms / cloud 100ms）拍发单 channel

**位置**：`crates/desktop/src/coordinator.rs:1833-1844, 2208-2219, 1866-1878`
**症状**：三个 OS 线程各自 `std::thread::sleep(...)` + `tx.send(Command::XTick)`，全程 5-10Hz 唤醒
**证据**：高
**建议方向**：合并为单 tokio interval（区分 stage）
**预估收益**：-3% CPU

### P1-6. `result_window` click-through poller + `clipboard_dock` edge poller 闲置时仍占 IPC

**位置**：`crates/desktop/src/result_window.rs:141-212`；`crates/desktop/src/clipboard_dock.rs:21-75`
**症状**：33ms tick（30 FPS）全程在线，即使鼠标静止。两 poller 各自调 cursor_position + outer_position + scale_factor 跨语言 IPC
**证据**：高
**预估收益**：闲置 -3~5% CPU

### P1-7. 截图 `scroll://frame` 每帧 emit 双重 JPEG + base64

**位置**：`crates/desktop/src/screenshot_commands.rs:1263-1292`
**症状**：滚动录制 30 FPS，每帧编码 JPEG (frame + preview 各一次) + base64，IPC payload 单帧 100-300KB
**证据**：高
**建议方向**：preview 改前端 canvas 缩放；frame 走 Raw body 二进制；节流到 15 FPS
**预估收益**：录制时 IPC 吞吐 -50%，Tokio blocking 池压力 -50%

### P1-8. ActionBar 渲染期未 memo 的多层 menuItems 过滤

**位置**：`crates/desktop/frontend/src/pages/ActionBar/index.tsx:518-526` + `:751-755`（无依赖 effect）
**症状**：每次渲染都 `menuItems.filter` + 递归 `isSubmenuVisible`；useEffect 无 deps 每次都跑
**证据**：高
**预估收益**：搜索期间每帧省 3-5 次 O(n) 过滤

### P1-9. ImagePreview 画笔绘制每 mousemove 克隆整个 points 数组

**位置**：`crates/desktop/frontend/src/pages/ImagePreview/index.tsx:535-544`
**症状**：`drawingRef.current.points.push(...)` 后 `setDraftAnn({ ..., points: [...points] })` 浅拷贝。画一笔几百点，每 mousemove O(已绘点数)
**证据**：高
**预估收益**：长笔画流畅度提升，省 O(n²) 累积开销

### P1-10. coordinator 终翻路径 `block_on(do_translate)` 卡死 ASR 状态机

**位置**：`crates/desktop/src/coordinator.rs:1567-1585`
**症状**：coordinator 是 mpsc 单消费者线程，终翻路径在它身上 `block_on(do_translate(...))`，云端 LLM 数秒期间所有 Toggle/Cancel/Tick 命令排队
**对比**：`spawn_polish_thread` 已是独立线程没此问题，但终翻不是
**证据**：高
**建议方向**：终翻也走 `tauri::async_runtime::spawn` + `tx.send(Command::FinalPolishDone)`
**预估收益**：终翻期间用户能继续 toggle 录音

### P1-11. 启动时图片迁移 `migrate_images_to_db` 无 version gate

**位置**：`crates/desktop/src/main.rs:508`；`crates/desktop/src/image_migration.rs:41-90`
**症状**：setup 闭包同步循环文件系统 + WebP 编码 + 写 DB，老用户首启可能数百毫秒到数秒
**建议方向**：迁到后台 spawn；空目录 short-circuit；引入"迁移完成"标记位
**预估收益**：冷启动 -200ms~数秒（老用户）

---

## P2 — 低优先级假设（anti-pattern 或微优化）

### P2-1. `query_with_search` 短查询（<3 字符）走 `LIKE '%...%'` 全表扫

**位置**：`crates/clipboard/src/store.rs:131-142, 171-179`
**症状**：中文搜索 1-2 字是常态，FTS5 trigram 阈值 3 字符，每次全表扫
**建议方向**：trigram tokenizer 支持任意 ≥1 字符子串，阈值降到 1
**预估收益**：万级条目短查询从 ~100ms → <5ms

### P2-2. AsrEditor `writeDoc` 每事件 2-3 次 O(n) 字符串操作

**位置**：`crates/desktop/frontend/src/pages/Result/AsrEditor.tsx:269-291`
**症状**：`view.state.doc.toString()` × 2 + `startsWith` × 1，长文本流式时累积
**建议方向**：用 `view.state.doc.length`（O(1)）快速判断 + 维护 `lastSyncedLen`
**预估收益**：长流式文本每事件 -20~40% dispatch 路径耗时

### P2-3. perf_log.rs 热路径同步 open+append 文件 + 持 Mutex

**位置**：`crates/desktop/src/perf_log.rs:46-64`
**症状**：tick 期间 `[SPEAKING]/[STATE]/[CARET]/[APPLY]` 触发频繁，每次 open+append
**建议方向**：mpsc actor 异步写盘，或退化到 `log::debug!`
**预估收益**：长会话 -1~3% CPU

### P2-4. Paraformer `extract_features_from_cache` 每 chunk 重建 Array2

**位置**：`crates/asr-local/src/streaming_paraformer.rs:459-486`
**症状**：每 chunk `Array2::zeros((CHUNK_SIZE, 80))` + 逐元素拷贝 + apply_lfr 内部又 zeros
**建议方向**：预分配 feat_buf 复用
**预估收益**：每 chunk -40KB 分配

### P2-5. `decode_tokens` 长录音全量重解码

**位置**：`crates/asr-local/src/streaming_paraformer.rs:176-181, 243-248`
**症状**：每新 token 都全量重解码 all_token_ids，长录音累积放大
**建议方向**：维护已解码 prefix 长度，只解新增 token
**预估收益**：长录音（>30s）每 tick -0.5~2ms

### P2-6. `useClipboardHistory` 串行 await 两个独立 invoke

**位置**：`crates/desktop/frontend/src/hooks/useClipboardHistory.ts:22-34`
**症状**：query_clipboard_history + clipboard_stats 串行 await，可 Promise.all
**预估收益**：列表刷新 RTT 减半（-5~30ms）

### P2-7. `tauri::async_runtime::spawn` fire-and-forget 任务无 cancel

**位置**：`crates/desktop/src/result_window.rs:301-306, 384-389`；`screenshot_commands.rs:215-223`；`tray.rs:156`
**症状**：多数有 session 哨兵保护，但部分只判 session id，窗口 destroy 时 task 仍跑完
**建议方向**：JoinHandle + 窗口 destroy 钩子 abort
**预估收益**：避免遗留任务卡 Tokio runtime

### P2-8. `WindowEvent::Moved` 多窗口注册，60Hz 拖拽期间每窗闭包 + 多重 IPC

**位置**：`crates/desktop/src/result_window.rs:72-77`；`clipboard_window.rs:102-149`
**症状**：clipboard_window 在 Moved 回调里多重 IPC（detect_dock_edge + load_dock_state + 可能 save_dock_state），节流仅覆盖 save_current_position
**建议方向**：节流提前到回调最前；或 RAF coalescing
**预估收益**：拖拽悬浮窗 CPU -10~30%

### P2-9. `cleanup_unreferenced_images` 嵌套子查询全表扫

**位置**：`crates/clipboard/src/store.rs:338-344`
**症状**：每次 `delete_items`/`clear_history` 都 `DELETE FROM image_data WHERE hash NOT IN (SELECT DISTINCT ref_data FROM clipboard_history)`
**建议方向**：单条删除走 `delete_image_if_unreferenced(hash)`（已存在）；批量定向清理
**预估收益**：单条删除 -30ms（万级条目）

### P2-10. 录音 samples Vec<f32> 非编辑态无界增长

**位置**：`crates/desktop/src/audio.rs:18, 315-324`
**症状**：正常录音（非编辑态）buffer 无上限，1 小时 48k 单声道 ≈ 691MB。若流式 tick 卡住（如加载引擎）会堆积
**建议方向**：录音态加软上限报警；或预分配容量
**预估收益**：长录音内存峰值可控

### P2-11. VAD Session 单例 + Mutex，多会话串行

**位置**：`crates/asr-local/src/vad.rs:19-67`
**症状**：单 VAD Session 跨所有会话共享，多路并发时串行 run
**建议方向**：每路录音会话独立 Session clone（需确认 ORT Rust binding 支持）
**预估收益**：多路并发 ASR 吞吐 +N 倍

### P2-12. fbank statics 多处重复（paraformer 与 fbank.rs 各持一份）

**位置**：`crates/asr-local/src/fbank.rs:25-27` + `paraformer.rs:24-32` + `qwen3_asr.rs:40-41`
**症状**：多引擎各持一份 fbank 静态表 + corrector 字典常驻
**建议方向**：收敛到 fbank.rs 一份
**预估收益**：内存 -10~30MB

### P2-13. Zipformer `token_buf` 用 `Vec::remove(0)` 做滑动窗口

**位置**：`crates/asr-local/src/streaming_zipformer.rs:830` + `zipformer.rs:974`
**症状**：`Vec::remove(0)` 是 O(n)（虽然 context_size=2 实际开销小）
**建议方向**：VecDeque 或环形缓冲
**预估收益**：<0.1ms/chunk（主要价值是去 anti-pattern）

---

## 关键观察

### 跨范围重叠（同一根因多症状）

1. **DB 单连接串行**：H2（IPC 报告）+ H5（DB 报告）+ watcher 写阻塞（H1 DB 报告）+ cleanup 争锁——根本是同一架构问题。**单点修复（连接池）收益最大**
2. **固定 sleep 等焦点**：H4（IPC）+ M14（并发 paste.rs）——同一 GUI 时序反模式，**polling 化统一解决**
3. **Zipformer 路径缺优化**：H1/H2/H3/H4/H5/H7/H8（ASR 报告）——Paraformer 已做的优化未对称移植到 Zipformer。**P0-8/P0-9 + P1-1/2/3/4 集中修复 Zipformer 路径**

### 已正确处理（不再报告）

- 截图 `capture_all_monitors` 已 `spawn_blocking` + `thread::scope` 并行（最近改动）
- `clipboard_commands.rs` 大部分重 IO 命令已 `spawn_blocking`
- 翻译/润色 `spawn_polish_thread` 独立线程
- `db_queue.rs` 用 actor 模型处理 ASR Finalize
- `action_bar_commands.rs:201-214` 已从 sleep 改 polling
- DB 已开 WAL + busy_timeout
- AGENTS.md 记录的 Paraformer 全部优化已实施
- 所有前端 `listen()` 都有 cleanup
- ImagePreview Canvas sticky + 视口切片
- Settings visibleNavItems useMemo 缓存

### 项目特点

- **Paraformer 路径优化深度 > Zipformer 路径**：AGENTS.md 记录的踩坑集中在 Paraformer，Zipformer 路径粗糙
- **z_perf 痕迹明显**：AsrEditor writeDoc 已有 perf 打点（8ms/800字符阈值），可直接复用验证
- **团队对性能有意识**：大量优化痕迹（append 快路径、视口切片、handlerRef、Compartment reconfigure）——剩余问题集中在 mousemove/流式事件热路径 + DB 架构

## 验证路径建议（按 ROI）

| 顺序 | 假设 | 验证方式 | 成本 |
|---|---|---|---|
| 1 | P0-4 FTS rebuild | 启动日志埋点 `Instant::now` | 低 |
| 2 | P0-3 paste sleep | 端到端计时 | 低 |
| 3 | P0-6 Result 重渲染 | React DevTools Profiler | 低 |
| 4 | P0-7 Screenshot 重绘 | Chrome Performance trace | 低 |
| 5 | P0-1/P0-2 DB 连接池 | 并发负载基准（watcher 写时 query） | 中 |
| 6 | P0-8/P0-9 fbank | criterion fbank bench | 中 |
| 7 | P1-1/2/3/4 Zipformer | criterion zipformer chunk bench | 中 |

## 报告约束

本报告为**假设清单**，所有条目均需 profile 验证后才能定性。z_perf 护栏：
- **Measure before change**：没有 baseline 数据，不动代码
- **One variable at a time**：每次只改一个变量
- **Correctness > speed**：优化后必须跑相关 crate 的 `cargo test --lib`
- **Evidence before assertions**：声称"修好了"前必须有 reproducible 测量数据
