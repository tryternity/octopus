# octopus-download 接入模型管理（阶段1）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `octopus-download` crate 接入模型管理——`octopus-cli download <repo>` 把 HF 模型下到 `~/.octopus/models/<repo>/`，ASR 的 `resolve_model_dir` 新增一级查找发现它。

**Architecture:** 三处改动正交。(1) `resolve_model_dir`（`crates/asr/src/config.rs`）在 HF-cache 查找前插入 `~/.octopus/models/<source>` 级，纯查找语义不变、缺失时报错并提示 `octopus-cli download`。(2) `AppConfig`（`crates/infra`）新增可选 `download_mirror` 字段（DB app_config 表 + struct + load/save + seed 同步），给下载镜像一个持久配置位。(3) `cli` 加 `Download` 子命令，薄封装 download crate（`build_hf_request` → `resolve_tasks` → 逐文件 `Downloader::download` + 进度），mirror 优先级 `--mirror` > config > 官方源。

**Tech Stack:** Rust，clap（cli 子命令），tokio（async runtime），octopus-download（HF 适配层 + 分块并发下载器），octopus-infra（AppConfig + DB），rusqlite（app_config 表）。

**Spec:** `docs/superpowers/specs/2026-06-21-download-model-integration-design.md`

---

## Spec 勘误（实施前必读）

Spec §2.2 / §3.2 称「3 处绕过 `resolve_model_dir` 直接拼 `.cache/huggingface/hub`」，列出 `streaming_paraformer.rs:797` / `zipformer.rs:1297` / `streaming_zipformer.rs:912`。

**实测结论：这 3 处全部位于 `#[cfg(test)] mod tests` 内的测试辅助函数 `hf_snapshot`，不是生产代码：**

| 位置 | 函数 | 上下文 |
|---|---|---|
| `streaming_paraformer.rs:796` | `fn hf_snapshot(repo)` | `#[cfg(test)] mod tests`（mod 起始于 `:792`） |
| `zipformer.rs:1295` | `fn hf_snapshot(repo)` | `#[cfg(test)] mod tests`（mod 起始于 `:1289`） |
| `streaming_zipformer.rs:910` | `fn hf_snapshot(repo)` | `#[cfg(test)] mod tests`（mod 起始于 `:904`） |

它们是集成测试用来动态定位本地 HF snapshot（跑真实模型的 `#[test]`），不影响生产 `resolve` 路径。统一它们属可选优化、收益低（repo 参数语义还与 `resolve_model_dir` 的 `source` 不一致——paraformer 传带 `models--` 前缀的格式，而 `resolve_model_dir` 接受原始 repo 名），按 YAGNI **本计划不纳入**。Spec §2.2 第 2 点、§3.2 整节作废；Task 4 会回写 spec 标注此勘误。

生产代码的真实调用点是 **13+ 处引擎 `resolve_model_dir(&entry.source)`**（spec §2.2 第 1 点）——这些是本计划要生效的对象，Task 1 的查找级扩展自动惠及它们。

---

## File Structure

| 文件 | 职责 | 本计划改动 |
|---|---|---|
| `crates/asr/src/config.rs` | 模型目录解析 + 引擎路由 | `resolve_model_dir` 加查找级；抽可测内核 `resolve_local_in`；`find_hf_cache` 错误提示改 cli |
| `crates/infra/src/config.rs` | `AppConfig` schema（config.yaml/DB 的唯一来源） | 加 `download_mirror` 字段 + default + Default |
| `crates/infra/src/db.rs` | app_config 表读写 | `load_app_config_at` match 加分支；`save_app_config_at` 数组 21→22 |
| `crates/infra/src/db.sql` | app_config seed | 加 `download_mirror` seed 行 |
| `crates/cli/src/main.rs` | cli 子命令 | 加 `Download` 变体 + `build_hf_request`（可测）+ `run_download` + main match |
| `crates/cli/Cargo.toml` | cli 依赖 | 加 `octopus-download` |

`resolve_model_dir` 的可测性改进：当前它直接调 `octopus_config_home()`（进程级 `Lazy` 锁定 `$HOME/.octopus`，测试无法注入），无法单测。本计划抽出前 3 级（基于传入 `octopus_home: &Path`）为内部纯函数 `resolve_local_in`，第 4 级（HF cache，依赖真实 `$HOME`）仍由 `find_hf_cache` 处理。这样查找逻辑可单测，且不改变任何外部 API 签名。

---

## Task 1: resolve_model_dir 扩展查找级 + 错误提示

**Files:**
- Modify: `crates/asr/src/config.rs:65`（`resolve_model_dir`，抽 `resolve_local_in` + 加第 3 级）
- Modify: `crates/asr/src/config.rs:34`（`find_hf_cache` 错误提示改 cli download）
- Test: `crates/asr/src/config.rs` 末尾 `#[cfg(test)] mod tests`（已存在于 `:484`）

- [x] **Step 1: 写 `resolve_local_in` 的失败测试**

在 `crates/asr/src/config.rs` 的 `#[cfg(test)] mod tests` 内（现有 `make_entry` 等 helper 之后，`order_engine_infos_sorts...` 测试之前）追加：

```rust
    // ── resolve_local_in 查找内核测试（阶段1：download 模型发现）──

    #[test]
    fn resolve_local_in_finds_bundled_relative() {
        // 第 1 级：octopus_home/<source>（随包小模型，如 models/zipformer）
        let tmp = std::env::temp_dir().join("octopus_t_resolve_bundled");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("models/zipformer")).unwrap();
        let p = super::resolve_local_in("models/zipformer", &tmp).unwrap();
        assert_eq!(p, tmp.join("models/zipformer"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_local_in_finds_downloaded_hf_repo() {
        // 第 3 级（新增）：octopus_home/models/<source>，source 是含 / 的 HF repo 名
        let tmp = std::env::temp_dir().join("octopus_t_resolve_downloaded");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("models/onnx-community/whisper-small")).unwrap();
        let p = super::resolve_local_in("onnx-community/whisper-small", &tmp).unwrap();
        assert_eq!(p, tmp.join("models/onnx-community/whisper-small"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_local_in_finds_absolute_path() {
        // 第 2 级：source 是绝对路径
        let tmp = std::env::temp_dir().join("octopus_t_resolve_abs");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let p = super::resolve_local_in(tmp.to_str().unwrap(), &std::env::temp_dir()).unwrap();
        assert_eq!(p, tmp);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_local_in_returns_none_when_missing() {
        // 前 3 级全 miss → None（HF cache 第 4 级由 find_hf_cache 处理，不在本函数）
        let tmp = std::env::temp_dir().join("octopus_t_resolve_missing");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(super::resolve_local_in("nonexistent/repo", &tmp).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-asr-local resolve_local_in`
Expected: 编译失败——`error[E0425]: cannot find function resolve_local_in in module config`（或 `not found in this scope`）。

- [x] **Step 3: 实现 `resolve_local_in` 并改造 `resolve_model_dir`**

把 `crates/asr/src/config.rs:61-78`（`resolve_model_dir` 函数及其上方 3 行 doc 注释）替换为：

```rust
/// 前 3 级模型目录查找（基于给定 octopus_home，可单测；不依赖全局 `$HOME`）。
///
/// 1. `octopus_home/<source>`（随包小模型，如 `models/zipformer`）
/// 2. 绝对路径（`source` 本身是绝对路径）
/// 3. `octopus_home/models/<source>`（download 下的 HF 模型，source 如 `onnx-community/whisper-small`）
///
/// 返回 `None` 表示前 3 级全 miss，调用方应回退第 4 级 HF cache（`find_hf_cache`）。
fn resolve_local_in(source: &str, octopus_home: &Path) -> Option<PathBuf> {
    // 1. octopus_home 下相对路径（随应用打包的小模型）
    let local = octopus_home.join(source);
    if local.is_dir() {
        return Some(local);
    }
    // 2. 绝对路径（join 绝对路径会覆盖 base，等效直接判断 source 本身）
    let abs = PathBuf::from(source);
    if abs.is_dir() {
        return Some(abs);
    }
    // 3. download 下的 HF 模型（~/.octopus/models/<source>）★ 阶段1 新增
    let downloaded = octopus_home.join("models").join(source);
    if downloaded.is_dir() {
        return Some(downloaded);
    }
    None
}

/// 解析模型目录：前 3 级本地查找（随包 / 绝对路径 / download 下载），回退 HF 缓存。
/// - source 为本地相对路径（如 "models/zipformer"）→ octopus_config_home/source
/// - source 为绝对路径 → 直接用
/// - source 为 HF repo 名（如 "onnx-community/whisper-small"）→ 优先 ~/.octopus/models/<source>（download 下到这里），
///   否则 find_hf_cache（兼容已用 hf-cli 下的 ~/.cache/huggingface）
pub fn resolve_model_dir(source: &str) -> Result<PathBuf> {
    if let Some(p) = resolve_local_in(source, octopus_config_home()) {
        return Ok(p);
    }
    find_hf_cache(source)
}
```

- [x] **Step 4: 运行测试确认通过**

Run: `cargo test -p octopus-asr-local resolve_local_in`
Expected: 4 个测试 PASS。

- [x] **Step 5: 改 `find_hf_cache` 错误提示**

把 `crates/asr/src/config.rs:41-47`（`find_hf_cache` 里 `if !model_dir.exists()` 的 `anyhow::bail!`）替换为：

```rust
    if !model_dir.exists() {
        anyhow::bail!(
            "模型 '{}' 未在 ~/.octopus/models/ 或 HF cache 找到。请运行 `octopus-cli download {}` 下载。",
            source,
            source
        );
    }
```

- [x] **Step 6: 跑 asr 全量测试确认无回归**

Run: `cargo test -p octopus-asr-local`
Expected: 全部 PASS（含既有 `pick_entry` / `resolve_*` / `parse_spec_*` 等；`resolve_local_in` 4 个新测试）。

- [x] **Step 7: Commit**

```bash
git add crates/asr/src/config.rs
git commit -m "feat(asr): resolve_model_dir 加 ~/.octopus/models/<source> 查找级 + 缺失提示 cli download"
```

---

## Task 2: AppConfig 新增 download_mirror 字段（DB 同步）

**Files:**
- Modify: `crates/infra/src/config.rs:148`（struct 加字段）、`:200`（default fn）、`:231`（Default impl）
- Modify: `crates/infra/src/db.rs:281`（load match 加字符串分支）、`:323`/`:344`（save 数组 21→22）
- Modify: `crates/infra/src/db.sql:112`（seed 加行，末行分号改逗号）
- Test: `crates/infra/src/config.rs` `#[cfg(test)]`、`crates/infra/src/db.rs` `#[cfg(test)]`

- [x] **Step 1: 写 config.rs 的失败测试**

在 `crates/infra/src/config.rs` 的 `#[cfg(test)] mod tests` 末尾（`edit_shortcut_explicit_from_yaml` 测试之后）追加：

```rust
    #[test]
    fn download_mirror_defaults_empty() {
        assert_eq!(AppConfig::default().download_mirror, "");
    }

    #[test]
    fn download_mirror_parsed_from_yaml() {
        let cfg: AppConfig =
            serde_yaml::from_str("download_mirror: https://hf-mirror.com\n").unwrap();
        assert_eq!(cfg.download_mirror, "https://hf-mirror.com");
    }

    #[test]
    fn download_mirror_absent_keeps_default() {
        // 缺该字段的旧 config → default 空（serde default）
        let cfg: AppConfig = serde_yaml::from_str("language: zh\n").unwrap();
        assert_eq!(cfg.download_mirror, "");
    }
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-infra download_mirror`
Expected: 编译失败——`no field download_mirror on type AppConfig`。

- [x] **Step 3: AppConfig struct 加字段**

在 `crates/infra/src/config.rs` 的 `AppConfig` struct 内，`edit_shortcut` 字段（`:144-147`）之后追加：

```rust
    /// HF 模型下载镜像 host（如 `https://hf-mirror.com`）。空 = 官方源 huggingface.co。
    /// cli `download --mirror` 临时覆盖此值；优先级 `--mirror` > 此字段 > 官方源。
    #[serde(default = "default_download_mirror")]
    pub download_mirror: String,
```

- [x] **Step 4: 加 default 函数**

在 `crates/infra/src/config.rs` 的 `default_edit_shortcut` 函数（`:200-202`）之后追加：

```rust
fn default_download_mirror() -> String {
    String::new()
}
```

- [x] **Step 5: Default impl 加字段**

在 `crates/infra/src/config.rs` 的 `impl Default for AppConfig`（`:207-234`）内，`edit_shortcut: default_edit_shortcut(),`（`:231`）之后追加：

```rust
            download_mirror: default_download_mirror(),
```

- [x] **Step 6: 运行 config 测试确认通过**

Run: `cargo test -p octopus-infra download_mirror`
Expected: 3 个测试 PASS。

- [x] **Step 7: db.rs load 加分支**

在 `crates/infra/src/db.rs:281`（load_app_config_at 的字符串字段组，`"polish_llm" => cfg.polish_llm = value,` 之后）追加一行：

```rust
            "download_mirror" => cfg.download_mirror = value,
```

- [x] **Step 8: db.rs save 数组 21→22**

把 `crates/infra/src/db.rs:323` 的类型签名：

```rust
    let fields: [(&str, String); 21] = [
```

改为：

```rust
    let fields: [(&str, String); 22] = [
```

并在数组末尾元素 `("denoise_mode", cfg.denoise_mode.to_string()),`（`:344`，下一行 `:345` 是 `];`）之后追加 `download_mirror`：

```rust
        ("denoise_mode", cfg.denoise_mode.to_string()),
        ("download_mirror", cfg.download_mirror.clone()),
    ];
```

- [x] **Step 9: db.sql seed 加行**

把 `crates/infra/src/db.sql:111-112`：

```sql
    ('hide_toolbar',             'false',                                '结果展示区工具栏是否自动隐藏'),
    ('denoise_mode',             '1',                                    '降噪模式: 0=无 / 1=轻度 / 2=深度');
```

改为（`denoise_mode` 行末 `;` 改 `,`，追加 `download_mirror` 行并以 `;` 结尾）：

```sql
    ('hide_toolbar',             'false',                                '结果展示区工具栏是否自动隐藏'),
    ('denoise_mode',             '1',                                    '降噪模式: 0=无 / 1=轻度 / 2=深度'),
    ('download_mirror',          '',                                     'HF 模型下载镜像 host（如 https://hf-mirror.com），空=官方源 huggingface.co');
```

- [x] **Step 10: 把 download_mirror 纳入既有 db 测试**

`crates/infra/src/db.rs` 的 `#[cfg(test)]` 已有两个测试覆盖 app_config seed + round-trip，扩展它们：

(1) `app_config_seed_provides_all_fields`（`:1164`）：在末尾 `assert_eq!(cfg.edit_shortcut, "Cmd+Enter");`（`:1176`）之后追加一行验证 seed 默认空：

```rust
        assert_eq!(cfg.download_mirror, "");
```

(2) `save_and_reload_preserves_overrides`（`:1179`）：在 `cfg.denoise_mode = 2;`（`:1188`）之后加：

```rust
        cfg.download_mirror = "https://hf-mirror.com".to_string();
```

并在 reload 断言区 `assert_eq!(cfg2.denoise_mode, 2);`（`:1196`）之后追加：

```rust
        assert_eq!(cfg2.download_mirror, "https://hf-mirror.com");
```

- [x] **Step 11: 运行 db 测试确认通过**

Run: `cargo test -p octopus-infra app_config`
Expected: seed + round-trip 测试 PASS（含新断言）。

- [x] **Step 12: workspace 编译确认**

Run: `cargo check -p octopus-infra`
Expected: 编译通过，0 warning。

- [x] **Step 13: Commit**

```bash
git add crates/infra/src/config.rs crates/infra/src/db.rs crates/infra/src/db.sql
git commit -m "feat(infra): AppConfig 加 download_mirror 字段（DB app_config 同步）"
```

---

## Task 3: cli Download 子命令

**Files:**
- Modify: `crates/cli/Cargo.toml`（加 `octopus-download` 依赖）
- Modify: `crates/cli/src/main.rs:13`（Commands enum 加 Download）、`:62`（main match 加分支）、文件末尾追加 `build_hf_request` + `run_download`
- Test: `crates/cli/src/main.rs` 末尾 `#[cfg(test)] mod tests`（新建）

- [x] **Step 1: cli Cargo.toml 加依赖**

在 `crates/cli/Cargo.toml` 的 `[dependencies]` 内，`octopus-infra = { path = "../infra" }` 之后追加：

```toml
octopus-download = { path = "../download" }
```

> 不加 `reqwest`：`run_download` 复用 `Downloader::client()`（download crate 内部的 reqwest::Client），`resolve_tasks` 接 `&reqwest::Client`，类型来自 download crate，cli 不直接命名 reqwest 类型。

- [x] **Step 2: 写 `build_hf_request` 的失败测试**

`crates/cli/src/main.rs` 当前无 `#[cfg(test)]`。在文件末尾追加整个测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::build_hf_request;

    #[test]
    fn build_request_cli_mirror_overrides_config() {
        // --mirror 优先于 config download_mirror
        let req = build_hf_request(
            "onnx-community/whisper-small".into(),
            vec!["onnx/*_int8.onnx".into()],
            vec![],
            Some("https://hf-mirror.com".into()),
            "https://ignored.example.com",
        );
        assert_eq!(req.repo, "onnx-community/whisper-small");
        assert_eq!(req.source_url.as_deref(), Some("https://hf-mirror.com"));
        assert_eq!(req.include, vec!["onnx/*_int8.onnx"]);
        assert!(req.target_dir.ends_with("models"));
    }

    #[test]
    fn build_request_config_mirror_when_no_cli() {
        // 无 --mirror → 用 config
        let req = build_hf_request(
            "org/m".into(),
            vec![],
            vec![],
            None,
            "https://hf-mirror.com",
        );
        assert_eq!(req.source_url.as_deref(), Some("https://hf-mirror.com"));
    }

    #[test]
    fn build_request_none_when_both_empty() {
        // cli 空 + config 空 → None（官方源，由 download crate 默认）
        let req = build_hf_request("org/m".into(), vec![], vec![], Some(String::new()), "");
        assert!(req.source_url.is_none());
        assert!(req.target_dir.ends_with("models"));
    }

    #[test]
    fn build_request_target_dir_under_octopus_models() {
        // target_dir 必须是 octopus_config_home/models（与 resolve_model_dir 第 3 级一致）
        let req = build_hf_request("org/m".into(), vec![], vec![], None, "");
        let expected = octopus_infra::octopus_config_home().join("models");
        assert_eq!(req.target_dir, expected);
    }
}
```

- [x] **Step 3: 运行测试确认失败**

Run: `cargo test -p octopus-cli build_request`
Expected: 编译失败——`cannot find function build_hf_request`。

- [x] **Step 4: 实现 `build_hf_request`**

在 `crates/cli/src/main.rs` 末尾（Step 2 的 `#[cfg(test)]` 之前——即测试模块上方）追加：

```rust
// ── download 子命令 ──

/// 构造 HF 下载请求。mirror 优先级：cli `--mirror` > config `download_mirror` > 空（官方源）。
/// target_dir 固定 `~/.octopus/models`，与 `resolve_model_dir` 第 3 级（`~/.octopus/models/<repo>`）一致。
fn build_hf_request(
    repo: String,
    include: Vec<String>,
    exclude: Vec<String>,
    cli_mirror: Option<String>,
    config_mirror: &str,
) -> octopus_download::HfRequest {
    let mirror = cli_mirror
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let c = config_mirror.trim();
            if c.is_empty() {
                None
            } else {
                Some(c.to_string())
            }
        });
    octopus_download::HfRequest {
        repo,
        include,
        exclude,
        source_url: mirror,
        target_dir: octopus_infra::octopus_config_home().join("models").to_path_buf(),
    }
}
```

- [x] **Step 5: 运行测试确认通过**

Run: `cargo test -p octopus-cli build_request`
Expected: 4 个测试 PASS。

- [x] **Step 6: Commands enum 加 Download 变体**

在 `crates/cli/src/main.rs:13` 的 `enum Commands` 内，`TranscribeUrl { ... }` 变体（`:44-59`，即 enum 最后一个变体）之后追加：

```rust
    /// 下载 HuggingFace 模型到 ~/.octopus/models/<repo>
    Download {
        /// HF repo，如 onnx-community/whisper-small（与 DB models 的 entry.source 一致）
        repo: String,
        /// 只下匹配的文件（glob，对齐 hf-cli，`*` 跨 `/`）。空 = 下整库
        #[arg(long)]
        include: Vec<String>,
        /// 排除匹配的文件
        #[arg(long)]
        exclude: Vec<String>,
        /// HF 镜像 host（如 https://hf-mirror.com），覆盖 config 的 download_mirror
        #[arg(long)]
        mirror: Option<String>,
    },
```

- [x] **Step 7: main match 加分支**

在 `crates/cli/src/main.rs:62` 的 `match cli.command` 内，`Commands::TranscribeUrl { ... } => { ... }` 分支（`:74-83`，即 match 最后一个 arm）之后追加：

```rust
        Commands::Download {
            repo,
            include,
            exclude,
            mirror,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run_download(&repo, &include, &exclude, mirror.as_deref()))
        }
```

- [x] **Step 8: 实现 `run_download`**

在 `crates/cli/src/main.rs` 的 `build_hf_request` 函数（Step 4 追加的）之后追加：

```rust
/// 执行下载：resolve 文件列表 → 逐文件 Downloader::download + 进度打印。
/// 失败透传 anyhow（resolve 网络 / hash 校验 / 镜像 fallback 均由 download crate 处理）。
async fn run_download(
    repo: &str,
    include: &[String],
    exclude: &[String],
    cli_mirror: Option<&str>,
) -> Result<()> {
    let app_cfg = octopus_infra::config::load_config()?;
    let req = build_hf_request(
        repo.to_string(),
        include.to_vec(),
        exclude.to_vec(),
        cli_mirror.map(|s| s.to_string()),
        &app_cfg.download_mirror,
    );

    println!("解析 {} 的文件列表...", repo);
    let dl = octopus_download::Downloader::new(octopus_download::DownloadConfig::default())
        .map_err(|e| anyhow::anyhow!("初始化下载器失败: {e:?}"))?;
    let tasks = octopus_download::resolve_tasks(dl.client(), req)
        .await
        .map_err(|e| anyhow::anyhow!("resolve 失败: {e:?}"))?;
    if tasks.is_empty() {
        anyhow::bail!("没有匹配的文件——检查 --include/--exclude glob");
    }
    println!(
        "共 {} 个文件 → {}",
        tasks.len(),
        octopus_infra::octopus_config_home().join("models").display()
    );

    for (i, task) in tasks.iter().enumerate() {
        println!("[{}/{}] {}", i + 1, tasks.len(), task.dest.display());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<octopus_download::Progress>(64);
        // rx move 进 printer：download 返回后 tx drop → channel 关闭 → rx.recv() 返回 None → printer 自然退出。
        // 勿在主作用域再 rx.close()——rx 已 move 进闭包，访问即 use-of-moved-value 编译错。
        let printer = tokio::spawn(async move {
            while let Some(p) = rx.recv().await {
                if let Some(total) = p.total_bytes {
                    let pct = p.downloaded_bytes as f64 / total as f64 * 100.0;
                    // 速度：download crate 250ms 推送 EMA 估算；下大模型时是关键 UX。
                    let spd = p
                        .speed_bps
                        .map(|s| format!(" {:.2} MB/s", s / 1_048_576.0))
                        .unwrap_or_default();
                    eprint!(
                        "\r  {}/{} bytes ({:.1}%){}   ",
                        p.downloaded_bytes, total, pct, spd
                    );
                }
            }
        });
        dl.download(task, tx, None)
            .await
            .map_err(|e| anyhow::anyhow!("下载 {} 失败: {e:?}", task.dest.display()))?;
        let _ = printer.await;
        // \x1b[2K 清当前行——进度行可能比 "✓ done" 长（大文件字节数多），不清会残留尾巴。
        eprintln!("\r\x1b[2K  ✓ done");
    }

    println!("\n完成。模型位于 ~/.octopus/models/{}/", repo);
    Ok(())
}
```

> 说明：`dl.client()` 复用 Downloader 内部 reqwest::Client 给 `resolve_tasks`，避免 cli 直接依赖 reqwest。`dl.download(&self, ...)` 不可 move 进 spawn，故在主循环顺序 await（多文件串行下载）。

- [x] **Step 9: 编译 + 全量测试**

Run: `cargo test -p octopus-cli`
Expected: 编译通过；4 个 `build_request` 测试 PASS。

- [x] **Step 10: 手工冒烟（真实下载，受网络限制不计入 CI）**

Run:
```bash
cargo run -p octopus-cli -- download onnx-community/whisper-tiny --include 'onnx/model_int8.onnx' --mirror https://hf-mirror.com
```
Expected: 打印「解析...」「共 N 个文件」→ 逐文件进度条 → 「完成。模型位于 ~/.octopus/models/onnx-community/whisper-tiny/」。`ls ~/.octopus/models/onnx-community/whisper-tiny/onnx/model_int8.onnx` 存在。

> 手工验证项（网络可用时）：(a) 不带 `--mirror` 且 config 无 `download_mirror` → 走官方源；(b) `resolve_model_dir("onnx-community/whisper-tiny")` 现能命中 `~/.octopus/models/onnx-community/whisper-tiny`（Task 1 第 3 级生效，可用 `octopus-cli config` 观察路径）。`run_download` 的完整 e2e 不纳入单测——Downloader 自建 reqwest client、连真实 HF，httpmock 无法注入；下载核心逻辑已由 download crate 自身的 httpmock 测试覆盖。

- [x] **Step 11: Commit**

```bash
git add crates/cli/Cargo.toml crates/cli/src/main.rs
git commit -m "feat(cli): 加 download 子命令（薄封装 octopus-download，mirror 优先级 cli>config>官方）"
```

---

## Task 4: 文档同步 + 收尾验证

**Files:**
- Modify: `docs/superpowers/specs/2026-06-21-download-model-integration-design.md`（§2.2/§3.2 勘误、§4 接口契约状态）
- Modify: `docs/superpowers/plans/2026-06-21-download-model-integration.md`（勾选完成的 step）
- Modify: `docs/architecture.md`（cli 加 download 子命令）

- [x] **Step 1: spec §2.2 / §3.2 勘误**

在 `docs/superpowers/specs/2026-06-21-download-model-integration-design.md`：

§2.2 第 2 点（「3 处绕过 resolve_model_dir...」整段，含 3 个文件:行号列表）替换为：

```markdown
- 实测：曾怀疑的 3 处「直接拼 `.cache/huggingface/hub`」（`streaming_paraformer.rs:796` / `zipformer.rs:1295` / `streaming_zipformer.rs:910`）经核实全部位于 `#[cfg(test)] mod tests` 的测试辅助 `hf_snapshot`，非生产代码，不影响 resolve 路径——不纳入统一。
```

§3.2 整节（「### 3.2 统一 3 处直接拼路径」）替换为：

```markdown
### 3.2 ~~统一 3 处直接拼路径~~（已撤销）

实施前实测：§2.2 列出的 3 处均在 `#[cfg(test)]` 测试辅助 `hf_snapshot` 内，非生产路径；统一它们收益低且 repo 参数语义与 `resolve_model_dir(source)` 不一致。按 YAGNI 不做。生产调用点（13+ 处引擎 `resolve_model_dir(&entry.source)`）由 3.1 的查找级扩展自动覆盖。
```

- [x] **Step 2: spec §4 接口契约表补状态**

§4 表格「config.yaml | 新增可选 `download.mirror`」一行的「变化」列改为：

```markdown
新增可选 `download_mirror`（AppConfig flat 字段，非嵌套 `download.mirror`；DB app_config 表同步）
```

- [x] **Step 3: architecture.md 补 cli download**

在 `docs/architecture.md` 描述 octopus-cli 的段落（或 `### octopus-cli` 模块说明），追加一句：

```markdown
- `download` 子命令：薄封装 octopus-download，把 HF 模型下到 `~/.octopus/models/<repo>/`；`--mirror` 优先于 config 的 `download_mirror`，source_url 作主源、官方 huggingface.co 自动作 fallback mirror。与 `resolve_model_dir` 第 3 级查找对接（下完即可被 ASR 引擎发现）。
```

> 若 architecture.md 用模块小节形式，按既有风格把这段并入 `octopus-cli` 小节即可；若该文件无 cli 专门小节，在 workspace 模块列表里补一行。

- [x] **Step 4: workspace 全量编译 + 测试**

Run: `cargo check --workspace --all-targets`
Expected: 编译通过，0 warning。

Run: `cargo test --workspace`
Expected: 全部 PASS（asr `resolve_local_in` ×4、infra `download_mirror` ×3 + app_config round-trip、cli `build_request` ×4，及既有测试无回归）。

- [x] **Step 5: 勾选本 plan 已完成 step**

把本计划中 Task 1–4 所有已完成 step 的 `- [ ]` 改为 `- [x]`。

- [x] **Step 6: Commit**

```bash
git add docs/superpowers/specs/2026-06-21-download-model-integration-design.md docs/superpowers/plans/2026-06-21-download-model-integration.md docs/architecture.md
git commit -m "docs: 同步 download 模型管理阶段1（spec §3.2 勘误 + architecture cli download）"
```

---

## 范围确认（本计划不做）

- **不碰 ort**（阶段 ② load-dynamic）。
- **不删 HF cache 兼容**：`resolve_model_dir` 仍回退 `find_hf_cache`（`~/.cache/huggingface`），兼容已用 hf-cli 的用户。
- **不统一 3 处测试辅助** `hf_snapshot`（spec §3.2 勘误，YAGNI）。
- **不做 GUI 模型管理页**（lib-first；setting-ui2 若复活再消费）。
- **不加 DB models 表的 source 自动改写**：用户手编 DB models 的 `source` 仍照旧，download 与 resolve 通过 `~/.octopus/models/<source>` 目录约定对接，不需要 DB schema 改动。

## 后续阶段（不属于本计划）

- **② ort load-dynamic**：`asr/Cargo.toml` 的 ort 从 `download-binaries` 改 `load-dynamic`，初始化指向 `~/.octopus/bin/` 的 dylib；各 binary 掉 ~20-35M 静态 ort。
- **③ download 拉 ort 运行时**：download 增加拉 `libonnxruntime` 能力（版本对齐 ort 2.0.0-rc.12、平台包、镜像 fallback）→ `~/.octopus/bin/`。
- **④ 分发打包**：三 binary 共享 `~/.octopus/bin/libonnxruntime`；发行包不含静态 ort。
