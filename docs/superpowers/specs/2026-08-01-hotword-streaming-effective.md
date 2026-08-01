# 热词在流式听写中生效

> **日期**：2026-08-01
> **状态**：✅ 已实现
> **背景**：`docs/pr/0801.md` 第 2 条——「热词好像没有生效，我听了比较多的热词，但好像没有一个命中的记录」

---

## 1. 问题复述

用户添加了热词，在实时听写中观察不到任何命中——既看不到文本被纠错，命中计数（设置页热词面板的命中数）也始终为 0。

## 2. 根因（3 个独立 gap）

热词实现是文本后处理层（`LightCorrector` + `HotwordIndex`）。索引会建好（`reload_hotwords` 在启动 + 每次写热词后正常调用），但在**实时流式听写路径**永不执行：

### Gap 1：`from_session(streaming_engine, false)` 硬编码 false

`crates/desktop/src/engine/coordinator/session.rs:232` + `lifecycle.rs:299` 两处调用 `LocalPipelineEngine::from_session(streaming_engine, false)`。`config.asr_correct` 在 `paste.rs:55` 算了但没传进 `from_session`。→ `StreamingRunner.maybe_correct` 的 `if !self.correct { return ev; }` 短路，`correct()` 永不调用。

### Gap 2：`StreamingRunner::finish()` 不过 corrector

`crates/asr-local/src/streaming/streaming_runner.rs:280-285`，finish 的 Final 文本只过 ITN。最终入库文本（经 `apply_engine_full`）无热词纠错。与 `docs/features/asr-engine.md` §注入点矛盾（finish 应是流式注入点）。

### Gap 3：流式路径无 `drain_hits()` / `bump_hotword_hit_by_word()`

即使修了 Gap 1+2，命中也不入库——`list_hotword_hits` UI 永远 0 命中。批量路径在 `postprocess_text`（`engines/pipeline.rs:85-89`）correct 后立即 drain+bump，流式路径无对应。

### 外加：`asr_correct` 默认 `false`

`config.rs:295` + `db.sql:395`。批量路径也受影响——用户加了热词，除非手动开 `asr_correct` 开关，否则批量也不纠。

## 3. 修复

### 3.1 激活 correct 开关（Gap 1）

`session.rs` + `lifecycle.rs` 两处 `from_session` 改为：
```rust
let correct = config.asr_correct && !config.language.eq_ignore_ascii_case("en");
LocalPipelineEngine::from_session(streaming_engine, correct)
```

`is_english` 在 coordinator 算（`StreamingRunner` 在 asr-local crate 不持 language）。`skip_corrector` 流式引擎 trait 无此方法（zipformer/paraformer 都不 skip），暂不考虑。

### 3.2 finish 注入 corrector（Gap 2）

`streaming_runner.rs::finish()` 加 correct 分支：
```rust
let corrected = if self.correct {
    crate::corrector::get_corrector().correct(&text)
} else { text };
TranscriptEvent::Final(crate::itn::normalize(&corrected))
```

`maybe_correct` 已处理 Partial/Committed（无需改）。`finish_with_tail` 末尾调 `self.finish()` 自动跟随。corrector 确定性幂等，Partial 阶段已纠过的内容 finish 再纠无副作用。

### 3.3 命中计数入库（Gap 3）

`lifecycle.rs` finish 后 `apply_engine_full` 之后加：
```rust
for word in octopus_asr_local::corrector::drain_hits() {
    if let Err(e) = octopus_infra::db::bump_hotword_hit_by_word(&word) {
        log::warn!(...);
    }
}
```

放 finish 后而非每 tick：corrector 的 `pending_hits` 是 `Mutex<Vec>` 跨 tick 累积，整场会话 drain 一次即可（与批量「一次转写 drain 一次」对称）。

### 3.4 `asr_correct` 默认改 `true` + 存量库迁移

- `config.rs::default_asr_correct()` → `true`
- `db.sql` seed `'false'` → `'true'`
- `config.rs::app_config_default_values` 测试 assert 改 `true`

**存量库迁移（schema v54→v55，2026-08-01 补）**：初版（c71ceac5）只改 seed，但 `INSERT OR IGNORE` 对存量库无效——存量用户的 `asr_correct` 仍是 `'false'`，加了热词却因开关关着永不生效（用户实测反馈「热词没用上」）。补 schema v54→v55 数据迁移：`UPDATE app_config SET config_value='true' WHERE config_key='asr_correct'`（`db/mod.rs::init_schema` v54 分支）。用户若故意关过，可在热词管理页面重新手动关。

### 3.5 UI 迁移：开关从「系统设置-语音」搬到「热词管理」

`asr_correct` 开关原在 `GeneralPanel`（系统设置-语音），用户难以发现（反馈「一直没注意到这个开关」）。2026-08-01 迁到 `HotwordPanel`（热词管理）——在加热词的地方控制纠错，语义更直观：

- `HotwordPanel.tsx`：加 `asrCorrect` prop，在方言模糊区上方加「热词纠错」总开关
- `Settings/index.tsx`：传 `asrCorrect={configResp.config.asr_correct}`
- `GeneralPanel.tsx`：删 asr_correct 行
- i18n 复用现有 `pinyinCorrect` / `pinyinCorrectHint` 文案

## 4. 测试隔离（corrector 全局单例）

`LightCorrector` 是进程级单例（`OnceLock`），`reload_hotwords` 改全局热词索引。跨模块测试（corrector / streaming_runner / engines / pipeline）共用同一单例，必须串行。

新增 `crates/asr-local/src/text/corrector.rs` 顶层（`#[cfg(test)] pub(crate)`）：
- `CORRECTOR_TEST_LOCK: OnceLock<Mutex<()>>` —— 跨模块共享锁
- `test_serial() -> MutexGuard` —— 持此 guard 的测试段串行

corrector tests 内的 `serial()` + streaming_runner tests 内的 `serial()` 都复用 `test_serial()`，确保跨模块互斥。

## 5. 不在本次范围

- **cloud 流式路径的热词纠错**：cloud 引擎（Aliyun/ByteDance/Tencent/Baidu WS）的 correct 由服务端还是客户端做需单独确认。本次只修 local 流式（用户报告场景）。cloud 路径的 `finalize_cloud` 未加 drain/bump。
- **流式引擎 `skip_corrector` trait 方法**：当前流式引擎（zipformer/paraformer）都不 skip。若未来 qwen3 流式需 skip，再给 `StreamingEngine` trait 加方法。

## 6. 代码位置速查（2026-08-01 状态）

| 位置 | 作用 |
|---|---|
| `crates/asr-local/src/streaming/streaming_runner.rs::finish` | Final 过 corrector（Gap 2 修复） |
| `crates/asr-local/src/streaming/streaming_runner.rs::maybe_correct` | Partial/Committed 过 corrector（已存在，Gap 1 激活后生效） |
| `crates/asr-local/src/text/corrector.rs::CORRECTOR_TEST_LOCK` + `test_serial` | 跨模块测试串行锁 |
| `crates/desktop/src/engine/coordinator/session.rs::from_session` | 传 `correct`（Gap 1） |
| `crates/desktop/src/engine/coordinator/lifecycle.rs::from_session` | 传 `correct`（Gap 1，WATCHDOG 路径） |
| `crates/desktop/src/engine/coordinator/lifecycle.rs` finish 后 | drain_hits + bump（Gap 3） |
| `crates/infra/src/config.rs::default_asr_correct` | 默认 true |
| `crates/infra/src/db.sql` seed | `asr_correct = 'true'` |
| `crates/asr-local/src/engines/pipeline.rs::postprocess_text` | 批量参考模型（correct + drain + bump） |
