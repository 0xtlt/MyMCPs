use mymcps_core::models::{User, UserPasskey, UserRecoveryCode, UserRole, UserTotpSecret};
use mymcps_core::two_factor::{
    TotpKey, TwoFactorStatus, forget_recovery_codes_if_unprotected, generate_recovery_codes,
    hash_recovery_code, reset_two_factor,
};
use mymcps_core::{Config, Core, Db, Timestamp};

async fn core() -> (std::sync::Arc<Core>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::boot(Config::for_tests(dir.path())).await.unwrap();
    (core, dir)
}

async fn member(db: &Db) -> User {
    let mut user = User::with_password("ada@example.com", None, "password123", UserRole::Member)
        .await
        .unwrap();
    user.insert(&**db).await.unwrap();
    user
}

async fn passkey(db: &Db, user: &User, credential_id: &str) -> UserPasskey {
    let mut passkey = UserPasskey {
        user_id: user.id,
        name: "Laptop".into(),
        credential_id: credential_id.into(),
        user_handle: "6f1d1d36-4a4e-4f4e-8a35-0b3f5a5b1c11".into(),
        passkey: "{}".into(),
        ..Default::default()
    };
    passkey.insert(&**db).await.unwrap();
    passkey
}

#[tokio::test]
async fn a_totp_step_is_accepted_once() {
    let (core, _dir) = core().await;
    let user = member(&core.db).await;
    let key = TotpKey::generate(&user.email);
    let mut secret = UserTotpSecret {
        user_id: user.id,
        secret: core.encryption.encrypt(&key.base32()),
        confirmed_at: Some(Timestamp::now()),
        ..Default::default()
    };
    secret.insert(&*core.db).await.unwrap();

    let stored = UserTotpSecret::for_user(&*core.db, user.id)
        .await
        .unwrap()
        .unwrap();
    let decrypted = core.encryption.decrypt(&stored.secret).unwrap();
    assert_eq!(decrypted, key.base32());
    assert_ne!(stored.secret, key.base32(), "encrypted at rest");

    assert!(secret.consume_step(&*core.db, 100).await.unwrap());
    assert!(!secret.consume_step(&*core.db, 100).await.unwrap());
    assert!(!secret.consume_step(&*core.db, 99).await.unwrap());
    assert!(secret.consume_step(&*core.db, 101).await.unwrap());
    assert_eq!(secret.last_used_step, Some(101));
}

#[tokio::test]
async fn a_recovery_code_is_spent_once() {
    let (core, _dir) = core().await;
    let user = member(&core.db).await;
    let codes = generate_recovery_codes();
    let hashes: Vec<String> = codes.iter().map(|code| hash_recovery_code(code)).collect();
    let mut connection = core.db.acquire().await.unwrap();
    UserRecoveryCode::replace_all(&mut connection, user.id, &hashes)
        .await
        .unwrap();
    drop(connection);
    assert_eq!(
        UserRecoveryCode::remaining(&*core.db, user.id)
            .await
            .unwrap(),
        10
    );

    let hash = hash_recovery_code(&codes[3].to_uppercase());
    assert!(
        UserRecoveryCode::redeem(&*core.db, user.id, &hash)
            .await
            .unwrap()
    );
    assert!(
        !UserRecoveryCode::redeem(&*core.db, user.id, &hash)
            .await
            .unwrap()
    );
    assert!(
        !UserRecoveryCode::redeem(&*core.db, user.id + 1, &hash_recovery_code(&codes[4]))
            .await
            .unwrap(),
        "another user's code"
    );
    assert_eq!(
        UserRecoveryCode::remaining(&*core.db, user.id)
            .await
            .unwrap(),
        9
    );
}

#[tokio::test]
async fn the_status_and_the_reset_cover_every_factor() {
    let (core, _dir) = core().await;
    let mut user = member(&core.db).await;
    assert!(
        !TwoFactorStatus::of(&core.db, user.id)
            .await
            .unwrap()
            .is_enabled()
    );

    // An unconfirmed authenticator app does not count.
    let mut secret = UserTotpSecret {
        user_id: user.id,
        secret: core.encryption.encrypt("JBSWY3DPEHPK3PXP"),
        ..Default::default()
    };
    secret.insert(&*core.db).await.unwrap();
    assert!(
        !TwoFactorStatus::of(&core.db, user.id)
            .await
            .unwrap()
            .is_enabled()
    );
    secret.confirmed_at = Some(Timestamp::now());
    secret.save(&*core.db).await.unwrap();
    passkey(&core.db, &user, "credential-a").await;
    let mut connection = core.db.acquire().await.unwrap();
    UserRecoveryCode::replace_all(&mut connection, user.id, &["a".repeat(64), "b".repeat(64)])
        .await
        .unwrap();
    drop(connection);
    assert_eq!(
        TwoFactorStatus::of(&core.db, user.id).await.unwrap(),
        TwoFactorStatus {
            passkeys: 1,
            totp: true,
            recovery_codes: 2
        }
    );

    let version = user.session_version;
    let summary = reset_two_factor(&core.db, &mut user).await.unwrap();
    assert_eq!(summary.passkeys, 1);
    assert!(summary.totp);
    assert_eq!(summary.recovery_codes, 2);
    assert_eq!(user.session_version, version + 1);
    assert_eq!(
        TwoFactorStatus::of(&core.db, user.id).await.unwrap(),
        TwoFactorStatus::default()
    );
}

#[tokio::test]
async fn recovery_codes_go_with_the_last_factor() {
    let (core, _dir) = core().await;
    let user = member(&core.db).await;
    let first = passkey(&core.db, &user, "credential-a").await;
    let second = passkey(&core.db, &user, "credential-b").await;
    let mut connection = core.db.acquire().await.unwrap();
    UserRecoveryCode::replace_all(&mut connection, user.id, &["a".repeat(64)])
        .await
        .unwrap();

    first.delete(&mut *connection).await.unwrap();
    forget_recovery_codes_if_unprotected(&mut connection, user.id)
        .await
        .unwrap();
    assert_eq!(
        UserRecoveryCode::remaining(&mut *connection, user.id)
            .await
            .unwrap(),
        1
    );

    second.delete(&mut *connection).await.unwrap();
    forget_recovery_codes_if_unprotected(&mut connection, user.id)
        .await
        .unwrap();
    assert_eq!(
        UserRecoveryCode::remaining(&mut *connection, user.id)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn deleting_a_user_deletes_their_factors() {
    let (core, _dir) = core().await;
    let user = member(&core.db).await;
    passkey(&core.db, &user, "credential-a").await;
    sqlx::query("delete from `users` where `id` = ?")
        .bind(user.id)
        .execute(&*core.db)
        .await
        .unwrap();
    assert!(
        UserPasskey::find_by_credential_id(&*core.db, "credential-a")
            .await
            .unwrap()
            .is_none()
    );
}
