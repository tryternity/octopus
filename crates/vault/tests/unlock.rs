//! vault::unlock 集成测试。
//!
//! 走 thread-local in-memory DB（`set_test_db`）+ in-memory Keychain
//! （`set_test_keychain`）覆盖，无需 OS Keychain / 真实 ~/.octopus/octopus.db。
//! 测试完全隔离，可安全地在 CI 上跑。

use octopus_vault::keychain;
use octopus_vault::unlock;
use octopus_vault::Zeroizing;

#[test]
fn test_full_setup_unlock_cycle() {
    // 注入 in-memory DB + in-memory Keychain，与单元测试同构。
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory DB");
    octopus_infra::db::set_test_db(conn);
    keychain::set_test_keychain();

    // 1. setup_vault
    // H1 修复（d98ad3f7）：主密码入口改 Zeroizing<String> 所有权转移，调用方需包 Zeroizing::new。
    let keys = unlock::setup_vault(Zeroizing::new("Test-password-123".into()))
        .expect("setup");
    assert_eq!(keys.user_vault_key.as_bytes().len(), 32);
    assert_eq!(keys.app_key.as_bytes().len(), 32);

    // 2. 本机启动解锁（K_machine 已在 setup 时生成）
    let app_key_local = unlock::unlock_app_key_local()
        .expect("local unlock")
        .expect("应有 K_machine");
    assert_eq!(app_key_local.as_bytes(), keys.app_key.as_bytes());

    // 3. 主密码解锁（应该能拿到同样的 user_vault_key 和 app_key）
    let keys2 = unlock::unlock_with_master_password(Zeroizing::new(
        "Test-password-123".into(),
    ))
    .expect("master unlock");
    assert_eq!(
        keys2.user_vault_key.as_bytes(),
        keys.user_vault_key.as_bytes()
    );
    assert_eq!(keys2.app_key.as_bytes(), keys.app_key.as_bytes());

    // 4. 错误密码应失败
    assert!(
        unlock::unlock_with_master_password(Zeroizing::new("Wrong-password-1!".into())).is_err()
    );

    // 5. 改主密码
    unlock::change_master_password(
        Zeroizing::new("Test-password-123".into()),
        Zeroizing::new("New-pwd-456!".into()),
    )
    .expect("change pwd");

    // 6. 旧密码失败，新密码成功
    assert!(unlock::unlock_with_master_password(Zeroizing::new(
        "Test-password-123".into()
    ))
    .is_err());
    let keys3 = unlock::unlock_with_master_password(Zeroizing::new("New-pwd-456!".into()))
        .expect("new pwd unlock");
    assert_eq!(
        keys3.user_vault_key.as_bytes(),
        keys.user_vault_key.as_bytes()
    ); // user_vault_key 不变
}
