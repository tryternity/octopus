use parking_lot::Mutex;
use std::collections::HashMap;

const ZH_CN_YAML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/frontend/src/locales/zh-CN.yaml"));
const EN_YAML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/frontend/src/locales/en.yaml"));

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

/// 纯插值核心：`${name}` 占位替换（无全局状态，可单测——2026-08-20 从 t() 抽出）。
fn interpolate(template: &str, params: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in params {
        out = out.replace(&format!("${{{name}}}"), value);
    }
    out
}

/// flat key 查找 + ${name} 插值
pub fn t(key: &str, params: &[(&str, &str)]) -> String {
    let dict = DICT.lock();
    let template = dict.get(key).cloned().unwrap_or_else(|| key.to_string());
    drop(dict);
    interpolate(&template, params)
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

    /// 插值核心不触全局 DICT（2026-08-20 flake 根因修复）：原测试调 init("en") 污染
    /// 全局 DICT 不恢复——并行窗口内其他测试的中文 t() 断言挂（锚：
    /// test_collect_open_tabs_oversized_image_rejected 的「图片过大」）。改为本地
    /// dict_for + 纯 interpolate 直接断言，en/zh 双语 + 多参键全覆盖。
    #[test]
    fn test_t_interpolation() {
        let zh = dict_for("zh-CN");
        assert_eq!(
            interpolate(zh.get("editor.charCount").unwrap(), &[("n", "42")]),
            "42 字"
        );
        // 多参键（zh）："${n} 个文件打开失败：${detail}"
        assert_eq!(
            interpolate(zh.get("editor.openFailed").unwrap(), &[("n", "3"), ("detail", "a.md, b.md")]),
            "3 个文件打开失败：a.md, b.md"
        );
        let en = dict_for("en");
        assert_eq!(
            interpolate(en.get("editor.charCount").unwrap(), &[("n", "42")]),
            "42 chars"
        );
        assert_eq!(
            interpolate(en.get("editor.openFailed").unwrap(), &[("n", "3"), ("detail", "x")]),
            "3 file(s) failed to open: x"
        );
        // 无参不改动模板
        assert_eq!(interpolate(zh.get("editor.undo").unwrap(), &[]), "撤销");
    }

    /// 缺键回退与 DICT 状态无关（空/en/zh dict 均返回 key 原文）——不触全局。
    #[test]
    fn test_t_missing_key() {
        assert_eq!(t("nonexistent.key", &[]), "nonexistent.key");
    }
}
