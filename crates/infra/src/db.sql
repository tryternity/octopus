-- octopus DB 初始化脚本
-- 首次启动（user_version=0）时由 init_schema 执行一次，之后不再重复执行。
-- 开发阶段无需迁移逻辑：调整 schema 时直接删除 ~/.octopus/octopus.db 重新初始化。

-- ── 表结构 ──────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS transcriptions (
    id            INTEGER PRIMARY KEY,   -- 应用写入的毫秒戳，非 AUTOINCREMENT
    created_at    TEXT    NOT NULL,
    engine        TEXT    NOT NULL,
    engine_mode   TEXT,
    raw_text      TEXT    NOT NULL,
    polished_text TEXT,
    polish_status TEXT    NOT NULL DEFAULT 'off',
    polish_model  TEXT,
    duration_ms   INTEGER,
    char_count    INTEGER
);

CREATE INDEX IF NOT EXISTS idx_trans_created ON transcriptions(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_trans_engine  ON transcriptions(engine);

CREATE TABLE IF NOT EXISTS models (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    domain      TEXT    NOT NULL,           -- 'asr' | 'llm'
    category    TEXT    NOT NULL,           -- ASR: 'zipformer'/'whisper'/... ; LLM: 'deepseek'/'bigmodel'/...
    name        TEXT    NOT NULL,           -- 唯一模型标识，精确匹配
    source      TEXT    NOT NULL,           -- ASR: 本地相对路径或 HF repo ; LLM: API base URL
    language    TEXT    NOT NULL DEFAULT '',
    description TEXT    NOT NULL DEFAULT '',
    secret_key  TEXT    NOT NULL DEFAULT '', -- LLM API Key（本地模型留空）
    is_thinking INTEGER NOT NULL DEFAULT 0,  -- LLM 专用：是否为思考（reasoning）模型
    is_local    INTEGER NOT NULL DEFAULT 0,  -- 是否为本地模型 (0=否, 1=是)
    is_enabled  INTEGER NOT NULL DEFAULT 1,  -- 是否启用 (0=禁用, 1=启用)
    UNIQUE(domain, name, is_local)           -- domain + name + is_local 作为唯一键
);

-- ── 默认数据（INSERT OR IGNORE，幂等）────────────────────────────────────────

-- ASR 引擎：激活由 config.yaml.asr_engine 控制，此处仅维护可选列表
INSERT OR IGNORE INTO models (domain, category, name, source, language, description, is_local, is_enabled)
VALUES
    ('asr', 'zipformer', 'zipformer-small-ctc',
        'models/zipformer', 'zh', 'zipformer-small-ctc, 27M（随应用打包，兜底引擎）', 1, 1),

    ('asr', 'zipformer', 'zipformer-multi',
        'k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13',
        'zh', 'zipformer-multi, 80M', 1, 0),

    ('asr', 'zipformer', 'zipformer-ctc',
        'csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30',
        'zh', 'zipformer-ctc, 163M', 1, 0),

    ('asr', 'paraformer', 'paraformer-streaming',
        'csukuangfj/sherpa-onnx-streaming-paraformer-zh',
        'zh', 'paraformer-streaming, 230M', 1, 0),

    ('asr', 'sensevoice', 'sherpa-onnx-sense-voice-funasr-nano-int8',
        'csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17',
        'auto', 'SenseVoice FunASR Nano INT8, 265M', 1, 0),

    ('asr', 'qwen3-asr', 'qwen3-asr-0.6B',
        'csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25',
        'auto', 'qwen3-asr-0.6B, 1G', 1, 0),

    ('asr', 'qwen3-asr', 'qwen3-asr-1.7B',
        'ilmina/qwen3-asr-1.7b-sherpa-onnx',
        'auto', 'qwen3-asr-1.7B, 约2.7G', 1, 0),

    ('asr', 'whisper', 'whisper-small',
        'onnx-community/whisper-small',
        'auto', 'Whisper Small - 快速轻量, 250M', 1, 0);

-- LLM 润色模型
INSERT OR IGNORE INTO models (domain, category, name, source, description, is_thinking, is_local, is_enabled)
VALUES
    ('llm', 'deepseek', 'deepseek-v4-flash',
        'https://api.deepseek.com/',
        'DeepSeek V4 Flash（思考模型，需关闭 thinking）', 1, 0, 0),

    ('llm', 'bigmodel', 'glm-4-flashx',
        'https://open.bigmodel.cn/api/paas/v4',
        'GLM-4 FlashX（非思考）', 0, 0, 0),

    ('llm', 'bigmodel', 'glm-4.5-flash',
        'https://open.bigmodel.cn/api/paas/v4',
        'GLM-4.5 Flash（思考模型，需关闭 thinking）', 1, 0, 0);
