use std::fmt;

/// 转换错误——Display 文案是用户直接看到的 toast（spec §6 错误处理表）。
#[derive(Debug)]
pub enum ConvertError {
    UnsupportedFormat(String),
    Anydoc(String),
    /// 网络抓取（`web::fetch_page`）裸消息——Display **不加前缀**，由编排层
    /// （desktop markdown.rs `convert_and_save_url_with`）统一叠「抓取失败: 」
    /// （spec 2026-08-18-url-to-markdown §9⑭ 前缀去重——原 `Html` 变体的
    /// 「HTML 转换失败: 」前缀与编排层叠加成双重前缀，且 fetch_page 是其唯一使用者，已删）。
    Web(String),
    Io(std::io::Error),
    TooManyFiles { count: usize, max: usize },
    TooLarge { bytes: u64, max_bytes: u64 },
    Empty,
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat(ext) => write!(f, "暂不支持 .{} 格式", ext),
            Self::Anydoc(e) => write!(f, "文档转换失败: {}", e),
            Self::Web(msg) => write!(f, "{}", msg),
            Self::Io(e) => write!(f, "文件读取失败: {}", e),
            Self::TooManyFiles { count, max } => {
                write!(f, "{} 个文件超出上限（最多 {} 个），请缩小范围", count, max)
            }
            Self::TooLarge { bytes, max_bytes } => write!(
                f,
                "{:.1}MB 超出上限（最多 {:.0}MB），请缩小范围",
                *bytes as f64 / 1024.0 / 1024.0,
                *max_bytes as f64 / 1024.0 / 1024.0
            ),
            Self::Empty => write!(f, "没有可转换的内容"),
        }
    }
}

impl std::error::Error for ConvertError {}

impl From<std::io::Error> for ConvertError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_unsupported_format() {
        assert_eq!(
            ConvertError::UnsupportedFormat("png".into()).to_string(),
            "暂不支持 .png 格式"
        );
    }

    #[test]
    fn test_display_too_many_files() {
        let e = ConvertError::TooManyFiles { count: 233, max: 200 }.to_string();
        assert!(e.contains("233 个文件超出上限"));
        assert!(e.contains("200 个"));
    }

    #[test]
    fn test_display_too_large_mb() {
        let e = ConvertError::TooLarge { bytes: 60 * 1024 * 1024, max_bytes: 50 * 1024 * 1024 }.to_string();
        assert!(e.starts_with("60.0MB 超出上限"));
        assert!(e.contains("50MB"));
    }

    #[test]
    fn test_display_empty() {
        assert_eq!(ConvertError::Empty.to_string(), "没有可转换的内容");
    }

    #[test]
    fn test_display_web_bare_no_prefix() {
        // §9⑭ 前缀去重：Web 变体 Display 裸消息——前缀由编排层（markdown.rs）唯一叠加
        assert_eq!(ConvertError::Web("HTTP 404".into()).to_string(), "HTTP 404");
        assert_eq!(
            ConvertError::Web("该 URL 不是 HTML 页面".into()).to_string(),
            "该 URL 不是 HTML 页面"
        );
    }

    #[test]
    fn test_from_io_error() {
        let e: ConvertError = std::io::Error::new(std::io::ErrorKind::NotFound, "gone").into();
        assert!(e.to_string().contains("文件读取失败"));
    }
}
