-- octopus DB 初始化脚本（schema 唯一真相源，2026-07-27 极简化重构）
-- 由 db.rs init_schema 执行：user_version == 0（全新库）时 execute_batch 本脚本。
-- schema 变更：直接改本文件 + 升 db.rs 的 CURRENT_SCHEMA_VERSION 常量。
-- 旧版本库（0 < v < CURRENT_SCHEMA_VERSION）一律 bail，不再支持自动迁移——清库重建。
-- 全部 CREATE TABLE IF NOT EXISTS + INSERT OR IGNORE，幂等。
--
-- 文件结构（2026-07-28 重组）：
--   §A 表结构（CREATE TABLE / INDEX / TRIGGER / VIRTUAL TABLE）—— 前半部分
--   §B 初始化数据（INSERT OR IGNORE seed）—— 后半部分

-- ╔══════════════════════════════════════════════════════════════════════════════╗
-- ║                              §A 表结构（DDL）                                  ║
-- ╚══════════════════════════════════════════════════════════════════════════════╝

-- ── 模型目录（models）─────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS models (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    domain        TEXT    NOT NULL,                       -- 'asr' | 'llm' | 'ocr' | 'translate'
    provider      TEXT    NOT NULL DEFAULT 'local',       -- vendor/运行位置：local/aliyun/deepseek/bigmodel
    category      TEXT    NOT NULL,                       -- ASR 引擎族(zipformer/whisper/Fun-ASR) ; LLM 模型系列(qwen/glm/deepseek)
    model_name    TEXT    NOT NULL,                       -- 具体模型标识，精确匹配
    source        TEXT    NOT NULL,                       -- 本地模型: 路径标识(domain/name) ; 云端: wss 端点 ; LLM: API base URL
    secret_key    TEXT    NOT NULL DEFAULT '',            -- source_type IN (0,1): 下载清单 manifest JSON ; source_type=2: API Key
    language      TEXT    NOT NULL DEFAULT '',
    source_type   INTEGER NOT NULL DEFAULT 1,             -- 模型来源: 0=builtin(内置，开箱即用) 1=local(用户下载) 2=cloud(云端)
    is_thinking   INTEGER NOT NULL DEFAULT 0,             -- LLM 专用：是否为思考（reasoning）模型
    is_streaming  INTEGER NOT NULL DEFAULT 0,             -- 是否支持流式 (0=否, 1=是)
    is_available  INTEGER NOT NULL DEFAULT 0,             -- 可用：文件就绪/配置完整（0=未就绪, 1=就绪），同域可多个
    is_enabled    INTEGER NOT NULL DEFAULT 0,             -- 激活：当前选用的那一个（每域仅 1 个=1）
    description   TEXT    NOT NULL DEFAULT '',            -- 描述
    UNIQUE(domain, provider, category, model_name)        -- domain + provider + category + model_name 作为唯一键
);

-- ── 润色提示词（prompts）───────────────────────────────────────────────────────
-- 用户可维护多条润色 prompt，激活其一（app_config.active_polish_prompt 存 id）。
-- id=1 为系统内置默认（is_system=1，不可编辑/删除）。
CREATE TABLE IF NOT EXISTS prompts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT    NOT NULL,
    category    TEXT    NOT NULL DEFAULT 'voice_text_polish',
    content     TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    is_system   INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- ── 应用配置（app_config）─────────────────────────────────────────────────────
-- config.yaml 的 DB 化：所有应用行为配置（引擎/快捷键/润色/降噪等）以 key-value 存储。
-- 值统一 TEXT，由 Rust 侧 load_app_config 按字段类型解析。
-- category 用于分组：'setting'（用户配置项）/ 'system'（窗口位置等系统状态）。
-- 首次启动由 init_schema 执行 seed；后续 set_config / persist_* 通过 ON CONFLICT DO UPDATE 仅改 config_value，保留 description + category。
CREATE TABLE IF NOT EXISTS app_config (
    category     TEXT NOT NULL DEFAULT 'setting',
    config_key   TEXT PRIMARY KEY,
    config_value TEXT NOT NULL,
    description  TEXT
);

-- ── 剪贴板历史（clipboard_history）─────────────────────────────────────────────
-- 统一存储 text/voice/ocr/image/file，吞并原 transcriptions 表。
-- content + ref_data + meta_info 三层数据模型。
CREATE TABLE IF NOT EXISTS clipboard_history (
    id              INTEGER PRIMARY KEY,       -- 毫秒戳
    item_type       TEXT    NOT NULL,          -- 'text' | 'voice' | 'ocr' | 'image' | 'file'
    content         TEXT    NOT NULL DEFAULT '',  -- voice/ocr/text: 文本全文; image/file: ""
    ref_data        TEXT,                      -- image: blob_hash; file: JSON 路径数组; voice/ocr/text: NULL
    meta_info       TEXT,                      -- JSON 元数据（按 item_type 不同 schema，见 spec §2.3）
    is_favorite     INTEGER NOT NULL DEFAULT 0,
    is_rich         INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL,
    has_thumbnail   INTEGER NOT NULL DEFAULT 0,
    segments        TEXT,                      -- 段 JSON（仅 voice，段模型真相源）
    is_deleted      INTEGER NOT NULL DEFAULT 0 -- 软删标记（0=活跃；1=已进回收站）。与 vault_ciphers/folders 一致。
                                               -- 仅 voice 软删（2026-07-29 重构）：voice 有还原价值，回收站 ≤100 条上限（INV-VT）。
                                               -- text/ocr/image/file 删除即物理 DELETE，is_deleted 对这些类型始终 0。
                                               -- 热词挖掘 list_recent_text 故意不过滤此列——软删内容仍是热词来源（INV-C1）。
);

CREATE INDEX IF NOT EXISTS idx_clip_created   ON clipboard_history(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_clip_type      ON clipboard_history(item_type);
CREATE INDEX IF NOT EXISTS idx_clip_favorite  ON clipboard_history(is_favorite);
CREATE INDEX IF NOT EXISTS idx_clip_ref       ON clipboard_history(ref_data);
CREATE INDEX IF NOT EXISTS idx_clip_deleted   ON clipboard_history(is_deleted) WHERE is_deleted = 1;

-- ── 图片缩略图存储（image_data）─────────────────────────────────────────────
-- 原图改文件系统存储（~/Documents/octopus/screens/<hash>.jpg，2026-07-29），
-- DB 只存缩略图（240×240，几 KB）+ 尺寸元数据，引用计数回收。
CREATE TABLE IF NOT EXISTS image_data (
    hash       TEXT PRIMARY KEY,     -- MD5(RGBA 像素)，去重键 + 文件名
    thumb      BLOB NOT NULL,        -- 缩略图 BLOB（240×240，JPEG q10）
    width      INTEGER NOT NULL,
    height     INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

-- FTS5 全文索引（trigram tokenizer 支持 CJK 子串匹配）
-- 索引 content 列：voice/ocr/text 有文本被索引，image/file content="" 自动跳过
CREATE VIRTUAL TABLE IF NOT EXISTS clipboard_history_fts USING fts5(
    content,
    content='clipboard_history',
    content_rowid='id',
    tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS clip_fts_ai AFTER INSERT ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER IF NOT EXISTS clip_fts_ad AFTER DELETE ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(clipboard_history_fts, rowid, content)
    VALUES('delete', old.id, old.content);
END;
CREATE TRIGGER IF NOT EXISTS clip_fts_au AFTER UPDATE OF content ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(clipboard_history_fts, rowid, content)
    VALUES('delete', old.id, old.content);
    INSERT INTO clipboard_history_fts(rowid, content) VALUES (new.id, new.content);
END;

-- ── Action Bar 菜单项（两级菜单，自引用 parent_id）──────────────────────────
CREATE TABLE IF NOT EXISTS action_bar_items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id   INTEGER DEFAULT NULL,
    title       TEXT NOT NULL,
    icon        TEXT NOT NULL DEFAULT '',
    action_type TEXT NOT NULL,
    action_data TEXT NOT NULL DEFAULT '',
    sort_order  INTEGER NOT NULL DEFAULT 0,
    is_system   INTEGER NOT NULL DEFAULT 1,
    is_enabled  INTEGER NOT NULL DEFAULT 1,
    is_async   INTEGER NOT NULL DEFAULT 1,
    write_output_to_clipboard INTEGER NOT NULL DEFAULT 0,
    shortcut    TEXT NOT NULL DEFAULT '',
    agent       TEXT NOT NULL DEFAULT '',
    accepts     TEXT NOT NULL DEFAULT 'text',
    trigger_keyword TEXT NOT NULL DEFAULT '',
    global_shortcut TEXT NOT NULL DEFAULT '',
    need_voice  INTEGER NOT NULL DEFAULT 0,
    app_bundle_ids TEXT NOT NULL DEFAULT '',   -- JSON 数组 ["com.apple.Safari"]，空=全局项（所有 app 显示）
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (parent_id) REFERENCES action_bar_items(id) ON DELETE CASCADE
);

-- ── 脚本执行记录（script_runs）──────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS script_runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id     INTEGER NOT NULL,
    script_type TEXT NOT NULL,
    exit_code   INTEGER,
    stdout      TEXT NOT NULL DEFAULT '',
    stderr      TEXT NOT NULL DEFAULT '',
    error_msg   TEXT NOT NULL DEFAULT '',
    started_at  TEXT NOT NULL,
    finished_at TEXT,
    duration_ms INTEGER,
    FOREIGN KEY (item_id) REFERENCES action_bar_items(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_script_runs_started_at ON script_runs(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_script_runs_item_id ON script_runs(item_id);

-- ── ASR 热词版本（hotword_sets）多场景词表，多选叠加 ──────────────────────────
-- id 用 TEXT UUID（v46 改造）：支持 git 同步跨设备无冲突，与 vault_ciphers 一致。
-- sync_md5：内容指纹（md5），增量同步 diff 用，由 sync crate 计算填入（不在 infra 算）。
CREATE TABLE IF NOT EXISTS hotword_sets (
    id          TEXT    PRIMARY KEY,             -- UUID v4 字符串（不再自增——支持 git 同步）
    name        TEXT    NOT NULL UNIQUE,
    enabled     INTEGER NOT NULL DEFAULT 1,   -- 0/1 是否勾选生效
    words_text  TEXT    NOT NULL DEFAULT '',
    sync_md5    TEXT,                             -- md5 内容指纹（增量同步 diff，NULL 表示待算）
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- ── ASR 热词全局命中计数（hotword_hits）词级，不绑版本 ────────────────────────
CREATE TABLE IF NOT EXISTS hotword_hits (
    word        TEXT    PRIMARY KEY,
    hit_count   INTEGER NOT NULL DEFAULT 0
);

-- ── Agent Adapter（用户自定义 agent 适配器）──────────────────────────────────
CREATE TABLE IF NOT EXISTS agent_adapters (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    key              TEXT NOT NULL UNIQUE,
    display_name     TEXT NOT NULL,
    detect_binary    TEXT NOT NULL,
    command_template TEXT NOT NULL,
    is_system        INTEGER NOT NULL DEFAULT 0,
    is_default       INTEGER NOT NULL DEFAULT 0,
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

-- ── Agent Task（agent × 语音识别联动）──────────────────────────────────────
CREATE TABLE IF NOT EXISTS agent_tasks (
    id               TEXT PRIMARY KEY,
    status           TEXT NOT NULL DEFAULT 'pending',
    agent_key        TEXT NOT NULL,
    context          TEXT NOT NULL DEFAULT '{}',
    transcribed_text TEXT NOT NULL DEFAULT '',
    error_msg        TEXT NOT NULL DEFAULT '',
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

-- ── 启动器索引（launcher_index）统一 app + command 缓存 ──────────────────────
-- type='app'：文件系统扫描的应用；type='command'：brew/cargo/system 等命令。
-- PRIMARY KEY (type, path) 既是去重键也是按 (type, path) 单点更新 keywords 的索引。
CREATE TABLE IF NOT EXISTS launcher_index (
    type        TEXT NOT NULL,               -- 'app' | 'command'
    name        TEXT NOT NULL,               -- app: file_stem（如 WeChat）; command: 命令名
    path        TEXT NOT NULL,               -- app: .app 绝对路径; command: 可执行路径/标识
    alias       TEXT NOT NULL DEFAULT '',     -- app 的本地化名（如 微信），command 无
    icon        TEXT NOT NULL DEFAULT '',     -- app 的 base64 PNG 图标（32×32），command 无
    source      TEXT NOT NULL DEFAULT '',     -- command 的来源（brew/cargo/system），app 用 'applications'
    description TEXT NOT NULL DEFAULT '',     -- 英文描述（command 用）
    keywords    TEXT NOT NULL DEFAULT '',     -- LLM 生成的中英文关键字（搜索增强用）
    bundle_id   TEXT NOT NULL DEFAULT '',     -- app 的 CFBundleIdentifier（app-aware 绑定 key），command 无
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (type, path)
);
CREATE INDEX IF NOT EXISTS idx_launcher_name  ON launcher_index(name);
CREATE INDEX IF NOT EXISTS idx_launcher_alias ON launcher_index(alias);

-- ── 搜索频次加权（search_frequency）按命中次数加权搜索结果排序 ────────────────
CREATE TABLE IF NOT EXISTS search_frequency (
    score_key   TEXT    NOT NULL,           -- 打分维度键（主键）
    query       TEXT    NOT NULL DEFAULT '', -- 最近一次命中的查询原文
    hit_count   INTEGER NOT NULL DEFAULT 0,  -- 累计命中次数
    last_hit_ts INTEGER NOT NULL DEFAULT 0,  -- 最近命中 Unix 秒
    PRIMARY KEY (score_key)
);

-- ============================================================================
-- Password Vault（schema v38，2026-07-18 新增）
-- ============================================================================

-- vault 元数据：单行（CHECK id=1）。
-- KDF 参数 + 双层密钥的"保护壳"（master_root_key / K_machine 双密文 app_key）。
CREATE TABLE IF NOT EXISTS vault_meta (
    id                          INTEGER PRIMARY KEY CHECK (id = 1),
    kdf_type                    INTEGER NOT NULL,            -- 0=Argon2id（MVP 仅支持 0）
    kdf_salt                    BLOB NOT NULL,               -- 32 字节随机盐
    kdf_iterations              INTEGER NOT NULL,            -- Argon2id: t (默认 3)
    kdf_memory_kib              INTEGER NOT NULL,            -- Argon2id: m (默认 65536 = 64 MiB)
    kdf_parallelism             INTEGER NOT NULL,            -- Argon2id: p (默认 4)
    protected_user_vault_key    TEXT NOT NULL,               -- v1:base64(...)，被 master_root_key 加密
    app_key_local_enc           TEXT NOT NULL,               -- 被 K_machine 加密（本机无感启动）
    app_key_sync_enc            TEXT NOT NULL,               -- 被 master_root_key 加密（跨机同步）
    security_stamp              TEXT NOT NULL,               -- 改主密码 / 改 KDF 时刷新（UUID v4）
    equivalent_domains          TEXT NOT NULL DEFAULT '[]',  -- JSON 数组的数组
    public_key                  TEXT,
    protected_private_key       TEXT,
    created_at                  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at                  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- vault 密码条目。所有敏感字段（name/notes/data/fields/password_history）均为密文 v1:base64(...)。
-- id 用 UUID v4 字符串（2026-07-21 v39：从 INTEGER AUTOINCREMENT 改 TEXT，支持 git 同步跨设备无冲突）。
CREATE TABLE IF NOT EXISTS vault_ciphers (
    id                  TEXT PRIMARY KEY,                -- UUID v4 字符串（不再自增——支持 git 同步）
    folder_id           TEXT DEFAULT NULL,               -- FK vault_folders(id)，UUID 字符串
    atype               INTEGER NOT NULL,                -- 1=Login（MVP 仅此）
    name                TEXT NOT NULL,                   -- 密文 v1:base64(...)
    notes               TEXT DEFAULT NULL,               -- 密文
    data                TEXT NOT NULL,                   -- 密文 JSON（uris/username/password/totp）
    fields              TEXT DEFAULT NULL,               -- 密文 JSON（自定义字段）
    password_history    TEXT DEFAULT NULL,               -- 密文 JSON（密码历史）
    reprompt            INTEGER NOT NULL DEFAULT 0,      -- 0=None 1=Password
    favorite            INTEGER NOT NULL DEFAULT 0,
    sync_md5            TEXT,                            -- md5 内容指纹（增量同步 diff，详见 vault::sync::fingerprint）
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
    is_deleted          INTEGER NOT NULL DEFAULT 0,      -- 软删除（0=活跃，1=回收站）
    FOREIGN KEY (folder_id) REFERENCES vault_folders(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_vault_ciphers_favorite
    ON vault_ciphers(favorite) WHERE is_deleted = 0;

-- vault 文件夹（id UUID 字符串，与 vault_ciphers 一致）。
CREATE TABLE IF NOT EXISTS vault_folders (
    id          TEXT PRIMARY KEY,                -- UUID v4 字符串
    name        TEXT NOT NULL,                    -- 密文 v1:base64(...)
    sort_order  INTEGER NOT NULL DEFAULT 0,
    sync_md5    TEXT,                             -- md5 内容指纹（增量同步 diff）
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    is_deleted  INTEGER NOT NULL DEFAULT 0      -- 软删除（0=活跃，1=回收站）
);

CREATE INDEX IF NOT EXISTS idx_vault_folders_active
    ON vault_folders(sort_order) WHERE is_deleted = 0;

-- ══ 录屏元数据（recordings / recordings_thumbnails，schema v51 + v52 audio_tracks）═══
CREATE TABLE IF NOT EXISTS recordings (
    id                INTEGER PRIMARY KEY,
    file_path         TEXT    NOT NULL,
    title             TEXT    NOT NULL DEFAULT '',
    duration_ms       INTEGER NOT NULL,
    width             INTEGER NOT NULL,
    height            INTEGER NOT NULL,
    fps               INTEGER NOT NULL,
    codec             TEXT    NOT NULL,
    has_system_audio  INTEGER NOT NULL DEFAULT 0,
    has_microphone    INTEGER NOT NULL DEFAULT 0,
    -- schema v52：JSON 序列化的 AudioTrack[]（spec 2026-07-27-screen-record-audio-post-merge.md）。
    -- 双轨保留 + 录后合并方案——存原始多轨元数据，合并时由 ffmpeg amix 处理。
    audio_tracks      TEXT    NOT NULL DEFAULT '[]',
    source_type       TEXT    NOT NULL,
    file_size         INTEGER NOT NULL,
    has_thumbnail     INTEGER NOT NULL DEFAULT 0,
    is_favorite       INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_rec_created   ON recordings(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_rec_favorite  ON recordings(is_favorite);
CREATE INDEX IF NOT EXISTS idx_rec_source    ON recordings(source_type);

CREATE TABLE IF NOT EXISTS recordings_thumbnails (
    recording_id INTEGER PRIMARY KEY,
    blob         BLOB NOT NULL,
    width        INTEGER NOT NULL,
    height       INTEGER NOT NULL,
    created_at   TEXT NOT NULL,
    FOREIGN KEY (recording_id) REFERENCES recordings(id) ON DELETE CASCADE
);


-- ╔══════════════════════════════════════════════════════════════════════════════╗
-- ║                          §B 初始化数据（seed）                                 ║
-- ║          全部 INSERT OR IGNORE，幂等；不破坏已有数据                            ║
-- ╚══════════════════════════════════════════════════════════════════════════════╝

-- ─── 模型 seed（models 表）───────────────────────────────────────────────────
-- is_available 表「文件就绪」：seed 初始大部分未就绪（is_available=0），用户下载后置 true。
-- is_enabled 表「激活」：seed 全 0，用户在管理页激活某模型时 switch_active_model 置 1（每域仅 1 个）。
-- 默认/兜底引擎 zipformer-small-ctc（source_type=0 builtin）Step 3 由 db.sql seed + fill_manifests 管理。
-- 本地 ASR 模型（source_type=1，应用限定的开发适配清单，只读；下载/就绪由模型管理页管理）
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, language, description, source_type, is_available, is_streaming)
VALUES
    ('asr','local','moonshine','moonshine-base-en','asr/moonshine-base-en','en','Moonshine Base EN (274M)',1,0,0),
    ('asr','local','moonshine','moonshine-tiny-en','asr/moonshine-tiny-en','en','Moonshine Tiny EN (119M)',1,0,0),
    ('asr','local','paraformer','paraformer-bilingual','asr/paraformer-bilingual','auto','paraformer中英版, 230M',1,0,1),
    ('asr','local','paraformer','paraformer-multi-zh','asr/paraformer-multi-zh','auto','paraformer方言+英语, 230M',1,0,1),
    ('asr','local','paraformer','paraformer-streaming','asr/paraformer-streaming','zh','paraformer-streaming, 230M',1,0,1),
    ('asr','local','paraformer','paraformer-zh','asr/paraformer-zh','zh','paraformer普通话版, 230M',1,0,1),
    ('asr','local','qwen3-asr','qwen3-asr-0.6B','asr/qwen3-asr-0.6B','auto','qwen3-asr-0.6B, 954M',1,0,0),
    ('asr','local','qwen3-asr','qwen3-asr-1.7B','asr/qwen3-asr-1.7B','auto','qwen3-asr-1.7B, 2.2G',1,0,0),
    ('asr','local','sensevoice-orig','sensevoice-orig-small','asr/sensevoice-orig-small','auto','原版 SenseVoice-Small quant (230M，FunASR 4输入，中/英/粤/日/韩)',1,1,0),
    ('asr','local','firered','firered-asr2','asr/firered-asr2','auto','FireRedASR2-AED CTC int8 (740M，中文+20方言+英)',1,1,0),
    ('asr','local','whisper','whisper-small','asr/whisper-small','en','whisper-small.en, 372M',1,0,0),
    ('asr','local','zipformer','zipformer','asr/zipformer','zh','zipformer, 160M',1,0,1),
    ('asr','local','zipformer','zipformer-large','asr/zipformer-large','zh','zipformer-large, 736M',1,0,1),
    -- builtin 兜底引擎（source_type=0，27M，首次启动下载；spec 2026-07-22-builtin-models.md §1.3）
    ('asr','local','zipformer','zipformer-small','asr/zipformer-small','zh','zipformer-small 兜底引擎（27M，内置，首次启动下载）',0,0,1);

-- OCR 模型（domain='ocr'）
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, language, description, source_type, is_available, is_streaming)
VALUES
    ('ocr','local','paddleocr','PP-OCRv6-small','ocr/PP-OCRv6-small','auto','PP-OCRv6 small (det 9.7M + rec 21.5M + keys 73K)，中/英/繁体/日',1,1,0),
    ('ocr','local','paddleocr','PP-OCRv5','ocr/PP-OCRv5','auto','PP-OCRv5 mobile (det 4.5M + rec 16M + keys 92K)，中/英/繁体/日',1,0,0);

-- 翻译模型（domain='translate'）
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, language, description, source_type, is_available, is_streaming)
VALUES
    ('translate','local','opus-mt','opus-mt','translate/opus-mt','auto','opus-mt 中英互译（轻量快速，~500M）',1,0,0),
    ('translate','local','m2m100','m2m100-418M','translate/m2m100-418M','auto','m2m100 多语言翻译（100+ 语言互译，~600M）',1,0,0);

-- 云端模型（source_type=2）不再 seed，由用户自行添加。
-- 参考模型列表存 app_config（category='asr_cloud_model' / 'llm_provider'），见下方 app_config seed。

-- prompts 表 seed 已外置到 crates/infra/seeds/prompts/，由 seeds::load_prompt_seeds
-- 在 load_external_seeds 时一次性加载（INSERT OR IGNORE，保护用户编辑）。
-- 文件清单：default-polish.md（id=1 默认润色）/ advanced-polish.md（id=2 进阶润色）。
-- llm_provider seed 已外置到 crates/infra/seeds/llm_providers.json，由
-- seeds::load_llm_providers_seed 一次性加载（7 个 provider）。

-- ─── 应用配置 seed（app_config 表）───────────────────────────────────────────
-- 通用设置（engine / 识别 / 润色 / 降噪等，不含快捷键——快捷键见下方独立 seed 块）
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
    ('engine_mode',              'embedded',                             'ASR 引擎模式: embedded | websocket | grpc'),
    ('remote_url',               'ws://127.0.0.1:3000/ws/stream',        'WebSocket 远程地址（engine_mode=websocket 时使用）'),
    ('grpc_endpoint',            'http://127.0.0.1:50051',               'gRPC 端点（engine_mode=grpc 时使用）'),
    ('language',                 'auto',                                 '识别语言: auto | zh | en | ja | ko'),
    ('paste_method',             'clipboard',                            '粘贴方式: clipboard | direct | none'),
    ('write_to_clipboard',       'true',                                 '粘贴后是否把结果写入剪贴板'),
    ('switch_input_source_on_paste', 'true',                             '粘贴前切换到英文输入源（避免中文输入法干扰，仅 macOS）'),
    ('microphone',               '',                                     '麦克风名称（空=系统默认）'),
    ('overlay_position',         'top',                                  'overlay 位置: top | bottom | none'),
    ('segment_silence',          '400',                                  'VAD 静音触发识别阈值（毫秒）'),
    ('polish_mode',              '0',                                    '润色模式: 0=关闭 / 1=仅最终 / 2=中间+最终'),
    ('polish_min_interval',      '5',                                    '中间润色最小间隔（秒，节流用）'),
    ('pause_polish_threshold_ms','600',                                  '停顿驱动中间润色的静音阈值（毫秒，必须 > 500）'),
    ('asr_hardware_accelerated', 'false',                                '是否使用 ASR 硬件加速'),
    ('asr_correct',              'false',                                '是否对 ASR 输出进行纠错'),
    ('output_simplified',        'true',                                 'ASR 输出字形: true=简体 / false=繁体'),
    ('hide_toolbar',             'false',                                '结果展示区工具栏是否自动隐藏'),
    ('denoise_mode',             '1',                                    '降噪模式: 0=无 / 1=轻度 / 2=深度'),
    ('download_mirror',          '',                                     'HF 模型下载镜像 host（如 https://hf-mirror.com），空=官方源 huggingface.co'),
    ('active_polish_prompt',     '1',                                    '激活的润色 prompt id（prompts 表 id 字段）');

-- 全局快捷键 seed（统一管理，按功能分组；与 config.rs 的 default_*_shortcut 函数保持一致）
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
    -- 识别 / 结果窗
    ('asr_shortcut',           'Alt+A',            '全局 ASR 激活/关闭快捷键'),
    ('edit_shortcut',          'CmdOrCtrl+Enter',  '结果窗编辑 toggle 快捷键（进入/保存同键）'),
    ('edit_global_shortcut',   'Alt+E',            '全局编辑结果窗快捷键（跨应用唤起窗口+进入/保存编辑）'),
    ('polish_global_shortcut', 'Alt+S',            '全局立即润色快捷键（跨应用 show 结果窗不聚焦 + 触发 polish_now）'),
    -- 剪贴板
    ('clipboard_shortcut',     'Alt+C',            '剪贴板历史窗口快捷键'),
    -- 操作面板（AI 命令面板）
    ('action_bar_shortcut',    'Alt+D',            'AI 命令面板快捷键'),
    -- 截图
    ('screenshot_shortcut',    'CmdOrCtrl+Shift+X','截图快捷键'),
    -- 录屏
    ('record_shortcut',        'CmdOrCtrl+Shift+R','录屏快捷键（呼出/暂停-恢复 toggle）'),
    -- 停止录屏固定为 Escape（record_hotkey.rs 的 STOP_SHORTCUT 常量），不暴露为配置项
    -- 密码箱（vault）
    ('vault_autotype_shortcut',    'CmdOrCtrl+Shift+S', '密码箱 Auto-Type 全局快捷键'),
    ('vault_generator_shortcut',   'CmdOrCtrl+Shift+G', '密码箱密码生成器快捷键');

-- 剪贴板其他配置项（非快捷键）
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
    ('clipboard_enabled',          'true',  '是否启用剪贴板历史监听'),
    ('clipboard_theme',            'light', 'UI 主题 id'),
    ('clipboard_max_items',        '1000',  '最大保留条数（不含收藏）'),
    ('clipboard_max_age_days',     '30',    '自动清理天数（不含收藏）'),
    ('action_bar_search_engine',   'google', 'AI 命令面板搜索引擎');

-- 密码箱其他配置项（非快捷键）
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
    ('vault_lock_timeout_secs',    '180',               '密码箱锁定超时（秒，焦点失活后）');

-- 录屏其他配置项（非快捷键）
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
    ('record_fps',               '30',                '录屏帧率（15/30/60）'),
    ('record_codec',             'h264',              '录屏编码（h264/hevc）'),
    ('record_resolution',        'original',          '录屏输出分辨率（original/1080p/720p）'),
    ('record_system_audio',      'true',              '默认是否录制系统音频'),
    ('record_microphone',        'false',             '默认是否录制麦克风（false=首启不申请麦克风权限）。注意：false 不代表 MVP 不支持麦克风，只是默认不开启'),
    ('record_microphone_device', '',                  '麦克风设备名（空=系统默认）'),
    ('record_hide_cursor',       'false',             '是否隐藏系统光标（P3 用）'),
    ('record_default_source_type', 'display',         '默认录制源类型'),
    ('record_output_dir',        '',                  '录屏保存目录（绝对路径，支持 ~/ 展开；空=默认 ~/Documents/octopus/recordings/）'),
    ('screen_output_dir',        '',                  '截图保存目录（绝对路径，支持 ~/ 展开；空=默认 ~/Documents/octopus/screens/）'),
    ('record_history_view',      'grid',              '历史列表默认视图（grid/list）'),
    ('record_reveal_after_stop', 'true',              '录屏停止后是否自动在 Finder 高亮文件');

-- 环境变量（category='env'）——模型下载地址模板替换
INSERT OR IGNORE INTO app_config (config_key, config_value, description, category) VALUES
    ('huggingface', 'https://hf-mirror.com', 'HuggingFace 下载镜像地址', 'env'),
    ('modelscope',  'https://modelscope.cn',  '魔搭社区下载镜像地址',   'env'),
    ('github',      'https://github.com',     'GitHub 下载地址',         'env');

-- 云端 ASR 模型参考列表（category='asr_cloud_model'）
INSERT OR IGNORE INTO app_config (config_key, config_value, description, category) VALUES
    ('aliyun:Fun-ASR', 'fun-asr-realtime;fun-asr-realtime-2026-02-28;fun-asr-realtime-2025-11-07;fun-asr-flash-8k-realtime;fun-asr-flash-8k-realtime-2026-01-28', '阿里云 FunASR 实时模型列表', 'asr_cloud_model'),
    ('aliyun:Paraformer', 'paraformer-realtime-v1;paraformer-realtime-v2;paraformer-realtime-8k-v1;paraformer-realtime-8k-v2', '阿里云 Paraformer 实时模型列表', 'asr_cloud_model'),
    ('aliyun:Qwen-ASR', 'qwen3-asr-flash-realtime;qwen3-asr-flash-realtime-2026-02-10;qwen3-asr-flash-realtime-2025-10-27', '阿里云 Qwen3-ASR Realtime 模型列表', 'asr_cloud_model'),
    ('bytedance:Doubao-ASR', 'doubao-asr-1.0-streaming', '火山引擎豆包 ASR 1.0', 'asr_cloud_model'),
    ('bytedance:Doubao-ASR-2.0', 'doubao-asr-2.0-streaming;seedasr-2.0-streaming', '火山引擎豆包 ASR 2.0', 'asr_cloud_model'),
    ('tencent:Tencent-ASR', '16k_zh;16k_zh_large;16k_zh-PY;16k_zh-TW;16k_yue;16k_zh_dialect;16k_wuu-SH', '腾讯云实时语音识别中文引擎', 'asr_cloud_model'),
    ('tencent:Tencent-ASR-Multi', '16k_zh_en;16k_multi_lang;16k_en;16k_en_large', '腾讯云实时语音识别多语种引擎', 'asr_cloud_model'),
    ('baidu:Baidu-ASR', '15372;15376;1537', '百度实时语音识别中文模型（dev_pid）', 'asr_cloud_model'),
    ('baidu:Baidu-ASR-EN', '17372;1737', '百度实时语音识别英文模型（dev_pid）', 'asr_cloud_model');

-- ─── Action Bar 菜单项 seed（两级菜单，自引用 parent_id）─────────────────────
-- 主菜单项
INSERT OR IGNORE INTO action_bar_items (id, parent_id, title, icon, action_type, action_data, sort_order, is_system, accepts) VALUES
    (1, NULL, 'AI',    'sparkles', 'submenu', '', 0, 1, 'any'),
    (2, NULL, '翻译',  'globe',    'ai', 'auto_translate', 1, 1, 'text'),
    (3, NULL, '搜索',  'search',   'submenu', '', 2, 1, 'any'),
    (4, NULL, '网页',  'link',     'url', '', 3, 1, 'text');

-- AI 子菜单（parent_id=1）
INSERT OR IGNORE INTO action_bar_items (id, parent_id, title, icon, action_type, action_data, sort_order, is_system) VALUES
    (5, 1, '润色', 'pencil',    'ai', '请对以下文本进行润色，使其更加流畅、专业。保持原意不变。只输出润色结果。', 0, 1),
    (6, 1, '摘要', 'file-text', 'ai', '请用简洁的中文总结以下内容的要点，不超过 3 句话。只输出总结。', 1, 1),
    (7, 1, '解释', 'lightbulb', 'ai', '请用简洁的中文解释以下内容的含义。只输出解释。', 2, 1);

-- 搜索子菜单（parent_id=3）
INSERT OR IGNORE INTO action_bar_items (id, parent_id, title, icon, action_type, action_data, sort_order, is_system) VALUES
    (8, 3, 'Google', 'search', 'url', 'https://www.google.com/search?q={text}', 0, 1),
    (9, 3, '百度',   'search', 'url', 'https://www.baidu.com/s?wd={text}', 1, 1),
    (10, 3, 'Bing',  'search', 'url', 'https://www.bing.com/search?q={text}', 2, 1),
     (11, 3, 'Github',  'search', 'url', 'https://github.com/search?type=repositories&q={text}', 3, 1);

-- 「问豆包」（用 title 去重，不固定 id 避免与用户自建项冲突；放在固定 id seed 之后）
INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system)
SELECT NULL, '问豆包', 'sparkles', 'script', '#osascript
set the clipboard to (do shell script ("printf %s " & quoted form of (system attribute "OCTOPUS_TEXT")))
do shell script "open -a Doubao"
delay 2
tell application "System Events"
    tell process "Doubao"
        keystroke "v" using command down
        delay 0.3
        key code 36
    end tell
end tell', 4, 1
WHERE NOT EXISTS (SELECT 1 FROM action_bar_items WHERE title='问豆包' AND parent_id IS NULL);

-- ─── ASR 热词 seed（hotword_sets）────────────────────────────────────────────
-- 默认「通用」版本：固定 UUID（跨设备一致——两台机器的「通用」集 sync 时 id 相同不冲突）。
INSERT OR IGNORE INTO hotword_sets(id, name, enabled, words_text, sync_md5)
VALUES('00000000-0000-0000-0000-000000000001', '通用', 1, '', NULL);

-- ─── Agent Adapter seed（agent_adapters）─────────────────────────────────────
-- 内置 agent（is_system=1，用户不可删除，仅可改 is_default）
-- is_default 由代码层保证唯一（set_default_agent 时先把全部置 0 再置目标为 1）
INSERT OR IGNORE INTO agent_adapters (key, display_name, detect_binary, command_template, is_system, is_default) VALUES
    ('claude', 'Claude Code', 'claude', 'claude --add-dir {cwd} {prompt}', 1, 0),
    ('pi',     'Pi',          'pi',     'pi {files_at} {prompt}',           1, 1);  -- Pi 默认（PPT 菜单等场景的兜底）
