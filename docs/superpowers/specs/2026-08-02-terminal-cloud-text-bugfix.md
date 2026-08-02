# 终端 / 云识别 / 文本后处理批量 bugfix

> **日期**：2026-08-02
> **状态**：✅ 已实现（18 处 bug 全修复，全量测试通过）
> **来源**：外部代码审查报告（32 候选核实 13 成立 + 5 次要），全量复查 CONFIRMED
> **分支**：`.worktrees/bugfix_pr_0801`（bugfix/pr-0801）

## 0. 复查结论

外部报告的 13 个问题 + 5 个次要项**全部 CONFIRMED，无一驳回**。核实中纠正了几处措辞（不影响成立性）：

| 报告措辞 | 实际情况 |
|---|---|
| #8「per frame」 | 实为「per `correct()` 调用」（每稳态句一次，非每音频帧） |
| #10「K 处黑名单」 | K 是 26 个黑名单词的匹配总和 |
| #13「flusher 永久自旋」 | 不是无限自旋（waiter 正常完成时 done 会被置位）；是 sessions map 残留 Arc+死 PTY fd 直到应用退出 |
| fontPrefs「jsdoc 与代码矛盾」 | 仅 `@returns` 一行反了；函数名/代码/块注释都对 |

## 1. 终端严重（🔴）

### 1.1 gitignore 过滤语义反向（#1）

**根因**：`crates/desktop/src/commands/terminal_commands.rs:251 git_ignored_names` 用 `ignore::WalkBuilder`（默认 yield 非 ignored、skip ignored），返回的实为「要显示的集合」；调用方 `:228` 当「要隐藏的集合」用 → git repo 内 `src/`、`Cargo.toml` 全隐藏，反而只显示 `target/`、`node_modules/`。单测用 tempdir（非 repo）走 `in_git_repo=false` 短路，掩盖 bug。

**修复**：改用 `ignore::gitignore::GitignoreBuilder` 构建匹配器：
1. 从 `dir` 及其祖先向上到 repo root，逐级 `builder.add(.gitignore)`
2. 加 `.git/info/exclude`（repo root 下）
3. 加全局 excludesfile（`gitconfig_excludes_path()`）
4. 对 `dir` 的每个直接子项 `matcher.matched(path, is_dir)`，`Match::Ignore` → 加入「要隐藏的集合」

重构 `in_git_repo` → `find_repo_root(dir) -> Option<PathBuf>`，复用向上遍历同时拿到 repo root。

**不变量**：
- 目录型 pattern（`node_modules/`）必须传 `is_dir=true`，否则 trailing-slash pattern 不匹配
- 子目录自身的 `.gitignore` 不影响直接子项的可见性（`foo` 含被忽略文件，但 `foo` 本身仍可见）
- 非 git repo（无 `.git`）→ 空集合（保持现状）

**回归测试**：`list_dir_filters_gitignore_in_repo`——tempdir 建 `.git/` + `.gitignore`（`target`、`node_modules/`、`*.log`、`!keep.log`）+ 匹配/不匹配文件，`show_hidden=true` 隔离 dot 分支，断言 target/node_modules/*.log 隐藏、src/keep.log 可见。

### 1.2 waiter join 超时（#2）

**根因**：`crates/pty/src/session.rs:273 reader_thread.join()` 无界阻塞。shell 起的后台进程（`sleep 100 &`、daemon、powerlevel10k）持 PTY slave fd → shell 退出后 master read 永不 EOF → reader 永不退 → join 永久阻塞 → `on_exit(code)` 永不调 → 前端永远显示「运行中」，flusher 永远循环。

**修复**：`JoinHandle::join` 无 stable `try_join`，改用「spawn 计时守护线程 + channel 传 join 结果 + recv_timeout」：

```rust
let (jtx, jrx) = std::sync::mpsc::channel();
let rhandle = reader_thread;
let jthr = thread::Builder::new()
    .name("octopus-pty-reader-join".into())
    .spawn(move || {
        let res = rhandle.join();
        let _ = jtx.send(res);
    });
match jrx.recv_timeout(JOIN_TIMEOUT) {
    Ok(Ok(())) => { /* reader 正常退出，jthr 已自然结束 */ }
    Ok(Err(panic)) => log::error!("pty reader thread panicked: {panic:?}"),
    Err(_) => {
        log::warn!("pty reader 未在 {:?} 内退出（后台进程持 slave fd），强制收尾", JOIN_TIMEOUT);
        // jthr + reader 线程泄漏：阻塞在 read，session Drop（master fd 关）时 OS 回收，benign
    }
}
// 保留 :276-283 tail-drain 序列不变
```

**关键不变量**（深度核实确认）：
- **force-finalize 不丢数据**：所有字节要么已被 flusher 发出，要么在 `pending`；`session.rs:277 std::mem::take(&pending)` 原子取全部
- **顺序保证保留**：`on_data(tail)` 严格先于 `on_exit(code)`（都在 waiter 线程顺序执行）
- **泄漏 benign**：超时后 reader 仍阻塞在 `read()`，session Drop 时 master fd 关 → read 返 EOF → reader 退出；jthr 也随之退出。进程级泄漏，应用退出时 OS 回收
- **常量**：`const JOIN_TIMEOUT: Duration = Duration::from_secs(2);`

**回归测试**：
- `tail_drained_on_normal_exit`（核心层，mock reader 即时 EOF，断言 pending 取空 + on_exit 收到 code）
- `waiter_finalizes_on_reader_hang`（`#[ignore]`，文档化复现 `sleep 100 &` 后 exit）

### 1.3 waiter spawn 失败清理（#9）

**根因**：waiter 是最后 spawn 的线程（顺序：reader → flusher → waiter）；spawn 失败（RLIMIT_NPROC）返 Err → Drop 杀但不 reap → 僵尸 child；flusher 已在跑但 `done` 永不被置位 → flusher 永旋；`pending`/`on_data`/`on_signal` Arc 永不释放。

**修复**：waiter spawn 的 `.map_err` 闭包内做清理：
1. `child.kill()` + `child.wait()` reap 僵尸
2. `done.store(true, Release)` + `cv.notify_all()` 让 flusher 退
3. reader 由 master drop（session Drop 时）触发 EOF 退

抽成 helper `cleanup_on_waiter_spawn_fail(child, done, cv)` 保证 reap + 停 flusher。

**回归测试**：难造 RLIMIT_NPROC，文档化 + helper 单测（验证 done 被置位 + child 被 reap，用 mock Child）。

### 1.4 getpwuid_r（#12）

**根因**：`crates/pty/src/shell_init.rs:68 libc::getpwuid`（非可重入版）返回进程级静态缓冲区指针。两 PTY 并发 spawn（用户快速连开两终端）竞争该缓冲区，可能读到错误 shell 路径。

**修复**：改 `getpwuid_r`（可重入，调用方提供 `passwd` + `buf`）。`buf` 用 `[u8; 4096]`（POSIX 建议 `sysconf(_SC_GETPW_R_SIZE_MAX)`，4KB 通常足够；不够则 `ERANGE` 时扩容重试）。

```rust
fn login_shell() -> Option<String> {
    let uid = unsafe { libc::getuid() };
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; 4096];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    loop {
        let ret = unsafe {
            libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr() as *mut _, buf.len(), &mut result)
        };
        if ret == 0 {
            if result.is_null() { return None; }
            // 读 (*result).pw_shell → CStr → String
        } else if ret == libc::ERANGE {
            buf.resize(buf.len() * 2, 0);
        } else {
            return None;
        }
    }
}
```

**回归测试**：`login_shell_concurrent_safe`（10 线程并发调，断言无 panic 且结果一致）。

### 1.5 服务端 reaper（#13）

**根因**：`PtyState`（`crates/pty/src/lib.rs:17`）纯被动 state，无后台扫描。仅 `pty_close`（前端）+ `pty_open` 早 re-check 移除。前端崩溃/路由切换不处理 `on_exit` → sessions map 残留 `Arc<PtySession>` + 死 PTY fd 直到应用退出。

**修复**：在 desktop 的 `init_pty`（`crates/desktop/src/core/setup.rs:681`）`.manage()` 后 spawn 一个 `std::thread`（**不引入 tokio 到 pty crate**，保持其「纯逻辑无 tauri」设计约束）。reaper 每 5s 扫描 `state.sessions.read()`，收集 `is_exited()==true` 的 id，释放读锁后取写锁 `remove`。

```rust
// pty crate 加可测方法
impl PtyState {
    pub fn reap_exited(&self) -> Vec<u32> {
        let dead: Vec<u32> = self.sessions.read()
            .iter().filter(|(_, s)| s.is_exited()).map(|(id, _)| *id).collect();
        if !dead.is_empty() {
            let mut sessions = self.sessions.write();
            for id in &dead { sessions.remove(id); }
        }
        dead
    }
}

// desktop setup.rs init_pty
fn init_pty(&self) {
    let state = octopus_pty::PtyState::new();
    self.app.manage(state);
    let handle = self.app.handle().clone();
    std::thread::Builder::new().name("octopus-pty-reaper".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            if let Some(state) = handle.try_state::<octopus_pty::PtyState>() {
                let dead = state.reap_exited();
                if !dead.is_empty() {
                    log::debug!("pty reaper 回收 {} 个已退出 session: {:?}", dead.len(), dead);
                }
            }
        }).ok();
}
```

**不变量**：
- 只 reap `exited==true`（waiter 已跑完，reader/flusher 已退或已超时强制退）
- 写锁互斥，不与 `pty_close` 竞争
- daemon 线程，随 app 生命周期，进程退出 OS 回收

**回归测试**：`reap_exited_removes_dead_sessions` —— 构造 PtyState 塞入 mock session（`exited=true`），调 `reap_exited()`，断言被移除；`exited=false` 的保留。mock 需 `PtySession` 可构造（或加 `#[cfg(test)]` 测试 helper）。

### 1.6 agent 窗口 ready 回拉（#7）

**根因**：`crates/desktop/src/ui/terminal_window.rs:210-220` 新窗口分支固定 `sleep(250ms)` + 单次 emit + `let _ =` 丢结果，无 ack。慢 mount（冷启动 React+xterm、老机器、debug build）> 250ms 时前端 listener 未注册，事件被 Tauri 静默丢弃 → agent 窗口弹出但命令未写入，空终端无错误。

**修复**（前端 ready 回拉 + 后端超时兜底）：

**前端**（`crates/desktop/frontend/src/pages/Terminal/index.tsx:262`）：`listen("terminal://new-tab").then(fn => unlisten=fn)` 块内，listener 注册成功后立即 `emit("terminal://ready", { windowLabel: currentLabel })`。保证 listener 先注册再发 ready（消除竞态——ready 到达时 listener 必然已在）。

**后端**（`terminal_window.rs` 新窗口分支）：改 `sleep(250ms)+emit` 为 `listen_once("terminal://ready", move |_| { emit_to(new-tab) })` + 5s 超时兜底（超时则强制 emit 一次 + log::warn，避免 ready 永不到时命令彻底丢）。事件名常量化。

**回归测试**：后端逻辑抽成可测函数 `wait_for_ready_then_emit(app, payload, timeout) -> Result<(), Elapsed>`，单测覆盖「ready 到→emit」「超时→兜底 emit」两路径（用 mock listener）。前端无单测，手动验证。

## 2. 云识别（🔴🟠）

### 2.1 close_async channel-close-as-Failed（#3）🔴

**根因**：`crates/asr-cloud/src/cloud_types.rs:129-141` `while let Some(event) = rx.recv().await`，WS task 因服务端主动 Close（见 #4）静默退出而未发 Finished/Failed → `result_tx` drop → `rx.recv()` 返 None → 循环正常结束 → 返 `Ok(text)`。text 可能 partial 或空。鉴权过期/超时/限流断连时用户拿到不完整结果却以为成功，错误被吞（无 bail、无日志）。影响全部 4 个 provider。

**修复**：加 `let mut finished = false;`，`StreamEvent::Finished => { finished = true; break; }`，循环后：
```rust
if !finished {
    bail!("cloud session closed without terminal event ({} bytes partial)", text.len());
}
```
与现有 `Failed` 分支 `bail!` 一致。调用方 `lifecycle.rs:539 handle_cloud_streaming_done` 已能容错 Err（仅 `warn!`，partial transcript 仍可 paste）。

**回归测试**：`close_async_fails_on_channel_close_without_finished`——mock `result_tx` 只发 Text 不发 Finished 就 drop，断言 close_async 返 Err。

### 2.2 三家补 Close 帧（#4）🟠

**根因**：aliyun(`:251`)/bytedance(`:275`)/tencent(`:221`) Close 落 `_ => {}`，`ws.next()` 返 `Ok(None)` → break → `return Ok(())` 无终态事件——是 #3 的触发路径。baidu(`:214`) 是唯一正确参照。

**修复**：每家加 `Message::Close(_) => { ...; return Ok(()); }`，参照 baidu 模板，按各家「稳态判定」分支：

| Provider | 稳态判定变量 | Close 时行为 |
|---|---|---|
| aliyun Fun-ASR | `!committed.is_empty()` | 稳态→Text+Finished；否则 Failed |
| aliyun Qwen | `!accumulated_text.is_empty()` | 同上 |
| bytedance | 新增 `got_last_frame`（`flags==0x3` 置 true）+ `last_text` | 稳态→Text(last)+Finished；否则 Failed |
| tencent | `!stable_segments.is_empty()` | 稳态→Text+Finished；否则 Failed |

**回归测试**：每家 `close_frame_emits_failed_when_no_stable_result` + `close_frame_emits_finished_when_stable`（mock WS 流注入 Close 帧）。

### 2.3 baidu Close partial-as-Finished（#11）

**根因**：`baidu_stream.rs:216` 只查 `display` 非空，不查是否收到过 FIN_TEXT。仅 MID_TEXT partial（`current_partial` 非空、`fin_texts` 空）时 `accumulate_display` 返非空却发 Finished，把可能不准确的 partial 当最终结果。

**修复**：改判 `let stable = !fin_texts.is_empty();`：
- `stable` → Text(display)+Finished（display 一定非空，因 `fin_texts.concat()` 非空）
- `!stable && !display.is_empty()` → Failed("仅收到非稳态 partial")
- 空 → Failed("未收到识别结果")

**回归测试**：`baidu_close_with_only_partial_emits_failed`（fin_texts 空 + current_partial 非空 → Close → Failed）。

### 2.4 baidu 多句分隔符（#5）🟠

**根因**：`baidu_stream.rs:243 accumulate_display` `fin_texts.concat()` 无分隔，英文粘连（`"hello world"+"today"→"helloworldtoday"`）。`_language` 参数未用。aliyun 用 `sentence_separator(&language)`（英文空格、中文逗号）。

**修复**：`accumulate_display` 接收 language（`_language: &str` → `language: &str`），`fin_texts.join(sentence_separator(language))`。调用点 `:217` 传 `&language`。

**回归测试**：扩 `accumulate_display` 单测：
- 英文 `["hello world","today is good"]` + "en" → `"hello world today is good"`
- 中文 `["你好","世界"]` + "zh" → `"你好，世界"`

### 2.5 bytedance PCM 帧 COMP_NONE（#6）🟠（性能）

**根因**：`bytedance_stream.rs:183,204,216` 三处音频帧 `COMP_GZIP`；PCM s16le 高熵 gzip 无效（ratio 0.9-1.1），双向 gzip 热路径（每 ~33ms 一帧）CPU 浪费 + 增端到端延迟。JSON config（`:166-172`）用 gzip 合理（文本可压），保留。

**修复**：加 `const COMP_NONE: u8 = 0x0;`（协议头注释 `:32-33` 已声明合法），三处音频帧 `COMP_GZIP → COMP_NONE`。

**⚠️ 需实测**：bytedance 服务端对音频帧 NONE 的兼容性（协议声明合法，但实现可能假设 gzip）。改完先手动跑一次 bytedance 识别验证，不通过则回退。

**回归测试**：`build_client_frame_audio_uses_none_compression`（音频帧 compression==0x0）；`config_frame_uses_gzip`（JSON config 仍 gzip）。

## 3. asr-local 文本后处理（🟡🟢）

### 3.1 corrector find_candidates 优化（#8）🟡

**根因**：`corrector.rs:121-134` 每次 `.lookup().cloned()` 整个 `Vec<(String,i64)>`（含 String 堆分配）+ `sort_by` 全排序后大部分丢弃（下游 `take(5)`，`correct_greedy` 取首个非原候选）。N=30/max_sz=4 约 90 次/correct()。`bigram_score` 在每次比较时重算（HashMap lookup + `word.chars().collect()`）。

**修复**：
1. `.cloned()` 改 borrow（`lookup` 返 `Option<&Vec>`）
2. 用 `select_nth_unstable_by`（O(n) 平均）取 top-5 再 sort，避免全排序
3. `bigram_score` 结果缓存（预计算每个候选的 score 到 `Vec<(f64, &String, i64)>` 再 sort，避免比较时重算）

保留 tie-break `.then_with(|| a.0.cmp(&b.0))` 确定性。

**回归测试**：现有 `correct_greedy` 测试保证语义不变；加 `find_candidates_returns_top5_sorted` 验证排序+tie-break。

### 3.2 itn 黑名单单次扫描（#10）🟡

**根因**：`itn.rs:47 chars().collect()` 在 loop{} 内，每匹配一黑名单词重 collect 全文 O(N) + 线性扫描 O(N)，K 处匹配 → O(N·K)。

**修复**：单次遍历——对每个黑名单词用 `str::find` 迭代收集所有匹配 `(byte_start, byte_end)`，记录后统一从后向前 `replace_range` 替换（避免索引偏移）。或用 `aho_corasick`（若已在依赖）；否则朴素多词扫描。

**回归测试**：`normalize_blacklist_multiple_matches`——长文本含多个黑名单词 + 嵌套，断言全替换。

### 3.3 miner DB 错误传播（🟢）

**根因**：`miner.rs:37 .unwrap_or_default()` 把 DB 故障伪装「无编辑历史」，与兄弟函数 `:71` 用 `?` 不一致，日志误导诊断。

**修复**：改 `?` 传播。若调用方不能容错则 log::error + 返空（可观测）。需检查 `mine()` 调用方（desktop 层）对 Err 的处理。

**回归测试**：`mine_propagates_db_error`（mock DB 返 Err，断言 mine 返 Err）。

### 3.4 miner 排序 tie-break（🟢）

**根因**：`miner.rs:56 sort_by(|a,b| b.1.cmp(&a.1))` 无 tie-break，HashMap 迭代序不确定，同频候选跨次运行不同。

**修复**：加 `.then_with(|| a.0.cmp(&b.0))`（字典序），对齐 corrector:133。

**回归测试**：`mine_ranking_deterministic_on_ties`（同频多候选，多次运行结果一致）。

### 3.5 reload 并发窗口（🟢，可选）

**根因**：`corrector.rs:226-227` 两独立写锁非原子；`:247`+`:254` 规则先换索引后建 → 瞬时新查询/旧 key 漏命中。瞬时自愈。

**修复**（低优先）：reload_hotwords 用单个 `RwLock<HotwordConfig>` 包 active_words+hotwords 原子换；reload_fuzzy_dialect 先建新索引再换规则（调换 247/254 顺序）。或文档化为「已知可接受竞态」不修。

## 4. 次要（🟢）

### 4.1 fontPrefs jsdoc

`crates/desktop/frontend/src/pages/Settings/fontPrefs.ts:21 @returns` 反了。改 jsdoc 为「true 表示在默认状态，不显示按钮」。代码不动。

### 4.2 agent_detect 引号 + 连字符误报

**引号**（`agent_detect.rs:309`）：`split_whitespace` 不剥引号，`'claude'`/`"claude"` 匹配失败。修复：token 先 `trim_matches(|c| c=='\''||c=='"')`。

**连字符误报**（`:315-316`）：连字符后缀是设计特性（`claude-enigma`→`claude`），但 `claude-config` 误报。难两全，**保守只修引号**（明确 bug），连字符误报文档化为「已知设计行为」。

**回归测试**：`match_agent_strips_quotes`（`'claude'`/`"claude"` → claude）。

## 5. 验证纪律

每任务后强制：编译（0 error 0 warning）+ 测试（核心层 `cargo test --lib`）+ 影响面 grep + 端到端链路。

关键手动验证：
- #1：octopus repo 根目录文件树显示 src/Cargo.toml，隐藏 target/node_modules
- #2：`sleep 100 &` 后 exit shell，验证 tab 不假死
- #6：bytedance 实测识别正常（COMP_NONE 兼容性）

## 6. 不在范围

- pty crate 引入 tokio（违反「纯逻辑」设计约束）
- 终端文件树支持嵌套 `.gitignore` 递归（直接子项层面足够，子目录自身可见性不受其内容影响）
- agent_detect 连字符误报的两全方案（设计特性，文档化即可）
- reload 并发窗口的彻底修复（瞬时自愈，低优先）
