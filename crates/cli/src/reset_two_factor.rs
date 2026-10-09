//! `mymcps user:reset-2fa <email>`: let a user who lost their passkeys, their
//! authenticator app and their recovery codes sign in with the password again.

use mymcps_core::Core;
use mymcps_core::models::User;
use mymcps_core::two_factor::reset_two_factor;

/// Remove the passkeys, the authenticator app and the recovery codes of the
/// account with this email address, and sign it out everywhere. `Ok` is what
/// to print on success, `Err` what to print before exiting with a failure.
pub async fn reset(core: &Core, email: &str) -> Result<String, String> {
    let lookup = User::find_by_email(&*core.db, email.trim()).await;
    let mut user = match lookup {
        Ok(Some(user)) => user,
        Ok(None) => return Err("No user found with that email address".into()),
        Err(error) => return Err(format!("The account could not be read: {error}")),
    };
    let summary = reset_two_factor(&core.db, &mut user)
        .await
        .map_err(|error| format!("Two-step verification could not be reset: {error}"))?;

    let mut removed = Vec::new();
    match summary.passkeys {
        0 => {}
        1 => removed.push("1 passkey".to_string()),
        count => removed.push(format!("{count} passkeys")),
    }
    if summary.totp {
        removed.push("the authenticator app".into());
    }
    match summary.recovery_codes {
        0 => {}
        1 => removed.push("1 recovery code".to_string()),
        count => removed.push(format!("{count} recovery codes")),
    }
    let removed = match removed.as_slice() {
        [] => "nothing to remove".to_string(),
        [only] => format!("removed {only}"),
        [first @ .., last] => format!("removed {} and {last}", first.join(", ")),
    };
    Ok(format!(
        "Two-step verification reset for {}: {removed}. The password alone signs in again. Sessions and remember-me tokens revoked.",
        user.email
    ))
}

#[cfg(test)]
mod tests {
    use mymcps_core::models::{UserPasskey, UserRole, UserTotpSecret};
    use mymcps_core::two_factor::TwoFactorStatus;
    use mymcps_core::{TestCore, Timestamp};
    use sqlx::Acquire;

    use super::*;

    async fn create_user(core: &Core, email: &str) -> User {
        let mut user = User::with_password(email, None, "password123", UserRole::Member)
            .await
            .unwrap();
        user.insert(&*core.db).await.unwrap();
        user
    }

    async fn protect(core: &Core, user: &User, passkeys: usize) {
        UserTotpSecret {
            user_id: user.id,
            secret: core.encryption.encrypt("JBSWY3DPEHPK3PXP"),
            confirmed_at: Some(Timestamp::now()),
            ..Default::default()
        }
        .save(&*core.db)
        .await
        .unwrap();
        for index in 0..passkeys {
            UserPasskey {
                user_id: user.id,
                name: format!("Key {index}"),
                credential_id: format!("credential-{}-{index}", user.id),
                user_handle: "00000000-0000-4000-8000-000000000000".into(),
                passkey: "{}".into(),
                ..Default::default()
            }
            .insert(&*core.db)
            .await
            .unwrap();
        }
        let mut connection = core.db.acquire().await.unwrap();
        let mut transaction = connection.begin().await.unwrap();
        let hashes: Vec<String> = (0..10).map(|index| format!("hash-{index}")).collect();
        mymcps_core::models::UserRecoveryCode::replace_all(&mut transaction, user.id, &hashes)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
    }

    #[tokio::test]
    async fn removes_every_factor_of_the_account_only() {
        let core = TestCore::new().await;
        let user = create_user(&core, "locked@example.com").await;
        let other = create_user(&core, "other@example.com").await;
        protect(&core, &user, 2).await;
        protect(&core, &other, 1).await;

        let message = reset(&core, " locked@example.com ").await.unwrap();
        assert_eq!(
            message,
            "Two-step verification reset for locked@example.com: removed 2 passkeys, the authenticator app and 10 recovery codes. The password alone signs in again. Sessions and remember-me tokens revoked."
        );
        let status = TwoFactorStatus::of(&core.db, user.id).await.unwrap();
        assert!(!status.is_enabled());
        assert_eq!(status.recovery_codes, 0);
        let reloaded = User::find(&*core.db, user.id).await.unwrap().unwrap();
        assert_eq!(reloaded.session_version, user.session_version + 1);
        assert!(reloaded.verify_password("password123").await.unwrap());

        let untouched = TwoFactorStatus::of(&core.db, other.id).await.unwrap();
        assert_eq!(untouched.passkeys, 1);
        assert!(untouched.totp);
        assert_eq!(untouched.recovery_codes, 10);
    }

    #[tokio::test]
    async fn says_when_there_was_nothing_to_remove() {
        let core = TestCore::new().await;
        create_user(&core, "plain@example.com").await;
        let message = reset(&core, "plain@example.com").await.unwrap();
        assert!(message.contains(": nothing to remove."), "{message}");
    }

    #[tokio::test]
    async fn rejects_an_unknown_account() {
        let core = TestCore::new().await;
        let error = reset(&core, "missing@example.com").await.unwrap_err();
        assert_eq!(error, "No user found with that email address");
    }
}
