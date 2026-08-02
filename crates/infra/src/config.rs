//! 统一应用配置 schema（config.yaml 的统一定义）。
//!
//! 本模块是 config.yaml 的唯一 schema 来源，所有 crate（asr/desktop/cli 等）共享。
//! 多余字段对不使用它们的 crate 无害——各 crate 只读自己关心的字段。

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// LLM 润色模式（config.yaml 的 polish_mode 字段，整数 0/1/2）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolishMode {
    /// 0 — 完全不润色（默认）
    #[default]
    Disabled,
    /// 1 — 仅最终润色（识别结束后润色一次）
    FinalOnly,
    /// 2 — 中间润色 + 最终润色
    Intermediate,
}

impl<'de> Deserialize<'de> for PolishMode {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let n = u8::deserialize(deserializer)?;
        Ok(match n {
            0 => PolishMode::Disabled,
            1 => PolishMode::FinalOnly,
            2 => PolishMode::Intermediate,
            other => {
                log::warn!("polish_mode={} 非法（应为 0/1/2），回退 0(Disabled)", other);
                PolishMode::Disabled
            }
        })
    }
}

impl Serialize for PolishMode {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(match self {
            PolishMode::Disabled => 0,
            PolishMode::FinalOnly => 1,
            PolishMode::Intermediate => 2,
        })
    }
}

/// 应用完整配置，读取自 ~/.octopus/config.yaml，字段缺失时使用默认值。
///
/// 各端按需读取字段：cli 只用 microphone；desktop 用全部；asr 引擎激活态走 DB is_enabled。
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    /// ASR 引擎模式: embedded | websocket | grpc
    #[serde(default = "default_engine_mode")]
    pub engine_mode: String,

    /// WebSocket 远程地址（engine_mode = websocket 时使用）
    #[serde(default = "default_remote_url")]
    pub remote_url: String,

    /// gRPC 端点（engine_mode = grpc 时使用）
    #[serde(default = "default_grpc_endpoint")]
    pub grpc_endpoint: String,

    /// 语言: auto | zh | en | ja | ko
    #[serde(default = "default_language")]
    pub language: String,

    /// UI 界面语言: zh-CN | en
    #[serde(default = "default_ui_language")]
    pub ui_language: String,

    /// 单键三模式触发键（handy-keys 名：OptRight/CmdRight/CtrlRight/ShiftRight/Fn），长按=PTT / 双击=toggle / 短按=hands-free
    #[serde(default = "default_asr_shortcut")]
    pub asr_shortcut: String,

    /// 粘贴方式: clipboard | direct | none
    #[serde(default = "default_paste_method")]
    pub paste_method: String,

    /// 粘贴后是否把识别结果写入剪贴板（默认 true，方便他处再粘贴）。
    /// false 时保留用户原剪贴板内容（等同旧行为）。
    #[serde(default = "default_write_to_clipboard")]
    pub write_to_clipboard: bool,

    /// 粘贴前是否临时切换到 ASCII 输入源（默认 true）。
    /// CJK 输入法（中文/日文/韩文）在 composing 状态下，模拟 Cmd+V 粘贴可能导致
    /// 乱码或字符丢失。开启后粘贴前自动切到 ABC → Cmd+V → 恢复原输入源。
    /// 仅 macOS 生效（Windows/Linux 无此问题）。
    #[serde(default = "default_switch_input_source_on_paste")]
    pub switch_input_source_on_paste: bool,

    /// 麦克风名称（空 = 系统默认）
    #[serde(default)]
    pub microphone: String,

    /// VAD 伪流式：静音触发识别的时长阈值（毫秒）
    /// 检测到语音后静音超过此时长即发送识别，默认 500 毫秒
    #[serde(default = "default_segment_silence")]
    pub segment_silence: f64,

    /// overlay 位置: top | bottom | none
    #[serde(default = "default_overlay_position")]
    pub overlay_position: String,

    /// 润色模式：0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色
    #[serde(default)]
    pub polish_mode: PolishMode,

    /// 中间润色间隔（秒），0 = 仅最终润色
    #[serde(default = "default_polish_min_interval")]
    pub polish_min_interval: f64,

    /// 停顿驱动中间润色的静音阈值（毫秒）：静音达此值即触发全量润色（mode=2 only）
    #[serde(default = "default_pause_polish_threshold_ms")]
    pub pause_polish_threshold_ms: f64,

    /// 是否使用 ASR 硬件加速
    #[serde(default = "default_asr_hardware_accelerated")]
    pub asr_hardware_accelerated: bool,

    /// 是否对 ASR 输出进行纠错与热词校正
    #[serde(default = "default_asr_correct")]
    pub asr_correct: bool,

    /// ASR 输出字形：true→简体（繁→简），false→繁体（简→繁）。默认简体。
    #[serde(default = "default_output_simplified")]
    pub output_simplified: bool,

    /// 结果展示区工具栏是否自动隐藏。true→鼠标移入显示、移出隐藏（默认）；
    /// false→工具栏始终显示（窗口高度保持展开态）。
    #[serde(default = "default_hide_toolbar")]
    pub hide_toolbar: bool,

    /// 降噪模式：0=无降噪，1=轻度降噪，2=深度降噪。默认 1。
    #[serde(default = "default_denoise_mode")]
    pub denoise_mode: u8,

    /// 结果展示区编辑 toggle 快捷键——进入与保存（退出）编辑都用此键，与 ✏️ 按钮同语义
    /// （窗口内、仅结果窗聚焦时生效）。Tauri Accelerator 格式（如 "Cmd+Enter"），默认 "Cmd+Enter"。
    #[serde(default = "default_edit_shortcut")]
    pub edit_shortcut: String,

    /// 全局编辑快捷键——任意应用聚焦时唤起结果窗并进入/保存编辑（toggle，复用窗口内编辑语义）。
    /// 与 edit_shortcut（窗口内、仅结果窗聚焦时生效）并存：edit_global 负责跨应用唤起+toggle，
    /// edit_shortcut 负责结果窗已聚焦时的编辑 toggle（用户明确要求保留不动）。
    /// Tauri Accelerator 格式，默认 "Alt+E"（不与 Alt+A/CmdOrCtrl+Enter 冲突）。
    #[serde(default = "default_edit_global_shortcut")]
    pub edit_global_shortcut: String,

    /// HF 模型下载镜像 host（如 `https://hf-mirror.com`）。空 = 官方源 huggingface.co。
    /// cli `download --mirror` 临时覆盖此值；优先级 `--mirror` > 此字段 > 官方源。
    #[serde(default = "default_download_mirror")]
    pub download_mirror: String,

    /// 剪贴板历史浮窗全局快捷键（Tauri Accelerator 格式）。默认 "Alt+C"。
    #[serde(default = "default_clipboard_shortcut")]
    pub clipboard_shortcut: String,

    /// 剪贴板最大保留条数（不含收藏，超出自动清理）。默认 1000。
    #[serde(default = "default_clipboard_max_items")]
    pub clipboard_max_items: i64,

    /// 剪贴板自动清理天数（超过此天数的非收藏记录自动删除）。默认 30。
    #[serde(default = "default_clipboard_max_age_days")]
    pub clipboard_max_age_days: i64,

    /// 是否启用剪贴板历史监听（ClipboardWatcher）。true→记录剪贴板历史；false→watcher 仍运行但不入库。
    /// 设置页「交互」开关 + 浮窗 title bar 快捷按钮可配，热重载生效（运行时 AtomicBool flag，
    /// 见 ClipboardHandle::recording_enabled；set_config 收到变更即翻转，无需重启）。默认 true。
    #[serde(default = "default_clipboard_enabled")]
    pub clipboard_enabled: bool,

    /// UI 主题 id（light / glass-dark / nord / 用户自定义）。默认 light。
    #[serde(default = "default_clipboard_theme")]
    pub clipboard_theme: String,

    /// AI 命令面板全局热键。默认 Alt+D。
    #[serde(default = "default_action_bar_shortcut")]
    pub action_bar_shortcut: String,

    /// AI 命令面板搜索引擎。默认 google。
    #[serde(default = "default_action_bar_search_engine")]
    pub action_bar_search_engine: String,

    /// 截图全局快捷键（Tauri Accelerator 格式）。默认 "CmdOrCtrl+Shift+X"。
    #[serde(default = "default_screenshot_shortcut")]
    pub screenshot_shortcut: String,

    /// 录屏 toggle 快捷键（弹配置浮窗 / 暂停 / 恢复）。默认 "CmdOrCtrl+Shift+R"。
    /// 仅 macOS 实际使用（record_hotkey 模块 cfg-gate），其他平台仅落库不消费。
    ///
    /// 停止录屏的快捷键固定为 Escape（octopus 全局通用停止键，不暴露为可配置项）。
    #[serde(default = "default_record_shortcut")]
    pub record_shortcut: String,

    /// vault Auto-Type 全局热键。默认 CmdOrCtrl+Shift+S。
    ///
    /// follow-up #10：仅在 `octopus-desktop` 启用 `vault` cargo feature 时实际使用
    /// （main.rs setup() 里 cfg-gate 了热键注册）。feature off 时此字段仍存在
    /// （serde 友好 + ~50B 开销可忽略），但仅落库 / 序列化，不被消费。
    #[serde(default = "default_vault_autotype_shortcut")]
    pub vault_autotype_shortcut: String,

    /// vault 离开焦点后的锁定超时（秒）。默认 180 = 3 分钟。
    ///
    /// 含义：保险库 tab 离开前台（切 tab / 关窗口 / 应用失焦）超过此秒数后，
    /// 自动锁定 user_vault_key（zeroize）。在前台时前端每 30s 调
    /// `vault_heartbeat` 刷新 last_active_at，永不超时。
    ///
    /// 可选值：30 / 60 / 180 / 300 / 900（30s / 1min / 3min / 5min / 15min）。
    /// 0 = 永不超时（不推荐，但允许）。
    ///
    /// 安全 vs 易用参考：
    ///   - 1Password 默认 5min，可调 1-30min
    ///   - Bitwarden 默认 15min
    ///   - octopus 默认 3min（折中），用户可改
    #[serde(default = "default_vault_lock_timeout_secs")]
    pub vault_lock_timeout_secs: u64,

    /// 首次启动权限引导页是否已完成（false=首次启动，弹引导页）。
    /// 用户点「完成」后置 true，不再弹。重置需清 DB。
    #[serde(default)]
    pub onboarding_completed: bool,

    /// 终端字号（px）。默认 13。
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: f64,

    /// 终端字体族（CSS font-family 字符串）。默认 SF Mono 系列。
    #[serde(default = "default_terminal_font_family")]
    pub terminal_font_family: String,
}

fn default_engine_mode() -> String {
    "embedded".into()
}
fn default_remote_url() -> String {
    "ws://127.0.0.1:3000/ws/stream".into()
}
fn default_grpc_endpoint() -> String {
    "http://127.0.0.1:50051".into()
}
fn default_language() -> String {
    "auto".into()
}
fn default_ui_language() -> String {
    "zh-CN".into()
}
fn default_asr_shortcut() -> String {
    "OptRight".into()
}
fn default_paste_method() -> String {
    "clipboard".into()
}
fn default_write_to_clipboard() -> bool {
    true
}
fn default_switch_input_source_on_paste() -> bool {
    true
}
fn default_overlay_position() -> String {
    "top".into()
}
fn default_polish_min_interval() -> f64 {
    5.0
}
fn default_pause_polish_threshold_ms() -> f64 {
    600.0
}
fn default_asr_hardware_accelerated() -> bool {
    false
}
fn default_asr_correct() -> bool {
    // 2026-08-01 改为 true：用户加了热词就期望生效（流式+批量）。
    // corrector 无热词即 no-op（零过纠铁证保留），无副作用。存量库 DB 已有值不受影响（INSERT OR IGNORE）。
    true
}
fn default_output_simplified() -> bool {
    true
}
fn default_hide_toolbar() -> bool {
    true
}
fn default_denoise_mode() -> u8 {
    1
}
fn default_edit_shortcut() -> String {
    "CmdOrCtrl+Enter".into()
}
fn default_edit_global_shortcut() -> String {
    "Alt+E".into()
}
fn default_download_mirror() -> String {
    String::new()
}
fn default_clipboard_shortcut() -> String {
    "Alt+C".into()
}
fn default_clipboard_max_items() -> i64 {
    1000
}
fn default_clipboard_max_age_days() -> i64 {
    30
}
fn default_clipboard_enabled() -> bool {
    true
}
fn default_clipboard_theme() -> String {
    "light".into()
}
fn default_action_bar_shortcut() -> String {
    "Alt+D".into()
}
fn default_action_bar_search_engine() -> String {
    "google".into()
}
fn default_screenshot_shortcut() -> String {
    "CmdOrCtrl+Shift+X".into()
}
fn default_record_shortcut() -> String {
    "CmdOrCtrl+Shift+R".into()
}
fn default_vault_autotype_shortcut() -> String {
    "CmdOrCtrl+Shift+S".into()
}
fn default_vault_lock_timeout_secs() -> u64 {
    180 // 3 分钟（焦点失活后）
}

fn default_terminal_font_size() -> f64 {
    13.0
}
fn default_terminal_font_family() -> String {
    "SF Mono".to_string()
}

fn default_segment_silence() -> f64 {
    400.0
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            engine_mode: default_engine_mode(),
            remote_url: default_remote_url(),
            grpc_endpoint: default_grpc_endpoint(),
            language: default_language(),
            ui_language: default_ui_language(),
            asr_shortcut: default_asr_shortcut(),
            paste_method: default_paste_method(),
            write_to_clipboard: default_write_to_clipboard(),
            switch_input_source_on_paste: default_switch_input_source_on_paste(),
            microphone: String::new(),
            segment_silence: default_segment_silence(),
            overlay_position: default_overlay_position(),
            polish_mode: PolishMode::default(),
            polish_min_interval: default_polish_min_interval(),
            pause_polish_threshold_ms: default_pause_polish_threshold_ms(),
            asr_hardware_accelerated: default_asr_hardware_accelerated(),
            asr_correct: default_asr_correct(),
            // fuzzy_dialect 已迁移到 fuzzy_dialect_rules DB 表（2026-08-01）
            output_simplified: default_output_simplified(),
            hide_toolbar: default_hide_toolbar(),
            denoise_mode: default_denoise_mode(),
            edit_shortcut: default_edit_shortcut(),
            edit_global_shortcut: default_edit_global_shortcut(),
            download_mirror: default_download_mirror(),
            clipboard_shortcut: default_clipboard_shortcut(),
            clipboard_max_items: default_clipboard_max_items(),
            clipboard_max_age_days: default_clipboard_max_age_days(),
            clipboard_enabled: default_clipboard_enabled(),
            clipboard_theme: default_clipboard_theme(),
            action_bar_shortcut: default_action_bar_shortcut(),
            action_bar_search_engine: default_action_bar_search_engine(),
            screenshot_shortcut: default_screenshot_shortcut(),
            record_shortcut: default_record_shortcut(),
            vault_autotype_shortcut: default_vault_autotype_shortcut(),
            vault_lock_timeout_secs: default_vault_lock_timeout_secs(),
            onboarding_completed: false,
            terminal_font_size: default_terminal_font_size(),
            terminal_font_family: default_terminal_font_family(),
        }
    }
}

/// 从 DB app_config 表加载应用配置。
///
/// 内部委托 `db::load_app_config()`（首次调用时触发 ensure_db + init_schema，
/// 包含 yaml → DB 一次性迁移）。不缓存——调用方各自决定是否缓存
/// （asr 侧引擎配置另有 OnceLock 缓存）。
pub fn load_config() -> Result<AppConfig> {
    crate::db::load_app_config()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polish_mode_deserialize_values() {
        assert_eq!(serde_yaml::from_str::<PolishMode>("0").unwrap(), PolishMode::Disabled);
        assert_eq!(serde_yaml::from_str::<PolishMode>("1").unwrap(), PolishMode::FinalOnly);
        assert_eq!(serde_yaml::from_str::<PolishMode>("2").unwrap(), PolishMode::Intermediate);
    }

    #[test]
    fn polish_mode_invalid_falls_back_to_disabled() {
        assert_eq!(serde_yaml::from_str::<PolishMode>("3").unwrap(), PolishMode::Disabled);
        assert_eq!(serde_yaml::from_str::<PolishMode>("99").unwrap(), PolishMode::Disabled);
    }

    #[test]
    fn polish_mode_default_is_disabled() {
        assert_eq!(PolishMode::default(), PolishMode::Disabled);
    }

    #[test]
    fn app_config_default_values() {
        let cfg = AppConfig::default();
        assert!(cfg.write_to_clipboard, "write_to_clipboard 应默认 true");
        assert_eq!(cfg.pause_polish_threshold_ms, 600.0);
        assert!(!cfg.asr_hardware_accelerated);
        assert!(cfg.asr_correct, "asr_correct 应默认 true（2026-08-01，加了热词即生效）");
        assert_eq!(cfg.denoise_mode, 1);
        assert_eq!(cfg.edit_shortcut, "CmdOrCtrl+Enter");
        assert_eq!(cfg.edit_global_shortcut, "Alt+E");
        assert_eq!(cfg.segment_silence, 400.0);
    }

    #[test]
    fn app_config_serialize_round_trip_preserves_overrides() {
        // 构造一个带覆盖值的 AppConfig（从 yaml 解析）
        let yaml = "polish_mode: 2\nmicrophone: \"My Mic\"\n";
        let cfg: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.polish_mode, PolishMode::Intermediate);

        // 序列化回 yaml，再解析，字段应保留
        let reserialized = serde_yaml::to_string(&cfg).unwrap();
        let cfg2: AppConfig = serde_yaml::from_str(&reserialized).unwrap();
        assert_eq!(cfg2.polish_mode, PolishMode::Intermediate);
        assert_eq!(cfg2.microphone, "My Mic");

        // polish_mode 序列化为整数（u8），非枚举名
        assert!(
            reserialized.contains("polish_mode: 2"),
            "polish_mode 应序列化为整数 2，实际: {}",
            reserialized
        );
    }

    #[test]
    fn denoise_mode_explicit_from_yaml() {
        // 显式 0/1/2 直接落到字段。
        let cfg: AppConfig = serde_yaml::from_str("denoise_mode: 2\n").unwrap();
        assert_eq!(cfg.denoise_mode, 2);
        let cfg: AppConfig = serde_yaml::from_str("denoise_mode: 0\n").unwrap();
        assert_eq!(cfg.denoise_mode, 0);
    }

    #[test]
    fn denoise_mode_legacy_denoise_enabled_ignored() {
        // 旧 denoise_enabled 已删除：serde 默认忽略未知字段，
        // 旧 config.yaml 里残留的 denoise_enabled 不影响 denoise_mode 解析。
        let cfg: AppConfig =
            serde_yaml::from_str("denoise_mode: 2\ndenoise_enabled: false\n").unwrap();
        assert_eq!(cfg.denoise_mode, 2);
    }

    #[test]
    fn edit_shortcut_explicit_from_yaml() {
        // 显式值原样落到字段（Tauri Accelerator 字符串）
        let cfg: AppConfig = serde_yaml::from_str("edit_shortcut: CmdOrCtrl+Shift+E\n").unwrap();
        assert_eq!(cfg.edit_shortcut, "CmdOrCtrl+Shift+E");
    }

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
}
