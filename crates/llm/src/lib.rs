#![warn(clippy::all)]
// crates/llm/src/lib.rs

pub mod client;
pub mod prompt;

pub use client::{polish, polish_regions, test_connection, PolishRegion};
pub use octopus_infra::db::CompatibleLlmConfig;
pub use prompt::{build_system_prompt, set_system_prompt, system_prompt};
