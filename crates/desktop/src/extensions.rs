//! Extension Package 加载、校验、导入。
//!
//! Package = ~/.octopus/extensions/<dir>/config.yaml + 脚本 + 资源。
//! 导入时创建 action_bar_items DB 记录（action_data 存脚本绝对路径）。

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// config.yaml 反序列化结构
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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

// ── Tauri commands ──

/// 导入结果——校验通过后返回，保存时才复制到 extensions
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub name: String,
    pub source_path: String,
    pub dir_name: String,
    pub is_async: bool,
    pub write_output_to_clipboard: bool,
}

/// 校验扩展包——支持 zip 文件或文件夹路径。
/// 仅校验 + 重复检测，不复制到 extensions。保存时调 install_extension。
#[tauri::command]
pub fn import_extension(source_path: String) -> Result<ImportResult, String> {
    use std::fs;

    let path = std::path::Path::new(&source_path);
    let is_zip = path.extension().map(|e| e == "zip").unwrap_or(false);

    // 定位 Package 目录（zip 解压到临时目录，文件夹直接用）
    let pkg_dir: std::path::PathBuf = if is_zip {
        let tmp_dir = std::env::temp_dir().join(format!(
            "octopus-ext-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
        ));
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).map_err(|e| format!("创建临时目录失败: {}", e))?;
        let zip_file = fs::File::open(path).map_err(|e| format!("打开 zip 失败: {}", e))?;
        let mut archive = zip::ZipArchive::new(zip_file).map_err(|e| format!("读取 zip 失败: {}", e))?;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| format!("读取 zip 条目失败: {}", e))?;
            let outpath = match file.enclosed_name() {
                Some(p) => tmp_dir.join(p),
                None => continue,
            };
            if file.is_dir() {
                fs::create_dir_all(&outpath).map_err(|e| format!("创建目录失败: {}", e))?;
            } else {
                if let Some(parent) = outpath.parent() {
                    fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
                }
                let mut outfile = fs::File::create(&outpath).map_err(|e| format!("创建文件失败: {}", e))?;
                std::io::copy(&mut file, &mut outfile).map_err(|e| format!("写入文件失败: {}", e))?;
                // 恢复 Unix 权限（可执行位等）
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Some(mode) = file.unix_mode() {
                        let _ = fs::set_permissions(&outpath, fs::Permissions::from_mode(mode));
                    }
                }
            }
        }
        let pkg = find_package_root(&tmp_dir)
            .ok_or_else(|| "zip 内未找到含 config.yaml 的顶层文件夹".to_string())?;
        // 不删 tmp_dir——保存时从这里复制；前端取消时由 clear_extension_temp 清理
        pkg
    } else if path.is_dir() {
        find_package_root(path).ok_or_else(|| "文件夹内未找到 config.yaml".to_string())?
    } else {
        return Err("路径既不是文件夹也不是 zip".into());
    };

    // 校验 config.yaml + 脚本
    let config = validate_package(&pkg_dir)?;

    let dir_name = pkg_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or("无法获取文件夹名")?;
    // 不拒绝已存在的 dir_name——install_extension 会 remove_dir_all 覆盖，
    // 允许编辑/重装时重选同一扩展包（否则形成编辑死锁）

    Ok(ImportResult {
        name: config.name,
        source_path: pkg_dir.to_string_lossy().to_string(),
        dir_name,
        is_async: config.action.is_async,
        write_output_to_clipboard: config.action.write_output_to_clipboard,
    })
}

/// 安装扩展——保存时调用。复制到 extensions + 创建 DB 记录。
#[tauri::command]
pub fn install_extension(
    source_path: String,
    dir_name: String,
    name: String,
    is_async: bool,
    write_output_to_clipboard: bool,
    parent_id: Option<i64>,
    shortcut: Option<String>,
    is_enabled: Option<bool>,
    replace_id: Option<i64>,
) -> Result<i64, String> {
    use std::fs;
    let src = std::path::Path::new(&source_path);
    let dest = extensions_dir().join(&dir_name);

    // 复制到 extensions
    fs::create_dir_all(extensions_dir()).map_err(|e| format!("创建 extensions 目录失败: {}", e))?;
    let _ = fs::remove_dir_all(&dest);
    copy_dir_recursive(src, &dest).map_err(|e| format!("复制文件失败: {}", e))?;

    // 清理临时目录（zip 解压的）
    if src.starts_with(std::env::temp_dir()) {
        let _ = fs::remove_dir_all(src);
    }

    // 创建/更新 DB 记录——action_data 存脚本绝对路径
    // replace_id 有值 = 编辑时重选扩展包：update 现有记录（保持 sort_order/位置），否则 insert 新记录
    let config = validate_package(&dest)?;
    let script_abs = dest.join(&config.action.script);
    let shortcut = shortcut.unwrap_or_default();
    let is_enabled = is_enabled.unwrap_or(true);
    if let Some(id) = replace_id {
        octopus_infra::db::update_action_bar_item(
            id,
            &name,
            "",
            "script",
            &script_abs.to_string_lossy(),
            is_enabled,
            is_async,
            write_output_to_clipboard,
            &shortcut,
            "",
            "text",
            "",
            false,  // need_voice——script 类型不需要语音
        )
        .map_err(|e| e.to_string())?;
        Ok(id)
    } else {
        octopus_infra::db::insert_action_bar_item(
            parent_id,
            &name,
            "",
            "script",
            &script_abs.to_string_lossy(),
            is_async,
            write_output_to_clipboard,
            &shortcut,
            "",
            "text",
            "",
            is_enabled,
            false,  // need_voice——script 类型不需要语音
        )
        .map_err(|e| e.to_string())
    }
}

/// 返回扩展列表 + DB 关联
#[tauri::command]
pub fn list_extensions() -> Result<Vec<ExtensionInfo>, String> {
    let packages = scan_extensions();
    let dir = extensions_dir();
    let dir_prefix = dir.to_string_lossy().to_string();

    let db_items = octopus_infra::db::list_all_action_bar_items().map_err(|e| e.to_string())?;
    let db_map: std::collections::HashMap<String, (i64, Option<i64>)> = db_items
        .iter()
        .filter(|i| i.action_data.starts_with(&dir_prefix))
        .map(|i| (i.action_data.clone(), (i.id, i.parent_id)))
        .collect();

    let mut list = Vec::new();
    for (dir_name, config) in packages {
        let pkg_path = dir.join(&dir_name);
        let script_abs = pkg_path.join(&config.action.script);
        let script_key = script_abs.to_string_lossy().to_string();
        let (db_item_id, parent_id) = db_map
            .get(&script_key)
            .map(|(id, pid)| (Some(*id), *pid))
            .unwrap_or((None, None));

        list.push(ExtensionInfo {
            dir_name,
            name: config.name,
            description: config.description,
            version: config.version,
            author: config.author,
            has_skill: config.skill.is_some(),
            skill_ref: config
                .skill
                .as_ref()
                .and_then(|s| s.skill_ref.clone().or_else(|| s.file.clone())),
            script_file: config.action.script.clone(),
            script_type_label: read_script_magic_comment(&pkg_path, &config.action.script),
            is_async: config.action.is_async,
            db_item_id,
            parent_id,
        });
    }
    Ok(list)
}

/// 删 DB 记录 + extensions 文件夹
#[tauri::command]
pub fn delete_extension(dir_name: String) -> Result<(), String> {
    use std::fs;
    let dir = extensions_dir().join(&dir_name);
    if !dir.exists() {
        return Err("扩展包不存在".into());
    }
    let db_items = octopus_infra::db::list_all_action_bar_items().map_err(|e| e.to_string())?;
    for item in db_items {
        if std::path::Path::new(&item.action_data).starts_with(&dir) {
            let _ = octopus_infra::db::delete_action_bar_item(item.id);
        }
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("删除文件夹失败: {}", e))?;
    Ok(())
}

/// 重新扫描（确保目录存在）
#[tauri::command]
pub fn refresh_extensions() -> Result<(), String> {
    std::fs::create_dir_all(extensions_dir()).map_err(|e| format!("创建目录失败: {}", e))?;
    Ok(())
}

// ── 辅助函数 ──

fn find_package_root(base: &Path) -> Option<PathBuf> {
    if base.join("config.yaml").exists() {
        return Some(base.to_path_buf());
    }
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("config.yaml").exists() {
                return Some(path);
            }
        }
    }
    None
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::fs;
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
