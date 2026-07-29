use parking_lot::Mutex;
use std::collections::HashMap;

const ZH_CN_YAML: &str = include_str!("../../frontend/src/locales/zh-CN.yaml");
const EN_YAML: &str = include_str!("../../frontend/src/locales/en.yaml");

type Dict = HashMap<String, String>;

static DICT: once_cell::sync::Lazy<Mutex<Dict>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

/// 递归拍平 serde_yaml::Value 为 flat dotted keys
fn flatten(value: &serde_yaml::Value, prefix: &str, result: &mut Dict) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                if let serde_yaml::Value::String(key) = k {
                    let full_key = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    flatten(v, &full_key, result);
                }
            }
        }
        serde_yaml::Value::String(s) => {
            result.insert(prefix.to_string(), s.clone());
        }
        serde_yaml::Value::Bool(b) => {
            result.insert(prefix.to_string(), b.to_string());
        }
        serde_yaml::Value::Number(n) => {
            result.insert(prefix.to_string(), n.to_string());
        }
        _ => {}
    }
}

/// 解析 YAML 为 flat dict
fn parse_locale(yaml_str: &str) -> Dict {
    let mut dict = Dict::new();
    match serde_yaml::from_str::<serde_yaml::Value>(yaml_str) {
        Ok(value) => flatten(&value, "", &mut dict),
        Err(e) => log::error!("Failed to parse locale YAML: {e}"),
    }
    dict
}

/// 根据 ui_language 选择对应的 dict
fn dict_for(ui_language: &str) -> Dict {
    match ui_language {
        "en" => parse_locale(EN_YAML),
        _ => parse_locale(ZH_CN_YAML),
    }
}

/// 初始化全局 locale dict（启动时调用）
pub fn init(ui_language: &str) {
    let dict = dict_for(ui_language);
    *DICT.lock() = dict;
    log::info!("i18n initialized with locale: {ui_language}");
}

/// 重新加载 locale（语言切换时调用）
pub fn reload(ui_language: &str) {
    init(ui_language);
}

/// flat key 查找 + ${name} 插值
pub fn t(key: &str, params: &[(&str, &str)]) -> String {
    let dict = DICT.lock();
    let mut str = match dict.get(key) {
        Some(v) => v.clone(),
        None => key.to_string(),
    };
    drop(dict);
    for (name, value) in params {
        str = str.replace(&format!("${{{name}}}"), value);
    }
    str
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_zh_cn() {
        let dict = parse_locale(ZH_CN_YAML);
        assert_eq!(dict.get("editor.undo").unwrap(), "撤销");
        assert_eq!(dict.get("editor.view.split").unwrap(), "分屏");
    }

    #[test]
    fn test_parse_en() {
        let dict = parse_locale(EN_YAML);
        assert_eq!(dict.get("editor.undo").unwrap(), "Undo");
        assert_eq!(dict.get("editor.view.split").unwrap(), "Split");
    }

    #[test]
    fn test_t_interpolation() {
        init("zh-CN");
        assert_eq!(t("editor.charCount", &[("n", "42")]), "42 字");
        init("en");
        assert_eq!(t("editor.charCount", &[("n", "42")]), "42 chars");
    }

    #[test]
    fn test_t_missing_key() {
        init("zh-CN");
        assert_eq!(t("nonexistent.key", &[]), "nonexistent.key");
    }
}
