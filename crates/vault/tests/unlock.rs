//! vault::unlock 集成测试。
//!
//! ⚠️ 需要真实 ~/.octopus/octopus.db + OS Keychain 权限。
//! 默认 #[ignore]，需 --ignored 跑。
//! 测试会修改 ~/.octopus/octopus.db，建议在测试环境用。

use octopus_vault::unlock;

#[test]
#[ignore]
fn test_full_setup_unlock_cycle() {
    // 清理（如果之前有遗留）
    // 注意：实际项目应加 reset_vault() 工具函数

    // 1. setup_vault
    let keys = unlock::setup_vault("test-password-123").expect("setup");
    assert_eq!(keys.user_vault_key.as_bytes().len(), 32);
    assert_eq!(keys.app_key.as_bytes().len(), 32);

    // 2. 本机启动解锁（K_machine 已在 setup 时生成）
    let app_key_local = unlock::unlock_app_key_local()
        .expect("local unlock")
        .expect("应有 K_machine");
    assert_eq!(app_key_local.as_bytes(), keys.app_key.as_bytes());

    // 3. 主密码解锁（应该能拿到同样的 user_vault_key 和 app_key）
    let keys2 = unlock::unlock_with_master_password("test-password-123").expect("master unlock");
    assert_eq!(
        keys2.user_vault_key.as_bytes(),
        keys.user_vault_key.as_bytes()
    );
    assert_eq!(keys2.app_key.as_bytes(), keys.app_key.as_bytes());

    // 4. 错误密码应失败
    assert!(unlock::unlock_with_master_password("wrong-password").is_err());

    // 5. 改主密码
    unlock::change_master_password("test-password-123", "new-pwd-456").expect("change pwd");

    // 6. 旧密码失败，新密码成功
    assert!(unlock::unlock_with_master_password("test-password-123").is_err());
    let keys3 = unlock::unlock_with_master_password("new-pwd-456").expect("new pwd unlock");
    assert_eq!(
        keys3.user_vault_key.as_bytes(),
        keys.user_vault_key.as_bytes()
    ); // user_vault_key 不变
}
