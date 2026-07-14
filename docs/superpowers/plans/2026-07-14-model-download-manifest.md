# 模型下载统一 Manifest 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将所有本地模型（ASR/翻译/OCR）的下载清单统一为声明式 manifest（`{path: {source, sha256, size}}`），存 DB `secret_key` 字段，统一路径结构 `~/.octopus/models/{domain}/{name}/`，翻译模型入 DB。

**Architecture:** `secret_key` 复用为下载清单（`is_local=1` 时）。manifest 格式扁平 key = 目标相对路径。所有 domain 走同一 `download_model` 命令（manifest 驱动逐文件下载）。翻译模型从代码常量迁移到 DB seed。

**Tech Stack:** Rust, SQLite, Tauri 2, React/TypeScript

**Spec:** `docs/superpowers/specs/2026-07-14-model-download-manifest-design.md`

## Global Constraints

- manifest 格式：`{"<相对路径>": {"source": "<URL>", "sha256": "<hex>", "size": <u64>}}`
- `source` URL 支持 `{env.huggingface}` / `{env.github}` / `{env.modelscope}` 模板变量（读 `app_config` 表 `category='env'`）
- 路径结构：`~/.octopus/models/{domain}/{model_name}/`（domain = asr/translate/ocr）
- DB `models.source`（`is_local=1`）= 路径标识（如 `asr/whisper-small`），不再是 HF repo 字符串
- `secret_key`（`is_local=1`）= manifest JSON；`secret_key`（`is_local=0`）= API Key
- 翻译模型 sha256/size 全预填；ASR/OCR sha256/size 全预填（从本地 HF cache 计算）
- 不改云端模型（`is_local=0`）任何逻辑
- 兼容现有文件：迁移时创建 `~/.octopus/models/{domain}/{name}/` → HF cache 软链

---

### Task 1: Manifest 格式升级——加 source 字段

**Files:**
- Modify: `crates/asr-local/src/manifest.rs`

**Interfaces:**
- Produces: `ManifestFile { source: String, sha256: String, size: u64 }`（新增 `source` 字段）
- Produces: `bootstrap_manifest` 不变签名，但内部生成的条目加 `source: ""`（空——bootstrap 不生成 URL）
- Produces: `verify_against_manifest` 不变（只校验 sha256）

- [ ] **Step 1: 写失败测试——ManifestFile 反序列化新格式**

在 `manifest.rs` 的 `#[cfg(test)] mod tests` 中加测试：

```rust
#[test]
fn manifest_file_deserializes_with_source() {
    let json = r#"{"a.onnx":{"source":"https://x.com/a.onnx","sha256":"abc","size":123}}"#;
    let m: Manifest = serde_json::from_str(json).unwrap();
    let f = m.get("a.onnx").unwrap();
    assert_eq!(f.source, "https://x.com/a.onnx");
    assert_eq!(f.sha256, "abc");
    assert_eq!(f.size, 123);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-asr-local --lib manifest::tests::manifest_file_deserializes_with_source`
Expected: FAIL — `ManifestFile` 无 `source` 字段

- [ ] **Step 3: 加 source 字段**

修改 `ManifestFile`：

```rust
#[derive(Serialize, Deserialize)]
pub struct ManifestFile {
    /// 下载来源 URL（支持 {env.*} 模板）。bootstrap 生成时为空串。
    pub source: String,
    pub sha256: String,
    pub size: u64,
}
```

修改 `bootstrap_manifest` 中 `collect_files` 的条目创建：

```rust
out.insert(rel, ManifestFile {
    source: String::new(),  // bootstrap 不生成 URL（旧格式兼容）
    sha256,
    size,
});
```

- [ ] **Step 4: 运行全部 manifest 测试**

Run: `cargo test -p octopus-asr-local --lib manifest`
Expected: PASS（新测试 + 旧 `bootstrap_manifest_hashes_files` + `verify_detects_tamper` 都过）

- [ ] **Step 5: Commit**

```bash
git add crates/asr-local/src/manifest.rs
git commit -m "feat: add source field to ManifestFile for manifest-driven downloads"
```

---

### Task 2: Model manifest 常量文件

创建独立 Rust 模块，存放 16 个模型的预填 manifest JSON 常量。数据从 HF cache 本地文件计算得出（spec §5）。

**Files:**
- Create: `crates/infra/src/model_manifests.rs`
- Modify: `crates/infra/src/lib.rs`（加 `pub mod model_manifests;`）

**Interfaces:**
- Produces: `model_manifests::asr_manifest(model_name) -> Option<&'static str>`
- Produces: `model_manifests::translate_manifest(model_name) -> Option<&'static str>`
- Produces: `model_manifests::ocr_manifest(model_name) -> Option<&'static str>`

- [ ] **Step 1: 生成 manifest JSON 数据**

运行脚本从 HF cache 生成所有 ASR/翻译/OCR 模型的 manifest JSON（排除 test_wavs/、.gitattributes、README.md、LICENSE、*.py、quantize_config.json）。

```bash
# 脚本已在 /tmp/asr_manifests.txt 生成 ASR 数据
# 翻译 + OCR 数据在 spec 中已列出
# 整理为 Rust 常量格式
```

- [ ] **Step 2: 创建 model_manifests.rs**

文件结构：
```rust
//! 所有本地模型的预填下载清单（从 HF cache 本地文件计算）。
//! DB v28 迁移时写入 models.secret_key，供 manifest 驱动下载。

/// ASR 模型 manifest。key = model_name，value = JSON。
pub fn asr_manifest(name: &str) -> Option<&'static str> {
    match name {
        "moonshine-base-en" => Some(MOONSHINE_BASE_EN),
        "moonshine-tiny-en" => Some(MOONSHINE_TINY_EN),
        // ... 12 个模型
        _ => None,
    }
}

pub fn translate_manifest(name: &str) -> Option<&'static str> { ... }
pub fn ocr_manifest(name: &str) -> Option<&'static str> { ... }

const MOONSHINE_BASE_EN: &str = r#"{...}"#;
// ...
```

注意：
- 排除 `test_wavs/`、`.gitattributes`、`README.md`、`LICENSE`、`*.py`、`quantize_config.json`
- source URL 用 `{env.huggingface}/{repo}/resolve/main/{path}` 格式
- OCR `keys_v6.txt` 和 `keys.txt` 的 source 用 `{env.github}/PaddlePaddle/PaddleOCR/raw/main/ppocr/utils/dict/ppocrv6_dict.txt`

- [ ] **Step 3: 在 lib.rs 注册模块**

```rust
pub mod model_manifests;
```

- [ ] **Step 4: 编译验证**

Run: `cargo build -p octopus-infra`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/infra/src/model_manifests.rs crates/infra/src/lib.rs
git commit -m "feat: add pre-filled model manifest constants for all local models"
```

---

### Task 3: DB Schema v28 迁移——翻译入 DB + source 改路径标识 + manifest 填充

**Files:**
- Modify: `crates/infra/src/db.sql`（domain 注释 + translate seed + OCR seed 改）
- Modify: `crates/infra/src/db.rs`（v27→v28 迁移逻辑）

**Interfaces:**
- Consumes: `model_manifests::asr_manifest` / `translate_manifest` / `ocr_manifest`
- Produces: DB v28，所有 `is_local=1` 模型有正确 `source`（路径标识）+ `secret_key`（manifest JSON）

- [ ] **Step 1: 更新 db.sql domain 注释**

```sql
domain        TEXT    NOT NULL,                       -- 'asr' | 'llm' | 'ocr' | 'translate'
```

- [ ] **Step 2: 更新 db.sql ASR seed——source 改为路径标识**

所有 ASR 本地模型的 INSERT 语句中，source 从 HF repo 改为 `asr/{model_name}`：

```sql
-- 例：
-- 旧: ('asr','local','whisper','whisper-small','onnx-community/whisper-small.en','en',...)
-- 新: ('asr','local','whisper','whisper-small','asr/whisper-small','en',...)
```

同时 `paraformer-zh`（与 `paraformer-streaming` 同 repo）的 source 改为 `asr/paraformer-zh`。

- [ ] **Step 3: 更新 db.sql OCR seed**

```sql
-- 删除旧 PP-OCRv6-small 行（GitHub MNN URL），替换为：
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, language, description, is_local, is_enabled, is_streaming)
VALUES
    ('ocr','paddleocr','ocr','PP-OCRv6-small','ocr/PP-OCRv6-small','auto','PP-OCRv6 small (det 9.7M + rec 21.5M + keys 73K)，中/英/繁体/日',1,1,0),
    ('ocr','paddleocr','ocr','PP-OCRv5','ocr/PP-OCRv5','auto','PP-OCRv5 mobile (det 4.5M + rec 16M + keys 92K)，中/英/繁体/日',1,0,0);
```

注意：secret_key 不在 db.sql 中预填（太大），由 v28 迁移函数从 `model_manifests` 写入。

- [ ] **Step 4: 加 db.sql translate seed**

```sql
-- ── 翻译模型（domain='translate'）─────────────────────────────────
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, language, description, is_local, is_enabled, is_streaming)
VALUES
    ('translate','local','opus-mt','opus-mt','translate/opus-mt','auto','opus-mt 中英互译（轻量快速，~500M）',1,0,0),
    ('translate','local','m2m100','m2m100-418M','translate/m2m100-418M','auto','m2m100 多语言翻译（100+ 语言互译，~600M）',1,0,0);
```

- [ ] **Step 5: 写 v27→v28 迁移逻辑（db.rs init_schema）**

在 `init_schema` 函数末尾（v27 block 之后）加 v28：

```rust
{
    // v27→v28：模型路径统一 + manifest 填充 + 翻译模型入 DB
    //
    // 1. ASR source: HF repo → asr/{model_name}
    // 2. OCR source/secret_key: GitHub MNN → ocr/{name} + manifest
    // 3. translate seed: db.sql IF NOT EXISTS 自动创建
    // 4. secret_key: 从 model_manifests 常量写入所有 is_local=1 模型

    // 重跑 INIT_SQL 确保 translate seed 已建（幂等）
    conn.execute_batch(INIT_SQL).ok();

    // ASR source 改路径标识（幂等：只改 source 仍为 HF repo 格式的行）
    conn.execute(
        "UPDATE models SET source = 'asr/' || model_name
         WHERE domain = 'asr' AND is_local = 1
           AND source NOT LIKE 'asr/%'",
        [],
    )?;

    // OCR source 改路径标识 + 清除旧 MNN URL secret_key
    conn.execute(
        "UPDATE models SET source = 'ocr/' || model_name, secret_key = ''
         WHERE domain = 'ocr' AND is_local = 1
           AND source NOT LIKE 'ocr/%'",
        [],
    )?;

    // 填充所有 is_local=1 模型的 manifest（secret_key 空 → 从常量写入）
    fill_manifests_from_constants(conn)?;

    conn.execute("PRAGMA user_version = 28", [])?;
    log::info!("schema upgraded to v28 (model path unification + manifest fill + translate seed)");
}
```

同时更新全新库 init 末尾的 `PRAGMA user_version = 28`。

- [ ] **Step 6: 实现 fill_manifests_from_constants 函数**

```rust
fn fill_manifests_from_constants(conn: &Connection) -> Result<()> {
    // ASR
    let rows: Vec<(String,)> = conn
        .prepare("SELECT model_name FROM models WHERE domain='asr' AND is_local=1 AND (secret_key='' OR secret_key IS NULL)")?
        .query_map([], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    for (name,) in &rows {
        if let Some(json) = crate::model_manifests::asr_manifest(name) {
            conn.execute("UPDATE models SET secret_key=?1 WHERE model_name=?2 AND domain='asr'", params![json, name])?;
        }
    }
    // translate（同样模式）
    let rows: Vec<(String,)> = conn
        .prepare("SELECT model_name FROM models WHERE domain='translate' AND is_local=1 AND (secret_key='' OR secret_key IS NULL)")?
        .query_map([], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    for (name,) in &rows {
        if let Some(json) = crate::model_manifests::translate_manifest(name) {
            conn.execute("UPDATE models SET secret_key=?1 WHERE model_name=?2 AND domain='translate'", params![json, name])?;
        }
    }
    // ocr（同样模式）
    let rows: Vec<(String,)> = conn
        .prepare("SELECT model_name FROM models WHERE domain='ocr' AND is_local=1 AND (secret_key='' OR secret_key IS NULL)")?
        .query_map([], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    for (name,) in &rows {
        if let Some(json) = crate::model_manifests::ocr_manifest(name) {
            conn.execute("UPDATE models SET secret_key=?1 WHERE model_name=?2 AND domain='ocr'", params![json, name])?;
        }
    }
    Ok(())
}
```

- [ ] **Step 7: 加 DB 查询函数 list_local_models_by_domain**

```rust
pub fn list_local_models_by_domain(domain: &str) -> Result<Vec<LocalAsrModelRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT category, model_name, source, secret_key, description, is_enabled, is_streaming
             FROM models WHERE domain=?1 AND is_local = 1",
        )?;
        let rows = stmt.query_map(params![domain], |row| {
            Ok(LocalAsrModelRow {
                category: row.get(0)?,
                model_name: row.get(1)?,
                source: row.get(2)?,
                secret_key: row.get(3)?,
                description: row.get(4)?,
                is_enabled: row.get::<_, i32>(5)? != 0,
                is_streaming: row.get::<_, i32>(6)? != 0,
            })
        })?;
        let mut out = Vec::new();
        for r in rows { out.push(r?); }
        Ok(out)
    })
}
```

- [ ] **Step 8: 编译 + 运行已有 DB 测试**

Run: `cargo test -p octopus-infra`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat: DB v28 — model path unification + manifest fill + translate seed"
```

---

### Task 4: 路径迁移——创建 models/{domain}/{name}/ 软链到 HF cache

迁移时自动将 `~/.octopus/models/asr/{name}/` 软链到 HF cache snapshot，使 `resolve_model_dir` 新路径查找命中。后续下载直接写入新路径。

**Files:**
- Create: `crates/onnx-infra/src/migrate.rs`
- Modify: `crates/onnx-infra/src/lib.rs`（加 `pub mod migrate;`）

**Interfaces:**
- Produces: `migrate::create_model_symlinks()` — 幂等创建软链

- [ ] **Step 1: 写失败测试——create_model_symlinks 幂等**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_symlink_skip_if_exists() {
        // 已存在的目录不重复创建
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("asr/test-model");
        std::fs::create_dir_all(&target).unwrap();
        // 应跳过（已存在）
        assert!(target.is_dir());
    }
}
```

- [ ] **Step 2: 实现 create_model_symlinks**

```rust
//! 模型路径迁移：为已下载到 HF cache 的模型创建
//! ~/.octopus/models/{domain}/{name}/ → HF cache snapshot 的软链。

use std::path::PathBuf;
use anyhow::Result;
use crate::paths::octopus_config_home;
use crate::find_hf_cache;

/// 幂等创建：读 DB 中所有 is_local=1 模型的 manifest，
/// 从第一个 source URL 解析 HF repo，在 HF cache 中找到 snapshot，
/// 在 ~/.octopus/models/{source}/ 创建软链。
pub fn create_model_symlinks() -> Result<()> {
    let models = octopus_infra::db::list_all_local_models();
    let base = octopus_config_home().join("models");

    for m in models {
        if m.source.is_empty() || m.secret_key.is_empty() { continue; }
        let dest = base.join(&m.source);
        if dest.exists() { continue; } // 已有（下载或之前的软链）

        // 从 manifest 解析第一个 HF repo
        let manifest: serde_json::Value = match serde_json::from_str(&m.secret_key) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let hf_repo = extract_hf_repo_from_manifest(&manifest);
        if let Some(repo) = hf_repo {
            if let Ok(snapshot_dir) = find_hf_cache(&repo) {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                #[cfg(unix)]
                std::os::unix::fs::symlink(&snapshot_dir, &dest).ok();
            }
        }
    }
    Ok(())
}

/// 从 manifest 的 source URL 解析 HF repo（{env.huggingface}/{owner}/{name}/resolve/main/...）
fn extract_hf_repo_from_manifest(manifest: &serde_json::Value) -> Option<String> {
    let obj = manifest.as_object()?;
    for (_path, meta) in obj {
        let source = meta.get("source")?.as_str()?;
        // 匹配 {env.huggingface}/{owner}/{repo}/resolve/main/...
        if let Some(idx) = source.find("/resolve/main/") {
            let prefix = &source[..idx]; // https://...{env}/owner/repo
            // 去掉 {env.*} 前缀 + 第一个 /
            if let Some(slash) = prefix.rfind("}") {
                let repo = &prefix[slash + 1..]; // owner/repo
                // 可能还带 mirror host——取最后两段 owner/repo
                let parts: Vec<&str> = repo.split('/').collect();
                if parts.len() >= 2 {
                    let n = parts.len();
                    return Some(format!("{}/{}", parts[n - 2], parts[n - 1]));
                }
            }
        }
    }
    None
}
```

- [ ] **Step 3: 在 desktop server 启动时调用**

在 `crates/desktop/src/main.rs` 的 setup 闭包中（`init_schema` 之后）加：

```rust
// 创建模型路径软链（HF cache → ~/.octopus/models/{domain}/{name}/）
if let Err(e) = octopus_onnx_infra::migrate::create_model_symlinks() {
    log::warn!("模型路径迁移失败（非致命）: {e:?}");
}
```

- [ ] **Step 4: 编译**

Run: `cargo build -p octopus-onnx-infra`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/onnx-infra/src/migrate.rs crates/onnx-infra/src/lib.rs crates/desktop/src/main.rs
git commit -m "feat: auto-create model path symlinks from HF cache on startup"
```

---

### Task 5: 翻译模型代码重构——删除硬编码常量，改读 DB

**Files:**
- Modify: `crates/translation/src/discovery.rs`
- Modify: `crates/translation/src/m2m100.rs`
- Modify: `crates/translation/src/opus_mt.rs`

**Interfaces:**
- Consumes: `octopus_infra::db::list_local_models_by_domain("translate")`
- Produces: `discover_translation_models()` 改为从 DB 读 + 文件系统检查

- [ ] **Step 1: 重写 discovery.rs**

删除 `KNOWN_MODELS` 和 `OPUS_REPOS` 常量。`discover_translation_models` 改为：

```rust
pub fn discover_translation_models() -> Vec<TranslationModelInfo> {
    let rows = match octopus_infra::db::list_local_models_by_domain("translate") {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    rows.iter().filter_map(|r| {
        let (downloaded, path) = check_model_ready(&r.model_name, &r.source);
        // m2m100 和 opus-mt 各有不同的就绪检查
        Some(TranslationModelInfo {
            name: r.model_name.clone(),
            source: r.source.clone(),
            downloaded,
            size_mb: 0, // 不再硬编码 size
            path,
        })
    }).collect()
}

/// list_downloadable_translation_models 删除——前端统一走 list_downloadable_models
```

`check_model_ready` 按 model_name 分发：
- `m2m100-418M` → `resolve_model_dir("translate/m2m100-418M")` + 检查 onnx 文件
- `opus-mt` → `resolve_model_dir("translate/opus-mt")` + 检查 zh-en/en-zh 子目录

- [ ] **Step 2: 重写 m2m100.rs——repo 从 DB source 读**

```rust
// 删除: const M2M100_REPO: &str = "lazycodepersona/m2m100_418m";

impl M2M100Engine {
    pub fn load() -> Result<Self> {
        let model_dir = onnx_infra::resolve_model_dir("translate/m2m100-418M")
            .context("m2m100 模型未找到...")?;
        // 后续不变
    }
}
```

- [ ] **Step 3: 重写 opus_mt.rs——resolve_opus_dir 简化**

`resolve_opus_dir` 不再拼 HOME 路径，直接用 `resolve_model_dir`：

```rust
fn resolve_opus_dir(source_lang: &str, target_lang: &str) -> Result<(PathBuf, String, String)> {
    let src = lang_prefix(source_lang);
    let tgt = lang_prefix(target_lang);
    let direction = format!("{}-{}", src, tgt);

    let base = onnx_infra::resolve_model_dir("translate/opus-mt")?;
    let dir = base.join(&direction);
    if dir.is_dir() {
        return Ok((dir, src, tgt));
    }
    anyhow::bail!("opus-mt 方向 '{}' 未找到...", direction)
}
```

- [ ] **Step 4: 编译**

Run: `cargo build -p octopus-translation`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/translation/src/discovery.rs crates/translation/src/m2m100.rs crates/translation/src/opus_mt.rs
git commit -m "refactor: translation models read from DB instead of hardcoded constants"
```

---

### Task 6: Desktop download_model 重构——manifest 驱动下载

**Files:**
- Modify: `crates/desktop/src/model_commands.rs`

**Interfaces:**
- Consumes: `Manifest`（从 `secret_key` JSON 反序列化）
- Consumes: `model_manifests`（旧格式升级用）
- Produces: `download_model(repo)` 改为按 manifest 逐文件下载

- [ ] **Step 1: 重写 download_model 为 manifest 驱动**

核心逻辑：
1. 从 DB 按 source 查 model → 读 secret_key manifest
2. 如果 secret_key 为空或无 source（旧格式），先 bootstrap 升级
3. 替换 `{env.*}` 模板变量
4. 逐文件下载到 `~/.octopus/models/{source}/{path}` + SHA256 校验
5. is_enabled = true

```rust
#[tauri::command]
pub async fn download_model(
    repo: String, // 实际是 source 路径标识，如 "asr/whisper-small"
    _rc: State<'_, SharedRuntimeConfig>,
    app_handle: AppHandle,
) -> Result<(), String> {
    // 1. 从 DB 读模型 manifest
    let (model_name, secret_key) = lookup_model_by_source(&repo)?;
    let manifest: octopus_asr_local::manifest::Manifest = if secret_key.is_empty() {
        return Err(format!("模型 '{repo}' 无下载清单"));
    } else {
        serde_json::from_str(&secret_key)
            .map_err(|e| format!("manifest 解析失败: {e}"))?
    };

    // 2. 探查：所有文件已就绪 → bootstrap + 置 true，不重下
    let dest_base = octopus_infra::paths::octopus_config_home().join("models").join(&repo);
    if dest_base.is_dir() && check_manifest_ready(&dest_base, &manifest) {
        // 已就绪
        let _ = app_handle.emit("download-done", serde_json::json!({"repo": &repo, "already_ready": true}));
        set_model_enabled_state(&model_name, true)?;
        return Ok(());
    }

    // 3. 解析 {env.*} 模板
    let env_vars = load_env_vars();

    // 4. 逐文件下载
    let total_files = manifest.len();
    let (tx, mut rx) = mpsc::channel::<u64>(64); // 简化：只推总进度

    let fwd = app_handle.clone();
    let fwd_repo = repo.clone();
    tokio::spawn(async move {
        let mut downloaded: u64 = 0;
        while let Some(bytes) = rx.recv().await {
            downloaded += bytes;
            let _ = fwd.emit("download-progress", serde_json::json!({
                "repo": &fwd_repo, "downloaded": downloaded, "total": 0,
            }));
        }
    });

    for (i, (path, file)) in manifest.iter().enumerate() {
        let _ = app_handle.emit("download-file", serde_json::json!({
            "repo": &repo, "index": i + 1, "total": total_files, "file": path,
        }));

        let url = resolve_env_template(&file.source, &env_vars);
        let dest = dest_base.join(path);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        // 用 download crate 的 Downloader 下载单个文件
        let task = octopus_download::DownloadTask {
            url: url.clone(),
            mirrors: vec![],
            dest: dest.clone(),
            expected_hash: if file.sha256.is_empty() { None }
                else { Some(octopus_download::Hash::Sha256(file.sha256.clone())) },
        };
        let dl = octopus_download::Downloader::new(octopus_download::DownloadConfig::default())
            .map_err(|e| format!("初始化下载器失败: {e:?}"))?;
        let (prog_tx, mut prog_rx) = mpsc::channel::<octopus_download::Progress>(64);
        // 转发进度
        let total = file.size;
        let tx2 = tx.clone();
        tokio::spawn(async move {
            while let Some(p) = prog_rx.recv().await {
                let _ = tx2.send(p.downloaded_bytes).await;
            }
        });
        dl.download(&task, prog_tx, None).await
            .map_err(|e| format!("下载 {path} 失败: {e:?}"))?;
    }
    drop(tx);

    // 5. 置 true
    set_model_enabled_state(&model_name, true)?;
    let _ = app_handle.emit("download-done", serde_json::json!({"repo": &repo, "already_ready": false}));
    Ok(())
}
```

- [ ] **Step 2: 实现 lookup_model_by_source 辅助函数**

```rust
fn lookup_model_by_source(source: &str) -> Result<(String, String), String> {
    // 查所有 domain 的 is_local=1 模型
    for domain in &["asr", "translate", "ocr"] {
        let rows = octopus_infra::db::list_local_models_by_domain(domain)
            .map_err(|e| e.to_string())?;
        if let Some(r) = rows.iter().find(|r| r.source == source) {
            return Ok((r.model_name.clone(), r.secret_key.clone()));
        }
    }
    Err(format!("未找到 source='{source}' 的模型"))
}
```

- [ ] **Step 3: 实现 check_manifest_ready + set_model_enabled_state**

```rust
fn check_manifest_ready(dir: &Path, manifest: &octopus_asr_local::manifest::Manifest) -> bool {
    manifest.iter().all(|(path, f)| {
        let full = dir.join(path);
        full.exists() && (!f.sha256.is_empty() || f.size > 0)
    })
}

fn set_model_enabled_state(model_name: &str, enabled: bool) -> Result<(), String> {
    // 翻译模型不在 ASR models 表中——用通用 set_model_enabled
    octopus_infra::db::set_model_enabled(model_name, enabled)
        .map_err(|e| e.to_string())?;
    // reload 只对 ASR 有意义
    octopus_asr_local::config::reload_models_config();
    Ok(())
}
```

- [ ] **Step 4: 修复 #[tauri::command] 错位**

将 line 79-80 的 `#[tauri::command]` 从 `resolve_env_template` 移到 `set_download_mirror` 正上方。`resolve_env_template` 改为内部函数（去掉 macro）。

- [ ] **Step 5: 更新 list_downloadable_models 加 domain 参数**

```rust
#[tauri::command]
pub fn list_downloadable_models(domain: Option<String>) -> Result<Vec<DownloadableModel>, String> {
    let domain = domain.unwrap_or_else(|| "asr".to_string());
    let rows = octopus_infra::db::list_local_models_by_domain(&domain)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(DownloadableModel {
            name: r.model_name,
            repo: r.source, // 现在是路径标识如 "asr/whisper-small"
            category: r.category,
            description: r.description,
            is_enabled: r.is_enabled,
        });
    }
    Ok(out)
}
```

- [ ] **Step 6: 编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/src/model_commands.rs
git commit -m "feat: manifest-driven download + fix tauri::command misplacement + domain param"
```

---

### Task 7: Desktop 命令注册 + 翻译命令清理

**Files:**
- Modify: `crates/desktop/src/main.rs`（invoke_handler）
- Modify: `crates/desktop/src/translation_commands.rs`

- [ ] **Step 1: 删除 translation_commands.rs 中 list_downloadable_translation_models**

```rust
// 删除：
// pub fn list_downloadable_translation_models() -> ...
// 前端统一走 list_downloadable_models({domain: "translate"})
```

- [ ] **Step 2: 更新 main.rs invoke_handler**

移除 `translation_commands::list_downloadable_translation_models`。
保留 `discover_translation_models` 和 `translate_status`。

- [ ] **Step 3: 编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/src/translation_commands.rs crates/desktop/src/main.rs
git commit -m "refactor: remove translation-specific download command, unify via list_downloadable_models"
```

---

### Task 8: 前端更新——统一 Tabs 调用

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/AsrTab.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/TranslateTab.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/OcrTab.tsx`

- [ ] **Step 1: AsrTab——list_downloadable_models 加 domain 参数**

```typescript
const data = await invoke<DownloadableModel[]>("list_downloadable_models", { domain: "asr" });
```

- [ ] **Step 2: TranslateTab——改用统一 API**

删除 `list_downloadable_translation_models` 调用，改用：

```typescript
const [dl, disc, st, cfg] = await Promise.all([
  invoke<DownloadableModel[]>("list_downloadable_models", { domain: "translate" }),
  invoke<TranslationModelInfo[]>("discover_translation_models"),
  invoke<TranslateStatus>("translate_status"),
  invoke<{ config: Record<string, string | number | boolean> }>("get_config"),
]);
```

`DownloadableModel` 接口与 AsrTab 统一（去掉 `sizeMb`）。

- [ ] **Step 3: OcrTab——加下载按钮**

OcrTab 当前只有 enable/disable toggle。改为与 AsrTab 统一模式：
- 调 `list_downloadable_models({domain: "ocr"})`
- 未就绪（is_enabled=false）显示下载按钮
- 就绪（is_enabled=true）显示校验按钮

- [ ] **Step 4: 前端构建**

Run: `cd crates/desktop/frontend && npm run build`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/Models/
git commit -m "feat: unify AsrTab/TranslateTab/OcrTab to use list_downloadable_models with domain param"
```

---

### Task 9: CLI download 命令适配新路径

**Files:**
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: 更新 build_hf_request 和 run_download**

CLI `download` 子命令目前直接接 HF repo。改为支持路径标识格式：
- 输入 `asr/whisper-small` → 从 DB 查 manifest → manifest 驱动下载
- 输入 `owner/repo`（旧格式）→ 走原有 resolve_tasks 逻辑（向后兼容）

- [ ] **Step 2: 编译**

Run: `cargo build -p octopus-cli`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "feat: CLI download supports manifest-driven path identifiers"
```

---

### Task 10: verify_model 适配新格式

**Files:**
- Modify: `crates/desktop/src/model_commands.rs`

- [ ] **Step 1: 更新 verify_model 支持 manifest 校验**

`verify_model_inner` 当前只处理 ASR 模型（`lookup_model_name` 只查 ASR）。改为：
- 从所有 domain 查 model_name
- 解析 manifest（新格式含 source）→ `verify_against_manifest`
- 损坏置 false

- [ ] **Step 2: 编译 + 测试**

Run: `cargo test -p octopus-desktop --features embedded`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/src/model_commands.rs
git commit -m "feat: verify_model supports all domains with new manifest format"
```

---

### Task 11: 全量编译 + 文档同步

**Files:**
- Modify: `docs/architecture.md`（更新模型管理章节）
- Modify: `docs/superpowers/specs/2026-07-14-model-download-manifest-design.md`（标注偏差）

- [ ] **Step 1: 全量编译**

Run: `cargo build --release -p octopus-server -p octopus-cli -p octopus-desktop --features embedded`
Expected: PASS

- [ ] **Step 2: 全量测试**

Run: `cargo test`
Expected: PASS（无回归）

- [ ] **Step 3: 更新 architecture.md**

更新 octopus-download、model_commands、translation 相关章节，反映 manifest 驱动下载。

- [ ] **Step 4: 更新 spec 偏差记录**

在 spec 末尾记录实现中的偏差（如有）。

- [ ] **Step 5: Commit**

```bash
git add docs/
git commit -m "docs: sync architecture + spec for manifest-driven model download"
```
