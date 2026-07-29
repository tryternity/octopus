//! 密码库功能域。

#[cfg(feature = "vault")] pub mod vault_commands;
#[cfg(feature = "vault")] pub mod vault_state;
pub mod vault_secret_access;  // 总是编译（cloud 推理热路径用）
#[cfg(feature = "vault")] pub mod vault_error;
#[cfg(feature = "vault")] pub mod vault_sync_commands;
#[cfg(feature = "vault")] pub mod autotype;
#[cfg(feature = "vault")] pub mod password_generator_window;
