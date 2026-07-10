//! Extension Package 加载、校验、导入。
//!
//! Package = ~/.octopus/extensions/<dir>/config.yaml + 脚本 + 资源。
//! 导入时创建 action_bar_items DB 记录（action_data 存脚本绝对路径）。

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// config.yaml 反序列化结构
#[derive(Debug, Clone, Deserialize)]
pub struct PackageConfig {
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub action: PackageAction,
    #[serde(default)]
    pub rules: Option<serde_yaml::Mapping>,
    #[serde(default)]
    pub skill: Option<PackageSkill>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PackageAction {
    #[serde(rename = "type", default = "default_script_type")]
    pub action_type: String,
    pub script: String,
    #[serde(default = "default_true")]
    pub is_async: bool,
    #[serde(default)]
    pub write_output_to_clipboard: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PackageSkill {
    #[serde(rename = "ref")]
    pub skill_ref: Option<String>,
    pub file: Option<String>,
    pub description: Option<String>,
}

fn default_script_type() -> String {
    "script".into()
}

fn default_true() -> bool {
    true
}

/// 扩展子页展示信息
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionInfo {
    pub dir_name: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub has_skill: bool,
    pub skill_ref: Option<String>,
    pub script_file: String,
    pub script_type_label: Option<String>,
    pub is_async: bool,
    pub db_item_id: Option<i64>,
    pub parent_id: Option<i64>,
}

/// ~/.octopus/extensions/
pub fn extensions_dir() -> PathBuf {
    octopus_infra::paths::octopus_config_home().join("extensions")
}

/// 扫描 extensions 目录，返回所有合法 Package 的 (dir_name, PackageConfig)
pub fn scan_extensions() -> Vec<(String, PackageConfig)> {
    let dir = extensions_dir();
    if !dir.exists() {
        return Vec::new();
    }
    let mut packages = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let config_path = path.join("config.yaml");
                if config_path.exists() {
                    if let Ok(config_str) = std::fs::read_to_string(&config_path) {
                        if let Ok(config) = serde_yaml::from_str::<PackageConfig>(&config_str) {
                            packages.push((
                                entry.file_name().to_string_lossy().to_string(),
                                config,
                            ));
                        }
                    }
                }
            }
        }
    }
    packages
}

/// 校验 Package 目录结构 + config.yaml 必填字段 + 脚本文件存在
pub fn validate_package(pkg_dir: &Path) -> Result<PackageConfig, String> {
    let config_path = pkg_dir.join("config.yaml");
    if !config_path.exists() {
        return Err("缺少 config.yaml".into());
    }
    let config_str = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("读取 config.yaml 失败: {}", e))?;
    let config: PackageConfig = serde_yaml::from_str(&config_str)
        .map_err(|e| format!("config.yaml 格式错误: {}", e))?;

    if config.name.trim().is_empty() {
        return Err("name 不能为空".into());
    }
    if config.description.trim().is_empty() {
        return Err("description 不能为空".into());
    }
    if config.version.trim().is_empty() {
        return Err("version 不能为空".into());
    }
    if config.author.trim().is_empty() {
        return Err("author 不能为空".into());
    }

    if config.action.action_type != "script" {
        return Err(format!(
            "action.type 仅支持 script（当前: {}）",
            config.action.action_type
        ));
    }
    let script_path = pkg_dir.join(&config.action.script);
    if !script_path.exists() {
        return Err(format!("脚本文件不存在: {}", config.action.script));
    }
    Ok(config)
}

/// 读脚本文件第一行 magic comment（如 #python / #shell）
pub fn read_script_magic_comment(pkg_dir: &Path, script_rel: &str) -> Option<String> {
    let script_path = pkg_dir.join(script_rel);
    let content = std::fs::read_to_string(&script_path).ok()?;
    content.lines().next().map(|l| l.trim().to_string())
}
