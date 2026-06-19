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
    edited_text   TEXT,                     -- 用户编辑后的最终文本（未编辑为 NULL）
    polish_status TEXT    NOT NULL DEFAULT 'off',
    polish_model  TEXT,
    duration_ms   INTEGER,
    char_count    INTEGER
);

CREATE INDEX IF NOT EXISTS idx_trans_created ON transcriptions(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_trans_engine  ON transcriptions(engine);

CREATE TABLE IF NOT EXISTS models (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    domain        TEXT    NOT NULL,                       -- 'asr' | 'llm'
    provider      TEXT    NOT NULL DEFAULT 'local',       -- vendor/运行位置：local/aliyun/deepseek/bigmodel
    category      TEXT    NOT NULL,                       -- ASR 引擎族(zipformer/whisper/Fun-ASR) ; LLM 模型系列(qwen/glm/deepseek)
    model_name    TEXT    NOT NULL,                       -- 具体模型标识，精确匹配
    source        TEXT    NOT NULL,                       -- ASR: 本地路径/HF repo/云 wss 端点 ; LLM: API base URL
    secret_key    TEXT    NOT NULL DEFAULT '',            -- 远程 API Key（本地模型留空）
    language      TEXT    NOT NULL DEFAULT '',
    is_local      INTEGER NOT NULL DEFAULT 0,             -- 是否为本地模型 (0=否, 1=是)
    is_thinking   INTEGER NOT NULL DEFAULT 0,             -- LLM 专用：是否为思考（reasoning）模型
    is_streaming  INTEGER NOT NULL DEFAULT 0,             -- 是否支持流式 (0=否, 1=是)
    is_enabled    INTEGER NOT NULL DEFAULT 1,             -- 是否启用 (0=禁用, 1=启用)
    description   TEXT    NOT NULL DEFAULT '',            -- 描述
    UNIQUE(domain, provider, category, model_name)        -- domain + provider + category + model_name 作为唯一键
);

-- ── 默认数据（INSERT OR IGNORE，幂等）────────────────────────────────────────

-- ASR 引擎：激活由 config.yaml.asr_engine 控制，此处仅维护可选列表
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, language, description, is_local, is_enabled, is_streaming)
VALUES
    ('asr','local','zipformer','zipformer-small-ctc','models/zipformer','zh','zipformer-small-ctc, 27M（随应用打包，兜底引擎）',1,1,1),
    ('asr','local','zipformer','zipformer-multi','k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13','zh','zipformer-multi, 80M',1,0,1),
    ('asr','local','zipformer','zipformer-ctc','csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30','zh','zipformer-ctc, 163M',1,0,1),
    ('asr','local','paraformer','paraformer-streaming','csukuangfj/sherpa-onnx-streaming-paraformer-zh','zh','paraformer-streaming, 230M',1,0,1),
    ('asr','local','sensevoice','sherpa-onnx-sense-voice-funasr-nano-int8','csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17','auto','SenseVoice FunASR Nano INT8, 265M',1,0,0),
    ('asr','local','qwen3-asr','qwen3-asr-0.6B','csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25','auto','qwen3-asr-0.6B, 1G',1,0,0),
    ('asr','local','qwen3-asr','qwen3-asr-1.7B','ilmina/qwen3-asr-1.7b-sherpa-onnx','auto','qwen3-asr-1.7B, 约2.7G',1,0,0),
    ('asr','local','whisper','whisper-small','onnx-community/whisper-small','auto','Whisper Small - 快速轻量, 250M',1,0,0),
    -- 阿里云 FunASR 实时（Feature 2 seed；is_streaming=0 走 chunk 路径；secret_key 用户填）
    ('asr','aliyun','Fun-ASR','fun-asr-2025-11-07','wss://dashscope.aliyuncs.com/api-ws/v1/inference','auto','阿里云百炼 FunASR 实时（DashScope key 填 secret_key）',0,0,0);

-- LLM 润色模型（原 category=vendor 迁移到 provider；category=模型系列）
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, description, is_thinking, is_local, is_enabled)
VALUES
    ('llm','deepseek','deepseek','deepseek-v4-flash','https://api.deepseek.com/','DeepSeek V4 Flash（思考模型，需关闭 thinking）',1,0,0),
    ('llm','aliyun','deepseek','deepseek-v4-flash','https://dashscope.aliyuncs.com/compatible-mode/v1','DeepSeek V4 Flash 经 DashScope（思考模型）',1,0,0),
    ('llm','bigmodel','glm','glm-4-flashx','https://open.bigmodel.cn/api/paas/v4','GLM-4 FlashX（非思考）',0,0,0),
    ('llm','bigmodel','glm','glm-4.5-flash','https://open.bigmodel.cn/api/paas/v4','GLM-4.5 Flash（思考模型，需关闭 thinking）',1,0,0),
    -- Feature 1：阿里云 Qwen 原生（DashScope OpenAI 兼容端点）
    ('llm','aliyun','qwen','qwen-plus','https://dashscope.aliyuncs.com/compatible-mode/v1','Qwen Plus（非思考）',0,0,0),
    ('llm','aliyun','qwen','qwen-turbo','https://dashscope.aliyuncs.com/compatible-mode/v1','Qwen Turbo（非思考，快）',0,0,0);

-- ── 应用配置（app_config 表）─────────────────────────────────────────────────
-- config.yaml 的 DB 化：所有应用行为配置（引擎/快捷键/润色/降噪等）以 key-value 存储。
-- 值统一 TEXT，由 Rust 侧 load_app_config 按字段类型解析。
-- category 用于后续分组（如 'default' / 'audio' / 'network'），当前全部 'default'。
-- 首次启动由 init_schema 执行 seed；后续 set_config / persist_* 通过 ON CONFLICT DO UPDATE 仅改 config_value，保留 description + category。

CREATE TABLE IF NOT EXISTS app_config (
    category     TEXT NOT NULL DEFAULT 'default',
    config_key   TEXT PRIMARY KEY,
    config_value TEXT NOT NULL,
    description  TEXT
);

INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
    ('engine_mode',              'embedded',                        'ASR 引擎模式: embedded | websocket | grpc'),
    ('remote_url',               'ws://127.0.0.1:3000/ws/stream',   'WebSocket 远程地址（engine_mode=websocket 时使用）'),
    ('grpc_endpoint',            'http://127.0.0.1:50051',          'gRPC 端点（engine_mode=grpc 时使用）'),
    ('asr_engine',               '',                                'ASR 引擎选择（DB models 表 model_name 精确匹配；空=兜底引擎）'),
    ('language',                 'auto',                            '识别语言: auto | zh | en | ja | ko'),
    ('asr_shortcut',             'CmdOrCtrl+Shift+Space',           '全局 ASR 激活/关闭快捷键'),
    ('edit_shortcut',            'Cmd+E',                           '结果窗进入编辑快捷键（保存固定 Cmd+Enter）'),
    ('paste_method',             'clipboard',                       '粘贴方式: clipboard | direct | none'),
    ('write_to_clipboard',       'true',                            '粘贴后是否把结果写入剪贴板'),
    ('microphone',               '',                                '麦克风名称（空=系统默认）'),
    ('overlay_position',         'top',                             'overlay 位置: top | bottom | none'),
    ('segment_silence',          '400',                             'VAD 静音触发识别阈值（毫秒）'),
    ('polish_mode',              '0',                               '润色模式: 0=关闭 / 1=仅最终 / 2=中间+最终'),
    ('polish_min_interval',      '5',                               '中间润色最小间隔（秒，节流用）'),
    ('pause_polish_threshold_ms','600',                             '停顿驱动中间润色的静音阈值（毫秒，必须 > 500）'),
    ('polish_llm',               'bigmodel:glm:glm-4-flashx',       '润色 LLM 模型 spec（PREFIX:CATEGORY:NAME）'),
    ('asr_hardware_accelerated', 'false',                           '是否使用 ASR 硬件加速'),
    ('asr_correct',              'false',                           '是否对 ASR 输出进行纠错'),
    ('output_simplified',        'true',                            'ASR 输出字形: true=简体 / false=繁体'),
    ('hide_toolbar',             'true',                            '结果展示区工具栏是否自动隐藏'),
    ('denoise_mode',             '1',                               '降噪模式: 0=无 / 1=轻度 / 2=深度');
