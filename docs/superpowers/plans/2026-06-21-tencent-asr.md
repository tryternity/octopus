# 腾讯云 ASR 实时语音识别实施计划

> Spec：`docs/superpowers/specs/2026-06-21-tencent-asr-design.md`

## Task 1：infra 层 — AsrSection.tencent 字段 + db.sql seed

### 1.1 crates/infra/src/db.rs
- `AsrSection` 新增 `pub tencent: Option<HashMap<String, ModelEntry>>`
- `load_asr_config` 新增 `("tencent", _) => &mut asr.tencent` match arm
- struct initializer 补 `tencent: None`
- 测试：seed 行数 +2，新增 tencent section 断言

### 1.2 crates/infra/src/db.sql
```sql
('asr','tencent','Tencent-ASR','16k_zh','{appid}:{secretid}','zh','腾讯云实时语音识别（16k 中文通用，source 填 appid:secretid，key 填 SecretKey）',0,0,1),
('asr','tencent','Tencent-ASR-Multi','16k_zh_en','{appid}:{secretid}','zh','腾讯云实时语音识别大模型（16k 普方英+31 方言，source 填 appid:secretid，key 填 SecretKey）',0,0,0);
```

### 验证
```bash
cargo test -p octopus-infra
```

---

## Task 2：asr config 层 — EngineCategory::Tencent

### crates/asr/src/config.rs
1. `EngineCategory` 新增 `Tencent`
2. `resolve_category`：`provider.eq_ignore_ascii_case("tencent") → Some(Tencent)`
3. `all_sections`：维度 8→9，追加 `(cfg.asr.tencent.as_ref(), EngineCategory::Tencent)`
4. `provider_of`：`Tencent => "tencent"`
5. `category_label`：`Tencent => "Tencent-ASR"`
6. `pick_entry`：`Tencent => cfg.asr.tencent.as_ref()`
7. 测试 struct literal 补 `tencent: None`

### crates/asr/src/engine.rs
`Tencent` match arm → `bail!("腾讯云 ASR 引擎仅支持流式模式...")`

### crates/cli/src/main.rs
- label：`Tencent => "Tencent(云)"`
- dispatch：`Tencent` arm → bail

### 验证
```bash
cargo test -p octopus-asr-local --release
```

---

## Task 3：desktop 层 — TencentStreamSession

### crates/desktop/src/tencent_stream.rs（新增）
- `TencentStreamSession` struct + impl（open/push_pcm/finish/try_recv_text/close_async）
- `build_signed_url(appid, secretid, secretkey, engine_model_type)` — 构造签名 URL
- `run_tencent_session()` — WS 双向循环
- 文本累积：`BTreeMap<i64, String>` 存 slice_type=2 稳态句
- 单元测试：签名 URL 构造、文本累积逻辑

### crates/desktop/src/cloud_session.rs
新增 `Tencent(TencentStreamSession)` 变体

### crates/desktop/src/main.rs
新增 `#[cfg(feature = "aliyun")] mod tencent_stream;`

### crates/desktop/Cargo.toml
`aliyun` feature 追加 `hmac`、`sha1`

### 验证
```bash
cargo build -p octopus-desktop --features embedded,aliyun
cargo test -p octopus-desktop --features embedded,aliyun
```

---

## Task 4：coordinator dispatch

### crates/desktop/src/coordinator.rs
- `is_cloud_engine`：追加 `Some(EngineCategory::Tencent)`
- `resolve_tencent_config(engine_spec)` → `(appid_secretid, secretkey)`
- onset 分派新增 `Some(Tencent)` arm
- `CloudSession::Tencent` 构造

### 验证
```bash
cargo build -p octopus-desktop --features embedded,aliyun
```

---

## Task 5：Build + test + 文档

```bash
cargo build -p octopus-infra -p octopus-asr-local -p octopus-cli
cargo build -p octopus-desktop --features embedded,aliyun
cargo test -p octopus-infra && cargo test -p octopus-asr-local && cargo test -p octopus-desktop --features embedded,aliyun
```

文档：architecture.md（Tencent 章节）、configuration.md（接入指南）

---

## 实施记录（2026-06-21 完成）

### 验证结果

| 验证项 | 结果 |
|---|---|
| `cargo build -p octopus-infra -p octopus-asr-local` | ✅ PASS |
| `cargo build -p octopus-cli` | ✅ PASS |
| `cargo build -p octopus-server` | ✅ PASS |
| `cargo build -p octopus-desktop --features embedded,aliyun` | ✅ PASS（0 warnings） |
| `cargo test -p octopus-infra` | ✅ 29 passed |
| `cargo test -p octopus-asr-local` | ✅ 54 passed (6 ignored) |
| `cargo test -p octopus-desktop` | ✅ 58 passed (1 ignored，含 5 个 Tencent 测试) |

### 新增依赖
- `hmac = "0.12"`（HMAC-SHA1 签名）
- `sha1 = "0.10"`（SHA1 摘要）

### 关键设计决策
1. **`source` 复合字段**：`{appid}:{secretid}` 冒号分隔。DB `source` 列是自由文本，冒号不与 model spec 的 3-part 冲突。
2. **`model_name` = `engine_model_type`**：直接作为 URL 参数，无需中间映射。
3. **文本累积策略**：`BTreeMap<i64, String>` 按 `index` 存 `slice_type=2` 稳态句，partial（0/1）覆盖当前句。显示文本 = `stable.join("") + current_partial`。
4. **`percent_encode` 自实现**：腾讯文档强调"必须编码 `+`、`=` 等特殊字符"，比 standard percent-encode 更保守（全部非字母数字都编码）。

### 未完成 / 待验证
- **e2e 实测**：无 API Key，协议严格按腾讯文档实现，5 个单元测试覆盖签名 URL 构造 / percent-encode。Key 到位后需 e2e 验证。
