//! vault_meta 表的薄包装（直接转发 infra）。

use anyhow::Result;
use octopus_infra::db::{self, VaultMeta, VaultMetaInput};

pub fn read_vault_meta() -> Result<Option<VaultMeta>> {
    Ok(db::load_vault_meta()?)
}

pub fn save_vault_meta(input: &VaultMetaInput) -> Result<()> {
    Ok(db::upsert_vault_meta(input)?)
}

pub fn update_security_stamp(stamp: &str) -> Result<()> {
    Ok(db::update_vault_security_stamp(stamp)?)
}
