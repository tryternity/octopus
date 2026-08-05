// db/models.rs —— 模型配置（models 表 / local_asr_models / app_config asr_cloud_model|llm_provider）CRUD。
//
// 含：ModelEntry/AsrSection/AsrConfig/CompatibleLlmConfig + ModelSpec + LocalAsrModelRow/ModelRow/
// ModelDetailRow/AsrEngineRow/LlmProviderPresetRow/LlmModelInfo/OcrModelInfo + 所有 model CRUD。

use super::{collect_rows, ensure_db, with_db, Connection, HashMap, Result, params};
use rusqlite::OptionalExtension;

// ── Model config schema（DB models 表）──

#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
pub struct ModelEntry {
    pub source: String,
    #[serde(default)]
    pub language: String,
    /// Secret key (API key) for remote API-based ASR engines, if applicable.
    #[serde(default)]
    pub secret_key: String,
    /// 模型来源: 0=builtin(内置) 1=local(用户下载) 2=cloud(云端)。
    /// serde default = 1（local）—— 向后兼容旧 YAML/JSON（无此字段时按本地模型处理）。
    #[serde(default = "default_local_source_type")]
    pub source_type: i64,
    #[serde(default)]
    pub is_enabled: bool,
    #[serde(default)]
    pub is_available: bool,
    #[serde(default)]
    pub is_streaming: bool,
    #[serde(default)]
    pub description: String,
}

fn default_local_source_type() -> i64 { 1 }

impl ModelEntry {
    /// 是否为内置模型（随应用/首次启动下载，开箱即用）。
    pub fn is_builtin(&self) -> bool { self.source_type == 0 }
    /// 是否为用户下载的本地模型。
    pub fn is_local(&self) -> bool { self.source_type == 1 }
    /// 是否为云端模型（API 调用）。
    pub fn is_cloud(&self) -> bool { self.source_type == 2 }
    /// 是否为本地模型（builtin 或 local）—— 与旧 `is_local == true` 语义等价。
    /// 用于判断 secret_key 是否为 manifest JSON（vs 云端 API Key）、是否需要 vault 解密。
    pub fn is_local_or_builtin(&self) -> bool { self.source_type <= 1 }
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize, Clone)]
pub struct AsrSection {
    pub whisper: Option<HashMap<String, ModelEntry>>,
    /// 原版 SenseVoice-Small（FunASR 4 输入 ONNX，非 sherpa 简化版）。provider='local' + category='sensevoice-orig' 路由入此。
    #[serde(default)]
    pub sensevoice_orig: Option<HashMap<String, ModelEntry>>,
    #[serde(default)]
    pub paraformer: Option<HashMap<String, ModelEntry>>,
    #[serde(default, rename = "qwen3-asr")]
    pub qwen3_asr: Option<HashMap<String, ModelEntry>>,
    #[serde(default)]
    pub zipformer: Option<HashMap<String, ModelEntry>>,
    /// Moonshine 端侧 ASR（Useful Sensors）。provider='local' + category='moonshine' 路由入此。
    #[serde(default)]
    pub moonshine: Option<HashMap<String, ModelEntry>>,
    /// FireRedASR2-AED CTC（小红书，本地）。provider='local' + category='firered' 路由入此。
    #[serde(default)]
    pub firered: Option<HashMap<String, ModelEntry>>,
    /// 阿里云云端 ASR（DashScope Fun-ASR 实时）。provider='aliyun' 路由入此。
    #[serde(default)]
    pub aliyun: Option<HashMap<String, ModelEntry>>,
    /// 火山引擎豆包大模型 ASR（bigmodel_async 双向流式优化版）。provider='bytedance' 路由入此。
    #[serde(default)]
    pub bytedance: Option<HashMap<String, ModelEntry>>,
    /// 腾讯云实时语音识别（WebSocket HMAC-SHA1 签名鉴权）。provider='tencent' 路由入此。
    #[serde(default)]
    pub tencent: Option<HashMap<String, ModelEntry>>,
    /// 百度智能云实时语音识别（WebSocket START 帧鉴权）。provider='baidu' 路由入此。
    #[serde(default)]
    pub baidu: Option<HashMap<String, ModelEntry>>,
}

/// DB models 表配置（domain='asr'；由 db::load_models 构造）。
#[derive(Debug, Default, serde::Deserialize, serde::Serialize, Clone)]
pub struct AsrConfig {
    pub asr: AsrSection,
}

/// 兼容 OpenAI 接口的 LLM 配置
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompatibleLlmConfig {
    /// 提供商标识（如 "openai", "deepseek"），仅用于日志
    pub provider: String,
    /// 模型名（如 "gpt-4o-mini", "deepseek-chat"）
    pub model: String,
    /// API base URL（如 "https://api.openai.com/v1"）
    pub base_url: String,
    /// API Key
    pub secret_key: String,
    /// 是否为思考（reasoning）模型。
    pub is_thinking: bool,
    /// 模型来源: 0=builtin 1=local 2=cloud（详见 ModelEntry.source_type）。
    #[serde(default = "default_local_source_type")]
    pub source_type: i64,
    /// 是否启用。
    pub is_enabled: bool,
}

impl CompatibleLlmConfig {
    /// 润色时是否需要显式关闭思考模式。
    pub fn needs_disable_thinking(&self) -> bool {
        self.is_thinking
    }
}

// ── Model spec 解析（统一 asr_engine / polish_llm 配置格式）──

/// 模型选择规格，统一 `asr_engine` 和 `polish_llm` 的 3-part 格式
/// `{provider}:{category}:{model_name}`。
///
/// | 配置写法 | 含义 |
/// |---------|------|
/// | `"PROVIDER:CATEGORY:NAME"` | 三段精确匹配 `provider AND category AND model_name` |
/// | `"NAME"`（无冒号） | 跨 provider/category 搜 name，优先 local（全局默认 fallback 用） |
/// | `"X:Y"`（1 个冒号，旧 2-part） | warn + 按整串作裸名兜底（NameOnly，向后兼容） |
///
/// 1 个冒号（旧 2-part 格式）按裸名兜底（NameOnly）并 warn。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSpec<'a> {
    /// `{provider}:{category}:{model_name}` 三段精确匹配
    Full { provider: &'a str, category: &'a str, model_name: &'a str },
    /// 裸 `{model_name}`：仅全局默认 fallback 用（跨 provider/category 搜 name，优先 local）
    NameOnly(&'a str),
}

/// 解析 3-part 规格字符串。
/// - 2 个冒号（3 段）→ Full
/// - 0 冒号 → NameOnly
/// - 1 冒号（旧 2-part 格式）→ warn + 按 NameOnly 兜底
pub fn parse_model_spec(spec: &str) -> ModelSpec<'_> {
    let parts: Vec<&str> = spec.split(':').collect();
    match parts.len() {
        3 => ModelSpec::Full { provider: parts[0], category: parts[1], model_name: parts[2] },
        1 => ModelSpec::NameOnly(parts[0]),
        _ => {
            log::warn!(
                "模型 spec '{}' 非合法 3-part '{{provider}}:{{category}}:{{model_name}}'，按裸名兜底",
                spec
            );
            ModelSpec::NameOnly(spec)
        }
    }
}

impl<'a> ModelSpec<'a> {
    /// 返回 model_name（去掉 provider:/category: 前缀）。
    pub fn model_name(&self) -> &'a str {
        match self {
            ModelSpec::Full { model_name, .. } | ModelSpec::NameOnly(model_name) => model_name,
        }
    }
}

// ── DB → AsrConfig（load_config 用）──

/// 从 DB models 表构造 AsrConfig（domain='asr'）。
pub fn load_models() -> Result<AsrConfig> {
    ensure_db()?;
    with_db(load_models_at)
}

pub(crate) fn load_models_at(conn: &Connection) -> Result<AsrConfig> {
    // 新语义：is_enabled=1 表激活（每域仅 1 个），is_available=1 表可用。
    // 推理路径只需激活的那一个——LIMIT 1（虽然每域只有一个 is_enabled=1，加 LIMIT 保险）。
    let mut stmt = conn.prepare(
        "SELECT provider, category, model_name, source, language, description, secret_key, source_type, is_enabled, is_streaming
         FROM models WHERE domain='asr' AND is_enabled=1 AND is_available=1 LIMIT 1",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i32>(8)?,
                row.get::<_, i32>(9)?,
            ))
        })?;

    #[allow(clippy::type_complexity)] // DB 行映射，10 字段 tuple 最直接
    let rows: Vec<(String, String, String, String, String, String, String, i64, i32, i32)> =
        collect_rows(rows, "load_models_at");

    let mut asr = AsrSection {
        whisper: None,
        sensevoice_orig: None,
        paraformer: None,
        qwen3_asr: None,
        zipformer: None,
        moonshine: None,
        firered: None,
        aliyun: None,
        bytedance: None,
        tencent: None,
        baidu: None,
    };
    for (provider, category, model_name, source, language, description, secret_key, source_type, is_enabled, is_streaming) in rows {
        let entry = ModelEntry {
            source,
            language,
            description,
            secret_key,
            source_type,
            is_enabled: is_enabled != 0,
            is_available: true, // load_models_at 只取 is_available=1 的行
            is_streaming: is_streaming != 0,
        };
        // provider='aliyun' → asr.aliyun；provider='bytedance' → asr.bytedance；
        // provider='tencent' → asr.tencent；provider='baidu' → asr.baidu；
        // 其余按本地 category 映射本地族
        let map: &mut Option<HashMap<String, ModelEntry>> = match (provider.as_str(), category.as_str()) {
            ("aliyun", _) => &mut asr.aliyun,
            ("bytedance", _) => &mut asr.bytedance,
            ("tencent", _) => &mut asr.tencent,
            ("baidu", _) => &mut asr.baidu,
            (_, "whisper") => &mut asr.whisper,
            (_, "sensevoice-orig") => &mut asr.sensevoice_orig,
            (_, "paraformer") => &mut asr.paraformer,
            (_, "qwen3-asr") => &mut asr.qwen3_asr,
            (_, "zipformer") => &mut asr.zipformer,
            (_, "moonshine") => &mut asr.moonshine,
            (_, "firered") => &mut asr.firered,
            _ => continue,
        };
        map.get_or_insert_with(HashMap::new).insert(model_name, entry);
    }
    Ok(AsrConfig { asr })
}

// ── 模型管理页：直读/写 models 表（不过滤 is_enabled）──

/// 模型管理页用的一行本地 ASR 模型（平铺，含 is_enabled）。
///
/// 与 `load_models_at`（过滤 is_enabled=1、按 category 分组、供引擎选择）区分：
/// 本结构**不过滤 is_enabled**，供模型管理页列出「所有可下载模型（含未就绪）」。
#[derive(Debug, Clone)]
pub struct LocalAsrModelRow {
    /// DB 行 id（translate_engine / asr_engine 等配置项按 id 存）。
    pub id: i64,
    pub category: String,
    pub model_name: String,
    pub source: String,
    /// local 模型重载为「文件清单 + sha256」JSON（见 model_commands）；api 模型仍是 API key。
    pub secret_key: String,
    pub description: String,
    pub is_enabled: bool,
    pub is_available: bool,
    pub is_streaming: bool,
    /// 模型来源: 0=builtin 1=local 2=cloud（v48 新增，供前端区分行为）。
    pub source_type: i64,
}

/// 列出全部本地 ASR 模型（domain='asr' AND source_type IN (0,1)，**不过滤 is_enabled**）。
pub fn list_all_local_asr_models() -> Result<Vec<LocalAsrModelRow>> {
    ensure_db()?;
    with_db(list_all_local_asr_models_at)
}

pub(crate) fn list_all_local_asr_models_at(conn: &Connection) -> Result<Vec<LocalAsrModelRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, category, model_name, source, secret_key, description, is_enabled, is_available, is_streaming, source_type
         FROM models WHERE domain='asr' AND source_type IN (0,1)
         ORDER BY source_type ASC, category, model_name",
    )?;
    let rows = stmt.query_map([], local_asr_model_row_mapper)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 按 domain 列出所有本地模型（source_type IN (0,1)），通用版。
/// 用于翻译/OCR 等非 ASR domain 的模型管理。
pub fn list_local_models_by_domain(domain: &str) -> Result<Vec<LocalAsrModelRow>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, category, model_name, source, secret_key, description, is_enabled, is_available, is_streaming, source_type
             FROM models WHERE domain=?1 AND source_type IN (0,1)
             ORDER BY source_type ASC, category, model_name",
        )?;
        let rows = stmt.query_map(params![domain], local_asr_model_row_mapper)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
}

/// 列出所有 builtin 模型（source_type=0，跨 domain）。
///
/// 供 desktop 启动时检测 builtin 模型文件是否缺失（首次启动下载场景）。
/// 返回 LocalAsrModelRow（复用现有行 struct，含 source/model_name/secret_key 等）。
pub fn list_builtin_models() -> Result<Vec<LocalAsrModelRow>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, category, model_name, source, secret_key, description, is_enabled, is_available, is_streaming, source_type
             FROM models WHERE source_type = 0",
        )?;
        let rows = stmt.query_map([], local_asr_model_row_mapper)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
}

/// 写某本地模型的 is_available（文件就绪/可用）。写 DB。
/// 原名 set_model_enabled，2026-07-17 改名为 set_model_available（语义对齐 is_available 列）。
pub fn set_model_available(model_name: &str, enabled: bool) -> Result<()> {
    ensure_db()?;
    with_db(|conn| set_model_available_at(conn, model_name, enabled))
}

pub(crate) fn set_model_available_at(conn: &Connection, model_name: &str, enabled: bool) -> Result<()> {
    if enabled {
        // 置可用——不动 is_enabled（用户需显式 switch_active_model 激活）
        conn.execute(
            "UPDATE models SET is_available = 1 WHERE model_name = ?1 AND source_type IN (0,1) AND domain IN ('asr','translate','ocr')",
            params![model_name],
        )?;
    } else {
        // 置不可用——同步清 is_enabled（不可用模型不能保持激活，防双激活残留）
        conn.execute(
            "UPDATE models SET is_available = 0, is_enabled = 0 WHERE model_name = ?1 AND source_type IN (0,1) AND domain IN ('asr','translate','ocr')",
            params![model_name],
        )?;
    }
    Ok(())
}

/// 写某本地模型的 secret_key（asr/translate/ocr，模型管理页存「文件清单 + sha256」JSON）。写 DB。
pub fn set_model_secret_key(model_name: &str, json: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| set_model_secret_key_at(conn, model_name, json))
}

pub(crate) fn set_model_secret_key_at(conn: &Connection, model_name: &str, json: &str) -> Result<()> {
    conn.execute(
        "UPDATE models SET secret_key = ?1 WHERE model_name = ?2 AND source_type IN (0,1) AND domain IN ('asr','translate','ocr')",
        params![json, model_name],
    )?;
    Ok(())
}

// ── 云端模型 CRUD（用户自建，domain='asr'|'llm' AND source_type=2）──

/// cloud model insert/update 公共字段（DB 层）。
/// 注意：desktop 有同名 `CloudModelInput`（Tauri 命令 DTO），本 struct 是 DB 写入专用。
#[derive(Debug, Clone)]
pub struct CloudModelDbFields<'a> {
    pub provider: &'a str,
    pub category: &'a str,
    pub model_name: &'a str,
    pub source: &'a str,
    pub secret_key: &'a str,
    pub is_streaming: bool,
    pub is_thinking: bool,
}

/// insert_cloud_model 输入——公共字段 + domain（update 通过 id 定位，无 domain）。
pub struct CloudModelDbInput<'a> {
    pub domain: &'a str,
    pub fields: CloudModelDbFields<'a>,
}

/// update_cloud_model 输入——公共字段 + id。
pub struct CloudModelDbUpdate<'a> {
    pub id: i64,
    pub fields: CloudModelDbFields<'a>,
}

/// 新增云端模型。is_available=1（前端已测试通过才保存=可用）；is_enabled=0（不自动激活，
/// 用户在管理页显式激活）。返回新行 id。
pub fn insert_cloud_model(input: &CloudModelDbInput) -> Result<i64> {
    let domain = input.domain;
    let f = &input.fields;
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "INSERT INTO models (domain, provider, category, model_name, source, secret_key, source_type, is_available, is_enabled, is_streaming, is_thinking)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 2, 1, 0, ?7, ?8)",
            params![domain, f.provider, f.category, f.model_name, f.source, f.secret_key,
                    f.is_streaming as i32, f.is_thinking as i32],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

/// 更新云端模型（按 id）。secret_key 为空时不覆盖原值。
pub fn update_cloud_model(update: &CloudModelDbUpdate) -> Result<()> {
    let id = update.id;
    let f = &update.fields;
    ensure_db()?;
    with_db(|conn| {
        if f.secret_key.is_empty() {
            // 不改 secret_key
            conn.execute(
                "UPDATE models SET provider=?1, category=?2, model_name=?3, source=?4,
                 is_streaming=?5, is_thinking=?6 WHERE id=?7 AND source_type=2",
                params![f.provider, f.category, f.model_name, f.source,
                        f.is_streaming as i32, f.is_thinking as i32, id],
            )?;
        } else {
            conn.execute(
                "UPDATE models SET provider=?1, category=?2, model_name=?3, source=?4,
                 secret_key=?5, is_streaming=?6, is_thinking=?7 WHERE id=?8 AND source_type=2",
                params![f.provider, f.category, f.model_name, f.source, f.secret_key,
                        f.is_streaming as i32, f.is_thinking as i32, id],
            )?;
        }
        Ok(())
    })
}

/// 删除云端模型（物理删除，按 id）。
pub fn delete_cloud_model(id: i64) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute("DELETE FROM models WHERE id=?1 AND source_type=2", params![id])?;
        Ok(())
    })
}

/// 按 domain + model_name + provider 查模型 id（用于前端编辑/删除）。
pub fn get_model_id(domain: &str, model_name: &str, provider: &str) -> Result<Option<i64>> {
    ensure_db()?;
    with_db(|conn| {
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM models WHERE domain=?1 AND model_name=?2 AND provider=?3",
                params![domain, model_name, provider],
                |r| r.get(0),
            )
            .ok();
        Ok(id)
    })
}

/// 按 id 查模型的 source 和 secret_key（用于编辑时回填）。
pub fn get_model_source_key(id: i64) -> Result<(String, String)> {
    ensure_db()?;
    with_db(|conn| {
        conn.query_row(
            "SELECT source, secret_key FROM models WHERE id=?1",
            params![id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .map_err(Into::into)
    })
}

/// 按 id 查模型的 is_streaming + is_thinking（用于编辑时回填）。
pub fn get_model_flags(id: i64) -> Result<(bool, bool)> {
    ensure_db()?;
    with_db(|conn| {
        conn.query_row(
            "SELECT is_streaming, is_thinking FROM models WHERE id=?1",
            params![id],
            |r| Ok((r.get::<_, i32>(0)? != 0, r.get::<_, i32>(1)? != 0)),
        )
        .map_err(Into::into)
    })
}

/// 批量查 ASR 域所有模型的 id / model_name / source / secret_key / is_streaming / is_thinking。
/// 替代 N+1 的 get_model_id + get_model_source_key + get_model_flags。
/// Task 2 后补 provider/category 字段（同名不同 provider 的 ASR 模型需精确匹配）+ is_enabled。
pub struct ModelDetailRow {
    pub id: i64,
    pub model_name: String,
    pub provider: String,
    pub category: String,
    pub source: String,
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
    /// DB models.is_enabled（激活态）。供前端标 current（每域仅 1 个=1）。
    pub is_enabled: bool,
}

pub fn list_asr_model_details() -> Result<Vec<ModelDetailRow>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, model_name, provider, category, source, secret_key, is_streaming, is_thinking, is_enabled
             FROM models WHERE domain='asr'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ModelDetailRow {
                id: r.get(0)?,
                model_name: r.get(1)?,
                provider: r.get(2)?,
                category: r.get(3)?,
                source: r.get(4)?,
                secret_key: r.get(5)?,
                is_streaming: r.get::<_, i32>(6)? != 0,
                is_thinking: r.get::<_, i32>(7)? != 0,
                is_enabled: r.get::<_, i32>(8)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    })
}

/// ASR 域全量模型行（管理列表用，不过滤 is_enabled，不分 local/cloud）。
/// 对应 `EngineInfo` 所需字段（name/provider/category/description/source_type）。
/// 与 `load_models`（过滤 is_enabled=1、按 section 分组、供推理缓存）区分：
/// 设置页/工具栏列表直查此函数，新增/编辑/删除后即时反映。
pub struct AsrEngineRow {
    pub model_name: String,
    pub provider: String,
    pub category: String,
    pub description: String,
    pub source_type: i64,
    /// Task 2 后补：DB 行 id（供前端 switch_active_model）。
    pub id: i64,
    pub source: String,
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
    /// DB models.is_enabled（激活态）。
    pub is_enabled: bool,
}

/// 列出 ASR 域所有模型（管理列表用）。不过滤 is_enabled，不分 local/cloud。
pub fn list_all_asr_engines() -> Result<Vec<AsrEngineRow>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT model_name, provider, category, description, source_type,
                    id, source, secret_key, is_streaming, is_thinking, is_enabled
             FROM models WHERE domain='asr'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AsrEngineRow {
                model_name: r.get(0)?,
                provider: r.get(1)?,
                category: r.get(2)?,
                description: r.get(3)?,
                source_type: r.get(4)?,
                id: r.get(5)?,
                source: r.get(6)?,
                secret_key: r.get(7)?,
                is_streaming: r.get::<_, i32>(8)? != 0,
                is_thinking: r.get::<_, i32>(9)? != 0,
                is_enabled: r.get::<_, i32>(10)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    })
}

/// 读取 ASR 云端参考模型列表。
/// 返回 Vec<(provider, category, models_str)>，models_str 为分号分隔。
pub fn list_asr_cloud_presets() -> Result<Vec<(String, String, String)>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT config_key, config_value FROM app_config WHERE category='asr_cloud_model' ORDER BY config_key"
        )?;
        let rows = stmt.query_map([], |r| {
            let key: String = r.get(0)?;
            let value: String = r.get(1)?;
            // key = "provider:category"
            let parts: Vec<&str> = key.splitn(2, ':').collect();
            let provider = parts.first().unwrap_or(&"").to_string();
            let category = parts.get(1).unwrap_or(&"").to_string();
            Ok((provider, category, value))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    })
}

/// 读取 LLM provider 预设 base_url。
/// LLM provider 预设（base_url + 参考模型列表）。
pub struct LlmProviderPresetRow {
    pub provider: String,
    pub base_url: String,
    pub models: Vec<String>,
}

/// 读取 LLM provider 预设。config_value 为 JSON：{"base_url":"...","models":["..."]}。
pub fn list_llm_provider_presets() -> Result<Vec<LlmProviderPresetRow>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT config_key, config_value FROM app_config WHERE category='llm_provider' ORDER BY config_key"
        )?;
        let rows = stmt.query_map([], |r| {
            let provider: String = r.get(0)?;
            let value: String = r.get(1)?;
            // 解析 JSON {"base_url":"...","models":["..."]}
            let parsed: serde_json::Value = serde_json::from_str(&value).unwrap_or_default();
            let base_url = parsed.get("base_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let models: Vec<String> = parsed.get("models")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|m| m.as_str().map(String::from)).collect())
                .unwrap_or_default();
            Ok(LlmProviderPresetRow { provider, base_url, models })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    })
}

///
/// `spec` 支持三种写法（见 [`parse_model_spec`]）：
/// - `"local:name"`：`source_type IN (0,1) AND name`（本地 LLM，如 Ollama）
/// - `"category:name"`：`category AND name` 联合精确查询
/// - `"name"`：仅按 name 查询（向后兼容）
pub fn load_llm_model(spec: &str) -> Result<Option<CompatibleLlmConfig>> {
    ensure_db()?;
    with_db(|conn| load_llm_model_at(conn, spec))
}

pub(crate) fn load_llm_model_at(conn: &Connection, spec: &str) -> Result<Option<CompatibleLlmConfig>> {
    let parsed = parse_model_spec(spec);

    let row = match parsed {
        ModelSpec::Full { provider, category, model_name } => {
            let mut stmt = conn.prepare(
                "SELECT source, secret_key, is_thinking, source_type, is_enabled
                 FROM models
                 WHERE domain='llm' AND provider=?1 AND category=?2 AND model_name=?3 AND is_available = 1",
            )?;
            let mut rows = stmt.query_map(params![provider, category, model_name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i32>(4)?,
                ))
            })?;
            rows.next().transpose()?
        }
        ModelSpec::NameOnly(model_name) => {
            // 裸名兜底：跨 provider/category 搜 name，优先 local（ORDER BY source_type ASC：builtin<local<cloud）
            let mut stmt = conn.prepare(
                "SELECT source, secret_key, is_thinking, source_type, is_enabled
                 FROM models
                 WHERE domain='llm' AND model_name=?1 AND is_available = 1
                 ORDER BY source_type ASC",
            )?;
            let mut rows = stmt.query_map(params![model_name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i32>(4)?,
                ))
            })?;
            rows.next().transpose()?
        }
    };

    let model_name = parsed.model_name();
    Ok(row.map(|(source, secret_key, is_thinking, source_type, is_enabled)| CompatibleLlmConfig {
        // Full 时取解析出的 provider；NameOnly 时为空串（仅日志用）
        provider: match parsed {
            ModelSpec::Full { provider, .. } => provider.to_string(),
            ModelSpec::NameOnly(_) => String::new(),
        },
        model: model_name.to_string(),
        base_url: source,
        secret_key,
        is_thinking: is_thinking != 0,
        source_type,
        is_enabled: is_enabled != 0,
    }))
}

/// models 表的通用行（用于翻译引擎按 id 查询、激活模型查询，不限于 llm domain）。
///
/// 含全字段（含 language/description）——供 [`get_active_model`] 构造完整
/// [`ModelEntry`]（无字段缺失，4 域统一）。比 LocalAsrModelRow 更通用：不限 domain、
/// 不限 source_type。
#[derive(Debug, Clone)]
pub struct ModelRow {
    pub id: i64,
    pub domain: String,
    pub provider: String,
    pub category: String,
    pub model_name: String,
    pub source: String,
    pub secret_key: String,
    pub language: String,
    pub description: String,
    pub source_type: i64,
    pub is_thinking: bool,
    pub is_streaming: bool,
    pub is_enabled: bool,
    pub is_available: bool,
}

/// 按 model_name 查 domain（供 model_commands 按域 reload 缓存）。
pub fn get_model_domain_by_name(model_name: &str) -> Result<Option<String>> {
    ensure_db()?;
    with_db(|conn| {
        let domain: Option<String> = conn.query_row(
            "SELECT domain FROM models WHERE model_name = ?1 LIMIT 1",
            params![model_name],
            |r| r.get(0),
        ).optional()?;
        Ok(domain)
    })
}

/// 按 id 查询 models 表行（不限 domain）。用于反查引擎配置。
pub fn get_model_by_id(id: i64) -> Result<Option<ModelRow>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, domain, provider, category, model_name, source, secret_key,
                    language, description, source_type, is_thinking, is_streaming, is_enabled, is_available
             FROM models WHERE id = ?1",
        )?;
        let row = stmt.query_row(params![id], model_row_mapper).optional()?;
        Ok(row)
    })
}

/// 查询指定域的激活模型（is_enabled=1 且 is_available=1），每域仅一个。
/// 供 load_active_engine(domain) 使用——4 域统一激活查询。
pub fn get_active_model(domain: &str) -> Result<Option<ModelRow>> {
    ensure_db()?;
    with_db(|conn| get_active_model_at(conn, domain))
}

/// 查询指定域的激活模型（is_enabled=1 且 is_available=1），每域仅一个。ORDER BY id 保证确定性。
///
/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。
pub(crate) fn get_active_model_at(conn: &Connection, domain: &str) -> Result<Option<ModelRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, domain, provider, category, model_name, source, secret_key,
                language, description, source_type, is_thinking, is_streaming, is_enabled, is_available
         FROM models WHERE domain=?1 AND is_enabled=1 AND is_available=1 ORDER BY id LIMIT 1",
    )?;
    let row = stmt.query_row(params![domain], model_row_mapper).optional()?;
    Ok(row)
}

/// ModelRow 行映射共享闭包（get_model_by_id / get_active_model 共用，14 列顺序一致）。
fn model_row_mapper(r: &rusqlite::Row<'_>) -> rusqlite::Result<ModelRow> {
    Ok(ModelRow {
        id: r.get(0)?,
        domain: r.get(1)?,
        provider: r.get(2)?,
        category: r.get(3)?,
        model_name: r.get(4)?,
        source: r.get(5)?,
        secret_key: r.get(6)?,
        language: r.get(7)?,
        description: r.get(8)?,
        source_type: r.get(9)?,
        is_thinking: r.get::<_, i64>(10)? != 0,
        is_streaming: r.get::<_, i64>(11)? != 0,
        is_enabled: r.get::<_, i64>(12)? != 0,
        is_available: r.get::<_, i64>(13)? != 0,
    })
}

/// LocalAsrModelRow 行映射共享闭包（list_all_local_asr_models_at /
/// list_local_models_by_domain / list_builtin_models 共用，10 列顺序一致）。
/// 2026-08-05 抽取（问题 2）：消除 3 处生产 + 1 处测试的逐字重复。
fn local_asr_model_row_mapper(r: &rusqlite::Row<'_>) -> rusqlite::Result<LocalAsrModelRow> {
    Ok(LocalAsrModelRow {
        id: r.get(0)?,
        category: r.get(1)?,
        model_name: r.get(2)?,
        source: r.get(3)?,
        secret_key: r.get(4)?,
        description: r.get(5)?,
        is_enabled: r.get::<_, i32>(6)? != 0,
        is_available: r.get::<_, i32>(7)? != 0,
        is_streaming: r.get::<_, i32>(8)? != 0,
        source_type: r.get(9)?,
    })
}

/// LlmModelInfo 行映射共享闭包（list_llm_models_at / list_cloud_models_by_domain_at 共用，
/// 10 列顺序一致）。2026-08-05 抽取（问题 2）。
fn llm_model_info_row_mapper(r: &rusqlite::Row<'_>) -> rusqlite::Result<LlmModelInfo> {
    Ok(LlmModelInfo {
        id: r.get::<_, i64>(0)?,
        provider: r.get::<_, String>(1)?,
        category: r.get::<_, String>(2)?,
        model_name: r.get::<_, String>(3)?,
        source_type: r.get::<_, i64>(4)?,
        source: r.get::<_, String>(5)?,
        secret_key: r.get::<_, String>(6)?,
        is_streaming: r.get::<_, i32>(7)? != 0,
        is_thinking: r.get::<_, i32>(8)? != 0,
        is_enabled: r.get::<_, i32>(9)? != 0,
    })
}

/// 切换激活模型——单语句全量刷新某域的 is_enabled（仅在可用模型中切换）。
/// SQLite 用 IIF（不是 MySQL 的 IF）。每域记录不多（最多几十条），全量刷新无性能问题。
pub fn switch_active_model(domain: &str, id: i64) -> Result<()> {
    ensure_db()?;
    with_db(|conn| switch_active_model_at(conn, domain, id))
}

/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。
///
/// SQL 语义（review fix 双激活 bug）：WHERE 覆盖两类行——
/// (a) 目标行（id=?）且 is_available=1 → 激活它
/// (b) 所有当前 is_enabled=1 的行（无论 is_available） → 清零（含残留的不可用行）
/// 这样不可用模型上残留的 is_enabled=1 也会被清理，防止「文件丢失→重新可用→双激活」。
pub(crate) fn switch_active_model_at(conn: &Connection, domain: &str, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE models SET is_enabled = IIF(id=?1, 1, 0) \
         WHERE domain=?2 AND ((id=?1 AND is_available=1) OR is_enabled=1)",
        params![id, domain],
    )?;
    Ok(())
}

/// 按 spec 精确查 ASR 域某可用模型（不限激活），返回完整 ModelRow。
///
/// 供 CLI `--model` 显式路径 / 多模型场景用——[`get_active_model`] 只返回激活的一个。
/// spec 支持 3-part（`provider:category:model_name`）或裸 model_name：
/// - 3-part：provider + category + model_name 精确匹配
/// - 裸名：仅按 model_name 匹配（取第一条）
pub fn get_asr_model_by_spec(provider: Option<&str>, category: Option<&str>, model_name: &str) -> Result<Option<ModelRow>> {
    ensure_db()?;
    with_db(|conn| get_asr_model_by_spec_at(conn, provider, category, model_name))
}

/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。
pub(crate) fn get_asr_model_by_spec_at(conn: &Connection, provider: Option<&str>, category: Option<&str>, model_name: &str) -> Result<Option<ModelRow>> {
    const SQL_FULL: &str = "SELECT id, domain, provider, category, model_name, source, secret_key,
                    language, description, source_type, is_thinking, is_streaming, is_enabled, is_available
             FROM models WHERE domain='asr' AND is_available=1 AND provider=?1 AND category=?2 AND model_name=?3 LIMIT 1";
    const SQL_NAME: &str = "SELECT id, domain, provider, category, model_name, source, secret_key,
                    language, description, source_type, is_thinking, is_streaming, is_enabled, is_available
             FROM models WHERE domain='asr' AND is_available=1 AND model_name=?1 LIMIT 1";
    let row = match (provider, category) {
        (Some(p), Some(c)) => {
            let mut stmt = conn.prepare(SQL_FULL)?;
            stmt.query_row(params![p, c, model_name], model_row_mapper).optional()?
        }
        _ => {
            let mut stmt = conn.prepare(SQL_NAME)?;
            stmt.query_row(params![model_name], model_row_mapper).optional()?
        }
    };
    Ok(row)
}

/// LLM 模型列表项（菜单用，仅含显示与排序所需字段）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmModelInfo {
    pub id: i64,
    pub model_name: String,
    pub provider: String,
    pub category: String,
    pub source_type: i64,
    pub source: String,
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
    pub is_enabled: bool,
}

/// 列出所有可用的 LLM 润色模型（domain='llm' AND is_available=1），按 source_type 升序（builtin<local<cloud）、category 升序排序。
/// 管理列表用——含未激活（is_enabled=0）的可用模型。is_enabled 字段供前端标 current。
pub(crate) fn list_llm_models_at(conn: &Connection) -> Result<Vec<LlmModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider, category, model_name, source_type, source, secret_key, is_streaming, is_thinking, is_enabled FROM models
         WHERE domain='llm' AND is_available = 1
         ORDER BY source_type ASC, category",
    )?;
    let rows = stmt.query_map([], llm_model_info_row_mapper)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 从 DB 列出启用的 LLM 模型（经 with_db，供 Tauri 命令调用）。
pub fn list_llm_models() -> Result<Vec<LlmModelInfo>> {
    ensure_db()?;
    with_db(list_llm_models_at)
}

/// 云端模型通用列表项（不限 domain，仅 source_type=2 cloud）。供 TranslateTab 等复用 llm 风格的云端 section。
///
/// 与 [`LlmModelInfo`] 字段一致（含 id、provider、category 等），区别在于：
/// - 按 domain 参数过滤（而非写死 'llm'）
/// - 过滤 source_type=2（只列云端模型，本地走 list_local_models_by_domain）
/// - 不过滤 is_enabled（Task 1 后云端模型 insert_cloud_model 写 is_enabled=0 不自动激活；
///   此处保留 is_enabled 字段供前端标 current——用户 switch_active_model 激活后置 1）
pub(crate) fn list_cloud_models_by_domain_at(conn: &Connection, domain: &str) -> Result<Vec<LlmModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider, category, model_name, source_type, source, secret_key, is_streaming, is_thinking, is_enabled
         FROM models WHERE domain = ?1 AND source_type = 2
         ORDER BY category, model_name",
    )?;
    let rows = stmt.query_map(params![domain], llm_model_info_row_mapper)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 从 DB 列出某 domain 的云端模型（source_type=2，经 with_db）。供 Tauri 命令调用。
pub fn list_cloud_models_by_domain(domain: &str) -> Result<Vec<LlmModelInfo>> {
    ensure_db()?;
    with_db(|conn| list_cloud_models_by_domain_at(conn, domain))
}

/// OCR 模型列表项（菜单用，仅含显示字段）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct OcrModelInfo {
    pub model_name: String,
    pub description: String,
    pub source_type: i64,
    /// Task 2 后：DB models.is_enabled（激活态，每域仅 1 个=1）。供前端标 current。
    pub is_enabled: bool,
}

/// 列出所有 OCR 模型（domain='ocr'，含未就绪的——前端列表需展示全部供下载/切换）。
pub(crate) fn list_ocr_models_at(conn: &Connection) -> Result<Vec<OcrModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT model_name, description, source_type, is_enabled FROM models
         WHERE domain='ocr'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(OcrModelInfo {
            model_name: row.get::<_, String>(0)?,
            description: row.get::<_, String>(1)?,
            source_type: row.get::<_, i64>(2)?,
            is_enabled: row.get::<_, i32>(3)? != 0,
        })
    })?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 从 DB 列出启用的 OCR 模型（经 with_db，供 Tauri 命令调用）。
pub fn list_ocr_models() -> Result<Vec<OcrModelInfo>> {
    ensure_db()?;
    with_db(list_ocr_models_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::fill_manifests;
    use crate::db::test_support::{open_init, setup_test_db};

    /// `get_model_by_id` 按 id 反查 models 行——translate_engine 配置链路核心。
    /// seed 后 opus-mt 是 translate domain 的本地模型，先取它的 DB id 再反查，
    /// 不假设自增起始值（避免被前面的 seed 行数变化打穿）。
    #[test]
    fn get_model_by_id_returns_translate_row() {
        setup_test_db();
        // list_local_models_by_domain 现在也返回 id（DB 行 id），直接用，无需再反查。
        let local = list_local_models_by_domain("translate").unwrap();
        let first = local
            .iter()
            .find(|r| r.model_name == "opus-mt")
            .expect("seed 应有 opus-mt 本地翻译模型");
        let id = first.id;
        let got = get_model_by_id(id).unwrap().expect("应查到 id 对应的行");
        assert_eq!(got.id, id);
        assert_eq!(got.domain, "translate");
        assert_eq!(got.model_name, "opus-mt");
        assert_eq!(got.source_type, 1, "本地模型 source_type 应为 1");
        // opus-mt seed 的 provider/category 固定
        assert_eq!(got.provider, "local");
        assert_eq!(got.category, "opus-mt");
        // list_local_models_by_domain 与 get_model_by_id 的 is_enabled 取值一致（都不过滤）
        assert_eq!(got.is_enabled, first.is_enabled);
    }

    /// `get_model_by_id` 查不存在的 id 应返回 None（optional() 路径）。
    #[test]
    fn get_model_by_id_missing_returns_none() {
        setup_test_db();
        let got = get_model_by_id(9_999_999).unwrap();
        assert!(got.is_none(), "不存在的 id 应返回 None");
    }

    /// 回归 Issue #7（code review）：switch_active_model 用 id=-1（LLM「不选择模型」）
    /// 应清空该域所有 is_enabled（前端 LlmTab.tsx 传 -1 表示取消激活）。
    /// 依赖 SQLite AUTOINCREMENT 永不产生负 id（IIF(id=-1,1,0) 无匹配行，全部置 0）。
    #[test]
    fn switch_active_model_with_id_neg1_clears_domain() {
        let conn = open_init();
        // 先激活 sensevoice（is_available=1）
        let sv: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'",
            [], |r| r.get(0)
        ).unwrap();
        switch_active_model_at(&conn, "asr", sv).unwrap();
        assert!(get_active_model_at(&conn, "asr").unwrap().is_some(),
            "测试前提：先激活一个模型");

        // 用 id=-1 调 switch_active_model（前端 LLM「不选择模型」语义）
        switch_active_model_at(&conn, "asr", -1).unwrap();

        // 验证：该域无任何 is_enabled=1 AND is_available=1
        assert!(get_active_model_at(&conn, "asr").unwrap().is_none(),
            "id=-1 应清空该域所有激活（IIF(id=-1,1,0) 无匹配，全置 0）");

        // 原 sensevoice 的 is_enabled 应为 0（被清空）
        let sv_enabled: i64 = conn.query_row(
            "SELECT is_enabled FROM models WHERE id=?1", params![sv], |r| r.get(0)
        ).unwrap();
        assert_eq!(sv_enabled, 0, "原激活模型应被清空");
    }

    #[test]
    fn seed_then_load_round_trips() {
        let conn = open_init();
        // 新语义：is_enabled=激活（每域仅 1），is_available=可用。
        // 激活 zipformer 模型（先置 available，再置 enabled）
        conn.execute("UPDATE models SET is_available = 1 WHERE model_name='zipformer' AND domain='asr'", []).unwrap();
        conn.execute("UPDATE models SET is_enabled = IIF(model_name='zipformer', 1, 0) WHERE domain='asr' AND is_available=1", []).unwrap();
        let cfg = load_models_at(&conn).unwrap();
        // load_models_at 只返回激活的那一个（LIMIT 1）
        let zf = cfg.asr.zipformer.as_ref().expect("zipformer section（激活的）");
        assert_eq!(zf.len(), 1);
        let zp = zf.get("zipformer").unwrap();
        assert_eq!(zp.source, "asr/zipformer");
        assert!(zp.is_local(), "ASR 模型应为本地模型");
        assert!(zp.is_available, "激活模型应 is_available=true");
        assert!(zp.is_streaming, "Zipformer 模型应支持流式");
        // 非激活的 section 不应出现
        assert!(cfg.asr.whisper.is_none(), "whisper 未激活不应出现");
        assert!(cfg.asr.paraformer.is_none(), "paraformer 未激活不应出现");
    }

    #[test]
    fn test_load_llm_model() {
        let conn = open_init();
        // LLM 不再 seed（v31），插入测试数据（is_available=1 表示可用）
        conn.execute_batch(
            "INSERT INTO models (domain, provider, category, model_name, source, description, is_thinking, source_type, is_available)
             VALUES
             ('llm','bigmodel','glm','glm-4-flashx','https://open.bigmodel.cn/api/paas/v4','GLM-4 FlashX',0,2,1),
             ('llm','deepseek','deepseek','deepseek-v4-flash','https://api.deepseek.com/','DeepSeek V4 Flash',1,2,1),
             ('llm','aliyun','deepseek','deepseek-v4-flash','https://dashscope.aliyuncs.com/compatible-mode/v1','DeepSeek via DashScope',1,2,1),
             ('llm','aliyun','qwen','qwen-plus','https://dashscope.aliyuncs.com/compatible-mode/v1','Qwen Plus',0,2,1),
             ('llm','bigmodel','glm','glm-4.5-flash','https://open.bigmodel.cn/api/paas/v4','GLM-4.5 Flash',1,2,1)"
        ).unwrap();

        // 3-part：bigmodel:glm:glm-4-flashx
        let glm = load_llm_model_at(&conn, "bigmodel:glm:glm-4-flashx")
            .unwrap()
            .unwrap();
        assert_eq!(glm.provider, "bigmodel");
        assert_eq!(glm.model, "glm-4-flashx");
        assert_eq!(glm.base_url, "https://open.bigmodel.cn/api/paas/v4");
        assert!(!glm.is_thinking, "glm-4-flashx 不是思考模型");

        // deepseek-v4-flash 在 deepseek 和 aliyun 两个 provider 下同名
        let ds = load_llm_model_at(&conn, "deepseek:deepseek:deepseek-v4-flash")
            .unwrap()
            .unwrap();
        assert_eq!(ds.provider, "deepseek");
        assert!(ds.is_thinking);

        let aliyun = load_llm_model_at(&conn, "aliyun:deepseek:deepseek-v4-flash")
            .unwrap()
            .unwrap();
        assert_eq!(aliyun.provider, "aliyun");
        assert!(aliyun.is_thinking);

        // provider 不匹配时应返回 None
        assert!(
            load_llm_model_at(&conn, "deepseek:qwen:qwen-plus")
                .unwrap()
                .is_none(),
            "deepseek 下不存在 qwen:qwen-plus"
        );

        let glm_think = load_llm_model_at(&conn, "bigmodel:glm:glm-4.5-flash")
            .unwrap()
            .unwrap();
        assert!(glm_think.is_thinking);

        // 裸名（NameOnly）
        let bare = load_llm_model_at(&conn, "glm-4-flashx").unwrap().unwrap();
        assert_eq!(bare.model, "glm-4-flashx");

        assert!(load_llm_model_at(&conn, "nonexistent-model").unwrap().is_none());

        // 插入 source_type=1（local）的 LLM 行，验证精确命中
        conn.execute(
            "INSERT INTO models (domain, provider, category, model_name, source, description, source_type, is_available)
             VALUES ('llm', 'ollama', 'qwen', 'qwen3-8b', 'http://localhost:11434/v1', 'local ollama', 1, 1)",
            [],
        )
        .unwrap();
        let local_llm = load_llm_model_at(&conn, "ollama:qwen:qwen3-8b").unwrap().unwrap();
        assert_eq!(local_llm.provider, "ollama");
        assert_eq!(local_llm.source_type, 1, "本地 LLM source_type 应为 1");
    }

    #[test]
    fn parse_model_spec_variants() {
        // 3-part → Full
        assert_eq!(
            parse_model_spec("bigmodel:glm:glm-4-flashx"),
            ModelSpec::Full { provider: "bigmodel", category: "glm", model_name: "glm-4-flashx" }
        );
        // 裸名 → NameOnly
        assert_eq!(parse_model_spec("bare-name"), ModelSpec::NameOnly("bare-name"));
        // 2-part（旧格式）→ warn + NameOnly 兜底（用整串作为裸名）
        assert_eq!(parse_model_spec("bigmodel:glm-4-flashx"), ModelSpec::NameOnly("bigmodel:glm-4-flashx"));
    }

    #[test]
    fn model_spec_name_strips_prefix() {
        assert_eq!(
            ModelSpec::Full { provider: "p", category: "c", model_name: "foo" }.model_name(),
            "foo"
        );
        assert_eq!(ModelSpec::NameOnly("baz").model_name(), "baz");
    }

    #[test]
    fn test_is_enabled_filtering() {
        let conn = open_init();

        conn.execute("UPDATE models SET is_enabled = 0 WHERE model_name = 'glm-4-flashx'", []).unwrap();
        assert!(load_llm_model_at(&conn, "bigmodel:glm:glm-4-flashx").unwrap().is_none());
        // 裸名也应查不到（唯一匹配的那条被禁用了）
        assert!(load_llm_model_at(&conn, "glm-4-flashx").unwrap().is_none());

        conn.execute("UPDATE models SET is_enabled = 0 WHERE model_name = 'paraformer-streaming'", []).unwrap();
        let cfg = load_models_at(&conn).unwrap();
        assert!(cfg.asr.paraformer.is_none() || !cfg.asr.paraformer.unwrap().contains_key("paraformer-streaming"));
    }

    #[test]
    fn list_all_local_asr_models_includes_disabled() {
        let conn = open_init();
        let rows = list_all_local_asr_models_at(&conn).unwrap();
        // seed 里本地 ASR 全 is_enabled=0，load_models_at 会过滤，本函数应保留
        let names: Vec<&str> = rows.iter().map(|r| r.model_name.as_str()).collect();
        assert!(names.contains(&"paraformer-streaming"), "未过滤 is_enabled=0");
        assert!(rows.iter().any(|r| !r.is_enabled), "应含未就绪模型");
        // v48 后含 1 条 builtin（zipformer-small, source_type=0）+ 13 条 local = 14 条
        assert_eq!(rows.len(), 14, "本地 ASR 清单应含 14 条（13 local + 1 builtin）");
        // builtin 兜底引擎 source 是 'asr/zipformer-small'（与其他 local 模型同 domain/name 格式）
        assert!(names.contains(&"zipformer-small"), "应含 builtin 兜底引擎");
    }

    #[test]
    fn set_model_available_persists() {
        let conn = open_init();
        set_model_available_at(&conn, "paraformer-streaming", true).unwrap();
        let rows = list_all_local_asr_models_at(&conn).unwrap();
        let p = rows.iter().find(|r| r.model_name == "paraformer-streaming").unwrap();
        assert!(p.is_available);
        // 关掉再读
        set_model_available_at(&conn, "paraformer-streaming", false).unwrap();
        let rows = list_all_local_asr_models_at(&conn).unwrap();
        let p = rows.iter().find(|r| r.model_name == "paraformer-streaming").unwrap();
        assert!(!p.is_available);
    }

    #[test]
    fn set_model_secret_key_persists() {
        let conn = open_init();
        let json = r#"{"files":[{"path":"a.onnx","sha256":"abc","size":10}]}"#;
        set_model_secret_key_at(&conn, "paraformer-streaming", json).unwrap();
        let rows = list_all_local_asr_models_at(&conn).unwrap();
        let p = rows.iter().find(|r| r.model_name == "paraformer-streaming").unwrap();
        assert_eq!(p.secret_key, json);
    }

    // ── 模型激活语义（Task 1-2 引入）单测 ──
    // 不变量来源：specs/2026-07-17-model-activation-refactor-design.md §3.3 / §6 / §7

    /// §7 降级路径：无激活模型（is_enabled 全 0）时 get_active_model 返回 None。
    /// seed 默认所有模型 is_enabled=0（用户激活时才设 1），故全新库应返回 None。
    #[test]
    fn get_active_model_returns_none_when_no_active() {
        let conn = open_init();
        // seed 里所有 asr 模型 is_enabled=0
        assert!(get_active_model_at(&conn, "asr").unwrap().is_none());
        assert!(get_active_model_at(&conn, "llm").unwrap().is_none());
        assert!(get_active_model_at(&conn, "ocr").unwrap().is_none());
        assert!(get_active_model_at(&conn, "translate").unwrap().is_none());
    }

    /// §3.3 + §6.1：激活查询 WHERE domain=? AND is_enabled=1 AND is_available=1。
    /// 仅 is_enabled=1 不够——必须 is_available=1（文件未就绪的激活模型不算）。
    #[test]
    fn get_active_model_requires_both_enabled_and_available() {
        let conn = open_init();
        // 找一个 is_available=1 的 ASR 模型（sensevoice-orig-small）并激活
        let row: (i64,) = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'", [], |r| Ok((r.get(0)?,))
        ).unwrap();
        let sid = row.0;
        switch_active_model_at(&conn, "asr", sid).unwrap();

        // 命中：is_enabled=1 AND is_available=1
        let active = get_active_model_at(&conn, "asr").unwrap().expect("应命中激活模型");
        assert_eq!(active.id, sid);
        assert_eq!(active.model_name, "sensevoice-orig-small");
        assert_eq!(active.domain, "asr");
        // §6.1 推理正确性：完整字段（source/secret_key/model_name 与 DB 一致）
        assert!(active.source.starts_with("asr/"));
        assert!(active.is_available);
        assert!(active.is_enabled);

        // 反例：手动设一个 is_enabled=1 AND is_available=0 的行 → 不应命中
        conn.execute(
            "UPDATE models SET is_enabled=1, is_available=0 WHERE model_name='paraformer-streaming'",
            [],
        ).unwrap();
        // 仍应返回 sensevoice（is_available=1 的那个），不返回 paraformer-streaming
        let active2 = get_active_model_at(&conn, "asr").unwrap().expect("仍应命中 sensevoice");
        assert_eq!(active2.model_name, "sensevoice-orig-small");
    }

    /// §6.3 事务性切换：switch_active_model 单 UPDATE 原子刷新——切换后该域仅 1 个 is_enabled=1。
    /// IIF(id=?,1,0) 语义：目标行置 1，其余可用行置 0。
    #[test]
    fn switch_active_model_atomic_single_active_per_domain() {
        let conn = open_init();
        // 两个 is_available=1 的 ASR 模型：sensevoice-orig-small + firered-asr2
        let sv: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'",
            [], |r| r.get(0)
        ).unwrap();
        let fr: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='firered-asr2'",
            [], |r| r.get(0)
        ).unwrap();
        assert_ne!(sv, fr, "测试前提：两模型 id 不同");

        // 先激活 sensevoice
        switch_active_model_at(&conn, "asr", sv).unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM models WHERE domain='asr' AND is_enabled=1 AND is_available=1",
            [], |r| r.get(0)
        ).unwrap();
        assert_eq!(count, 1, "切换后该域应仅 1 个 is_enabled=1 AND is_available=1");
        let active = get_active_model_at(&conn, "asr").unwrap().unwrap();
        assert_eq!(active.id, sv);

        // 切换到 firered——sensevoice 应自动 is_enabled=0
        switch_active_model_at(&conn, "asr", fr).unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM models WHERE domain='asr' AND is_enabled=1 AND is_available=1",
            [], |r| r.get(0)
        ).unwrap();
        assert_eq!(count, 1, "再切换后仍仅 1 个激活");
        let active = get_active_model_at(&conn, "asr").unwrap().unwrap();
        assert_eq!(active.id, fr, "激活应已切到 firered");
    }

    /// §6.3 边界：切到不可用模型时清空该域激活——
    /// SQL WHERE 覆盖 (id=? AND is_available=1) OR is_enabled=1，
    /// 不可用目标行不满足前者 → 不激活；所有 is_enabled=1 行被清零。
    #[test]
    fn switch_active_model_clears_domain_when_target_not_available() {
        let conn = open_init();
        // paraformer-streaming is_available=0（未就绪）
        let ps: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='paraformer-streaming'",
            [], |r| r.get(0)
        ).unwrap();
        // 先激活 sensevoice（is_available=1）确认初始有激活
        let sv: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'",
            [], |r| r.get(0)
        ).unwrap();
        switch_active_model_at(&conn, "asr", sv).unwrap();
        assert!(get_active_model_at(&conn, "asr").unwrap().is_some());

        // 切到 paraformer-streaming（is_available=0）——不满足 (id=? AND is_available=1)
        switch_active_model_at(&conn, "asr", ps).unwrap();
        // sensevoice 在 is_enabled=1 范围内 → 被清零 → 无激活
        assert!(get_active_model_at(&conn, "asr").unwrap().is_none(),
            "切到未就绪模型应清空该域激活");
        // paraformer-streaming 本身 is_enabled 仍 0（不在 WHERE 范围——既不满足激活条件也非 is_enabled=1）
        let ps_enabled: i64 = conn.query_row(
            "SELECT is_enabled FROM models WHERE id=?1", params![ps], |r| r.get(0)
        ).unwrap();
        assert_eq!(ps_enabled, 0, "未就绪模型不应被设为激活");
    }

    /// 回归 review fix 双激活 bug：不可用模型上残留的 is_enabled=1 在 switch 时被清理。
    ///
    /// 触发链：X 激活 → X 文件丢失(a=0) → set_model_available("X",false) 清 e →
    /// switch 到 Y → X 不再 is_enabled=1 → X 恢复 a=1 → 不双激活。
    #[test]
    fn switch_clears_stale_enabled_on_unavailable_model() {
        let conn = open_init();
        let sv: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'",
            [], |r| r.get(0)
        ).unwrap();
        let fr: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='firered-asr2'",
            [], |r| r.get(0)
        ).unwrap();

        // 1. 激活 sensevoice
        switch_active_model_at(&conn, "asr", sv).unwrap();
        // 2. sensevoice 文件丢失 → set_model_available(false) 同步清 is_enabled
        set_model_available_at(&conn, "sensevoice-orig-small", false).unwrap();
        let sv_e: i64 = conn.query_row("SELECT is_enabled FROM models WHERE id=?1", params![sv], |r| r.get(0)).unwrap();
        assert_eq!(sv_e, 0, "set_model_available(false) 应清 is_enabled");
        // 3. 激活 firered
        switch_active_model_at(&conn, "asr", fr).unwrap();
        // 4. sensevoice 文件恢复 → set_model_available(true)
        set_model_available_at(&conn, "sensevoice-orig-small", true).unwrap();
        // 5. 不双激活——sensevoice 的 is_enabled 仍 0
        let sv_e2: i64 = conn.query_row("SELECT is_enabled FROM models WHERE id=?1", params![sv], |r| r.get(0)).unwrap();
        assert_eq!(sv_e2, 0, "恢复可用后不应自动激活");
        // 仅 firered 激活
        let active = get_active_model_at(&conn, "asr").unwrap().unwrap();
        assert_eq!(active.id, fr, "仅 firered 激活");
    }

    /// §6.4 4 域统一：get_active_model / switch_active_model 对 asr/llm/ocr/translate
    /// 4 个 domain 行为一致——同一 API，按 domain 过滤，互不串扰。
    #[test]
    fn switch_active_model_isolates_domains() {
        let conn = open_init();
        // ASR 域激活 sensevoice
        let sv: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'",
            [], |r| r.get(0)
        ).unwrap();
        // OCR 域激活 PP-OCRv6-small（is_available=1）
        let ocr: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='ocr' AND model_name='PP-OCRv6-small'",
            [], |r| r.get(0)
        ).unwrap();

        switch_active_model_at(&conn, "asr", sv).unwrap();
        switch_active_model_at(&conn, "ocr", ocr).unwrap();

        // 4 域各查一次——asr/ocr 命中各自激活，llm/translate 仍 None
        let asr_active = get_active_model_at(&conn, "asr").unwrap().unwrap();
        assert_eq!(asr_active.model_name, "sensevoice-orig-small");
        assert_eq!(asr_active.domain, "asr");

        let ocr_active = get_active_model_at(&conn, "ocr").unwrap().unwrap();
        assert_eq!(ocr_active.model_name, "PP-OCRv6-small");
        assert_eq!(ocr_active.domain, "ocr");

        assert!(get_active_model_at(&conn, "llm").unwrap().is_none());
        assert!(get_active_model_at(&conn, "translate").unwrap().is_none());

        // 域间不串扰：再切 ASR 不影响 OCR
        let fr: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='firered-asr2'",
            [], |r| r.get(0)
        ).unwrap();
        switch_active_model_at(&conn, "asr", fr).unwrap();
        let ocr_still = get_active_model_at(&conn, "ocr").unwrap().unwrap();
        assert_eq!(ocr_still.id, ocr, "切 ASR 不应影响 OCR 激活态");
    }

    /// get_asr_model_by_spec：3-part spec（provider+category+name）精确匹配。
    /// 仅查 is_available=1 的（不限 is_enabled）——CLI 多模型路径专用。
    #[test]
    fn get_asr_model_by_spec_full_3part_matches_available() {
        let conn = open_init();
        // sensevoice-orig-small is_available=1（seed）
        let row = get_asr_model_by_spec_at(&conn, Some("local"), Some("sensevoice-orig"), "sensevoice-orig-small")
            .unwrap().expect("应命中可用模型");
        assert_eq!(row.model_name, "sensevoice-orig-small");
        assert_eq!(row.provider, "local");
        assert_eq!(row.category, "sensevoice-orig");
        assert_eq!(row.domain, "asr");
        assert!(row.is_available);
    }

    /// get_asr_model_by_spec：裸名（provider/category=None）跨 provider/category 匹配。
    #[test]
    fn get_asr_model_by_spec_bare_name_matches() {
        let conn = open_init();
        let row = get_asr_model_by_spec_at(&conn, None, None, "sensevoice-orig-small")
            .unwrap().expect("裸名应命中可用模型");
        assert_eq!(row.model_name, "sensevoice-orig-small");
    }

    /// get_asr_model_by_spec：is_available=0 的模型不返回（文件未就绪不可用）。
    #[test]
    fn get_asr_model_by_spec_filters_unavailable() {
        let conn = open_init();
        // paraformer-streaming is_available=0
        let result = get_asr_model_by_spec_at(&conn, Some("local"), Some("paraformer"), "paraformer-streaming")
            .unwrap();
        assert!(result.is_none(), "未就绪模型不应被查询到");
        let result2 = get_asr_model_by_spec_at(&conn, None, None, "paraformer-streaming")
            .unwrap();
        assert!(result2.is_none(), "裸名查未就绪模型也应返回 None");
    }

    /// get_asr_model_by_spec：非 ASR domain 不命中（函数硬编码 domain='asr'）。
    #[test]
    fn get_asr_model_by_spec_rejects_non_asr_domain() {
        let conn = open_init();
        // PP-OCRv6-small 是 ocr domain，is_available=1，但函数只查 asr
        let result = get_asr_model_by_spec_at(&conn, None, None, "PP-OCRv6-small")
            .unwrap();
        assert!(result.is_none(), "ocr domain 模型不应被 asr 查询命中");
    }

    /// get_asr_model_by_spec：不存在的 name 返回 None（不报错）。
    #[test]
    fn get_asr_model_by_spec_returns_none_for_unknown() {
        let conn = open_init();
        let result = get_asr_model_by_spec_at(&conn, None, None, "nonexistent-model-xxx")
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn list_llm_models_filters_disabled_and_sorts() {
        let conn = open_init();
        // LLM 不再 seed（v31），插入测试数据（is_available=1 表示可用）
        conn.execute_batch(
            "INSERT INTO models (domain, provider, category, model_name, source, description, source_type, is_available)
             VALUES
             ('llm','deepseek','deepseek','deepseek-v4-flash','https://api.deepseek.com/','',2,1),
             ('llm','bigmodel','glm','glm-4-flashx','https://open.bigmodel.cn/api/paas/v4','',2,1),
             ('llm','bigmodel','glm','glm-4.5-flash','https://open.bigmodel.cn/api/paas/v4','',2,1),
             ('llm','aliyun','deepseek','deepseek-v4-flash','https://dashscope.aliyuncs.com/compatible-mode/v1','',2,1),
             ('llm','aliyun','qwen','qwen-plus','https://dashscope.aliyuncs.com/compatible-mode/v1','',2,1),
             ('llm','aliyun','qwen','qwen-turbo','https://dashscope.aliyuncs.com/compatible-mode/v1','',2,1)"
        ).unwrap();
        // 禁用 aliyun provider 下全部 3 条（is_available=0）
        conn.execute(
            "UPDATE models SET is_available = 0 WHERE domain='llm' AND provider='aliyun'",
            [],
        ).unwrap();
        let list = list_llm_models_at(&conn).unwrap();
        // 剩余 3 条 → category 字母序: deepseek, glm, glm
        assert_eq!(list.len(), 3, "aliyun 3 条被禁用应过滤");
        assert_eq!(
            list.iter().map(|m| m.category.as_str()).collect::<Vec<_>>(),
            vec!["deepseek", "glm", "glm"],
            "按 category 字母序"
        );
    }

    #[test]
    fn list_llm_models_at_empty_when_all_disabled() {
        let conn = open_init();
        // seed 无 LLM 数据（用户自建）
        let list = list_llm_models_at(&conn).unwrap();
        assert!(list.is_empty(), "无 LLM 数据时返回空");
    }

    #[test]
    fn list_ocr_models_returns_all() {
        let conn = open_init();
        let list = list_ocr_models_at(&conn).unwrap();
        // seed 2 条 OCR，全部返回（list_ocr_models_at 不过滤 is_available/is_enabled）
        assert_eq!(list.len(), 2, "seed 2 条 OCR，全量返回");
        assert!(list.iter().any(|m| m.model_name == "PP-OCRv6-small"));
        assert!(list.iter().any(|m| m.model_name == "PP-OCRv5"));
    }

    #[test]
    fn list_ocr_models_includes_all_even_disabled() {
        let conn = open_init();
        conn.execute("UPDATE models SET is_available = 0 WHERE domain='ocr'", []).unwrap();
        let list = list_ocr_models_at(&conn).unwrap();
        // 即使全部 is_available=0，仍返回全部（前端需展示供切换）
        assert_eq!(list.len(), 2, "全不可用时仍返回全部 OCR 模型");
    }

    /// fill_manifests 应为 secret_key 为空的 source_type IN (0,1) 模型填充 manifest。
    #[test]
    fn fill_manifests_populates_empty_secret_key() {
        let conn = open_init();
        // INIT_SQL 后 secret_key 全空（seed 不预填 manifest）
        let empty_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM models WHERE source_type IN (0,1) AND (secret_key='' OR secret_key IS NULL)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(empty_count > 0, "seed 后应有 source_type IN (0,1) 且 secret_key 空的行");

        fill_manifests(&conn).unwrap();

        // 验证 ASR 模型有了 manifest
        let sk: String = conn
            .query_row(
                "SELECT secret_key FROM models WHERE model_name='whisper-small' AND domain='asr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!sk.is_empty(), "whisper-small secret_key 应被填充");
        let parsed: serde_json::Value = serde_json::from_str(&sk).unwrap();
        assert!(parsed.as_object().unwrap().contains_key("onnx/encoder_model_int8.onnx"));

        // 验证翻译模型有了 manifest
        let sk: String = conn
            .query_row(
                "SELECT secret_key FROM models WHERE model_name='opus-mt' AND domain='translate'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!sk.is_empty(), "opus-mt secret_key 应被填充");

        // 验证 OCR 模型有了 manifest
        let sk: String = conn
            .query_row(
                "SELECT secret_key FROM models WHERE model_name='PP-OCRv6-small' AND domain='ocr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!sk.is_empty(), "PP-OCRv6-small secret_key 应被填充");
    }

    /// fill_manifests 幂等：已填充的 manifest 不应被覆盖。
    #[test]
    fn fill_manifests_is_idempotent() {
        let conn = open_init();
        fill_manifests(&conn).unwrap();
        let sk1: String = conn
            .query_row(
                "SELECT secret_key FROM models WHERE model_name='whisper-small' AND domain='asr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // 再次调用——不会重写（secret_key 非空，WHERE 条件不匹配）
        fill_manifests(&conn).unwrap();
        let sk2: String = conn
            .query_row(
                "SELECT secret_key FROM models WHERE model_name='whisper-small' AND domain='asr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sk1, sk2, "二次调用不应改变已有 manifest");
    }

    /// list_local_models_by_domain 按 domain 正确过滤。
    #[test]
    fn list_local_models_by_domain_filters_correctly() {
        let conn = open_init();
        fill_manifests(&conn).unwrap();

        let asr_rows = list_all_local_asr_models_at(&conn).unwrap();
        // v48: 含 1 条 builtin（zipformer-small, source='asr/zipformer-small'）+ 13 local
        assert!(asr_rows.iter().all(|r| r.source.starts_with("asr/")),
            "ASR models source 应以 asr/ 开头（builtin + local 统一格式）");
        assert!(asr_rows.iter().any(|r| r.model_name == "zipformer-small"),
            "应含 builtin 兜底引擎");

        // 用新函数查 translate
        let translate_rows: Vec<LocalAsrModelRow> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, category, model_name, source, secret_key, description, is_enabled, is_available, is_streaming, source_type
                     FROM models WHERE domain='translate' AND source_type IN (0,1)",
                )
                .unwrap();
            let rows = stmt.query_map([], local_asr_model_row_mapper).unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        assert_eq!(translate_rows.len(), 2, "应有 2 个翻译模型");
        assert!(
            translate_rows.iter().any(|r| r.model_name == "opus-mt"),
            "应包含 opus-mt"
        );
        assert!(
            translate_rows.iter().any(|r| r.model_name == "m2m100-418M"),
            "应包含 m2m100-418M"
        );
    }

    /// ASR source 应从旧 HF repo 格式更新为 asr/{name} 路径标识。
    #[test]
    fn asr_source_is_path_identifier() {
        let conn = open_init();
        // INIT_SQL 已用新 seed（asr/{name}），验证
        let source: String = conn
            .query_row(
                "SELECT source FROM models WHERE model_name='whisper-small' AND domain='asr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source, "asr/whisper-small");
    }

    /// OCR seed 不再含旧 GitHub MNN URL。
    #[test]
    fn ocr_source_is_path_identifier_not_mnn() {
        let conn = open_init();
        let source: String = conn
            .query_row(
                "SELECT source FROM models WHERE model_name='PP-OCRv6-small' AND domain='ocr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source, "ocr/PP-OCRv6-small");
        assert!(!source.contains("github.com"), "不应再含 GitHub URL");
    }

    /// PP-OCRv5 应在 seed 中。
    #[test]
    fn ocr_v5_in_seed() {
        let conn = open_init();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM models WHERE model_name='PP-OCRv5' AND domain='ocr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "PP-OCRv5 应在 seed 中");
    }

    // ── TDD 防御：OCR 列表不过滤 is_enabled ──

    /// list_ocr_models 返回全部 OCR 模型（含 is_enabled=0 的未就绪模型）。
    #[test]
    fn list_ocr_models_includes_disabled() {
        let conn = open_init();
        // PP-OCRv5 默认 is_enabled=0
        let pp5_enabled: i32 = conn
            .query_row(
                "SELECT is_enabled FROM models WHERE model_name='PP-OCRv5' AND domain='ocr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pp5_enabled, 0, "PP-OCRv5 默认未就绪");

        // list_ocr_models_at 不过滤 is_enabled → 应包含 PP-OCRv5
        let ocrs = list_ocr_models_at(&conn).unwrap();
        assert!(
            ocrs.iter().any(|m| m.model_name == "PP-OCRv5"),
            "list_ocr_models 应包含未就绪的 PP-OCRv5"
        );
        assert!(
            ocrs.iter().any(|m| m.model_name == "PP-OCRv6-small"),
            "list_ocr_models 应包含已就绪的 PP-OCRv6-small"
        );
    }
}
