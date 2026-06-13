// crates/llm/src/lib.rs

pub mod client;
pub mod config;
pub mod prompt;

pub use client::polish;
pub use config::CompatibleLlmConfig;
pub use prompt::set_system_prompt_override;
