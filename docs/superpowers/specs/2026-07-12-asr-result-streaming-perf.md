# ASR Result 窗流式卡顿诊断 + writeDoc 增量修复

> Result 窗（语音识别浮窗）录音出字时偶发卡死/卡顿，等一阵恢复，不稳定复现。
> 定位到前端 CM6 `writeDoc` 整篇全量替换 O(n) 是主嫌；改增量追加 O(delta)，并加双端性能打点写 `~/.octopus/logs/asr.log` 待日志确认根因。

## 症状

- 引擎：**本地**（用户当前环境）。
- 场景：录音识别、流式出字过程中，**偶发** UI 卡死卡顿，等好一阵子才恢复。
- 无稳定复现步骤、无错误日志（项目未配 tracing 文件输出），事后取证困难。

## 诊断（系统化调试 Phase 1 — 根因调查）

### 已排除（后端阻塞）

| 嫌疑 | 结论 | 证据 |
|------|------|------|
| DB 写阻塞 tick 循环 | ❌ 不成立 | `update_transcription_raw`（coordinator.rs）只 `sender.send(DbCommand)` 进独立 DB 线程 + `DB_FLUSH_INTERVAL_MS` 落库节流 |
| 前端命令阻塞主线程 | ❌ 不成立 | `set_caret` / `set_selection` / `enter_edit_mode` / `commit_edit` 全是 `channel.send`，命令在 spawn 线程处理（coordinator.rs `enter_edit_mode` / `set_caret` / `set_selection`） |
| tick 线程阻塞 | ❌ 不成立 | 100–200ms 周期发 `StreamingTick` 也是 channel.send |

### emit 频率（pipeline.rs `StreamingPipeline::tick`）

- **本地引擎**：仅 `changed`（文本实际变化）时 emit `Emit`——按需，频率低。
- **云端引擎**：每 tick 都 emit（显示 base + partial 预览），`CLOUD_STREAMING_TICK_INTERVAL_MS = 100ms` ⇒ **10 次/秒**。
- 本次问题用户用本地，emit 频率不高；但本地 VAD 切句是「按句突发」，单次文本增长大。

### 定位（前端渲染 — 主嫌）

`AsrEditor.writeDoc`（`AsrEditor.tsx`）收到 `update-result`（携带**完整文本**）后做：

```ts
view.dispatch({ changes: { from: 0, to: doc.length, insert: newText }, scrollIntoView: true })
```

——**整篇 CM6 doc 全量替换**。流式文本本质是「前缀追加」（新 text = 旧 text + 新 delta），全量替换让 CM6 丢弃整个 doc 重建（重解析全文 + 重建所有行 DOM + 重算选区 + scrollIntoView），代价 **O(n)**。

叠加效应：长录音累积到上千字后，每次出字（本地按句突发）的单次 dispatch 就可能超帧预算 → UI 卡死；等这一句 emit 完、文本停止增长，才追上恢复。匹配「卡一阵才好」。

## 修复：writeDoc 增量追加

前缀扩展（`newText.startsWith(cur)` 且变长）走尾部插入 O(delta)；中插 / 润色重写 / 分支回退仍走全量：

```ts
const isAppend = newText.length > curLen && newText.startsWith(cur);
const changes = isAppend
  ? { from: curLen, insert: newText.slice(curLen) }   // O(delta)
  : { from: 0, to: curLen, insert: newText };          // 中插/重写仍全量
```

中插（insertion）态本来就不是末尾追加，仍全量——只优化最高频的正常流式追加。

## 诊断工具：perf_log（临时，定位后移除）

| 项 | 内容 |
|----|------|
| 模块 | `crates/desktop/src/perf_log.rs`（新增） |
| 输出 | `~/.octopus/logs/asr.log`，append，chrono 本地时区毫秒时间戳，Mutex 串行化，IO 错误静默吞 |
| 前端入口 | `#[tauri::command] perf_log_cmd(msg)`，注册到 `main.rs` generate_handler |
| 前端打点 | `writeDoc` dispatch 前后 `performance.now()`；阈值 `dt>8ms 或 total>800` 才记，含 `total/delta/mode` |
| 后端打点 | `pipeline.tick` 推理耗时（`engine.tick` 包围）+ tick 总耗时；阈值 `total>30ms` 才记，含 `infer/samples/changed/is_cloud` |

日志样例：
```
2026-07-12 16:38:27.123 [FE writeDoc] 14.2ms total=1820 delta=6 mode=append
2026-07-12 16:38:27.456 [BE tick] total=52ms infer=48ms samples=3200 changed=true is_cloud=false
```

## 实施摘要

- ✅ `perf_log.rs` 新建 + `chrono` 依赖 + `main.rs` 注册
- ✅ `AsrEditor.tsx` writeDoc 增量追加 + 前端打点
- ✅ `pipeline.rs` tick 推理耗时打点
- ✅ `cargo check`（0 错 0 警告）+ 前端 `tsc -b`（0 错）

## 状态：待日志确认（未完结）

`writeDoc` 增量替换是**高置信度假设的修复**，但**未拿到复现数据，根因未最终坐实**。

用户改用「自然使用中观察，不刻意复现」策略。卡顿时翻 `~/.octopus/logs/asr.log`：

- `[FE writeDoc]` ms 飙高（>16） → 前端渲染，增量修复应明显改善；若仍高说明还有别的渲染压力。
- `[BE tick]` infer 飙高 → 后端 ASR 推理慢（本地 ONNX 偶发），修法另议（推理线程/调度）。
- 卡顿时刻两边都低/无记录 → 根因不在测点内（emit 跨线程传输 / React 重渲染 / WKWebView），补打点再查。

**确认根因后**：移除 `perf_log` 模块 + `perf_log_cmd` 注册 + 两端打点（`writeDoc` 增量替换作为正式修复保留）。
