//! octopus-convert——文档转 Markdown 领域库（spec 2026-08-18-actionbar-markdown-conversion-design）。
//! 零项目内依赖（对齐 infra 惯例）。格式分派 / 单文件转换 / 多文件与文件夹合并。

pub mod error;

pub use error::ConvertError;
