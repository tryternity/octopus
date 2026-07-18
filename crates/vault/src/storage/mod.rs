//! vault 存储层：把 types::Cipher 的加解密与 infra CRUD 结合。

pub mod cipher;
pub mod folder;
pub mod meta;

pub use cipher::{
    create_cipher, list_ciphers, load_cipher, permanent_delete, restore, save_cipher, soft_delete,
};
pub use folder::{create_folder, delete_folder, list_folders, rename_folder, FolderDto};
pub use meta::{read_vault_meta, save_vault_meta, update_security_stamp};
