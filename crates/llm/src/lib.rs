// crates/llm/src/lib.rs

pub mod client;
pub mod prompt;

pub use client::{polish, test_connection};
pub use octopus_infra::db::CompatibleLlmConfig;
pub use prompt::{build_system_prompt, set_system_prompt, system_prompt};
