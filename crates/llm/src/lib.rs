// crates/llm/src/lib.rs

pub mod client;
pub mod prompt;

pub use client::{polish, test_connection};
pub use octopus_infra::db::CompatibleLlmConfig;
pub use prompt::set_system_prompt_override;
