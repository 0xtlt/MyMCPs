//! `mymcps user:reset-protection <email>`: let an account sign in with its
//! password alone again, from the server itself. For someone who lost
//! their authenticator app, their passkeys and their recovery codes, and
//! whom no super admin can help: the only super admin, for one.

use mymcps_core::Core;
use mymcps_core::models::User;

/// Remove the authenticator app, the passkeys and the recovery codes of
/// the account with this email address. `Ok` is what to print on success,
/// `Err` what to print before exiting with a failure.
pub async fn reset(core: &Core, email: &str) -> Result<String, String> {
    fn failed(error: impl std::fmt::Display) -> String {
        format!("The account could not be changed: {error}")
    }
    let mut user = match User::find_by_email(&*core.db, email.trim()).await {
        Ok(Some(user)) => user,
        Ok(None) => return Err("No user found with that email address".into()),
        Err(error) => return Err(format!("The account could not be read: {error}")),
    };

    let mut transaction = core.db.begin().await.map_err(failed)?;
    user.remove_protection(&mut transaction)
        .await
        .map_err(failed)?;
    transaction.commit().await.map_err(failed)?;
    Ok(format!(
        "Sign-in protection removed for {}. Its password signs in alone.",
        user.email
    ))
}

#[cfg(test)]
mod tests {
    use mymcps_core::TestCore;
    use mymcps_core::crypto::totp;
    use mymcps_core::models::{Passkey, RecoveryCode};

    use super::*;

    #[tokio::test]
    async fn removes_everything_an_account_asks_for_besides_its_password() {
        let core = TestCore::new().await;
        let db = &*core.db;
        let mut user = User::with_password("lost@example.com", None, "password123")
            .await
            .unwrap();
        let mut other = User::with_password("other@example.com", None, "password123")
            .await
            .unwrap();
        for account in [&mut user, &mut other] {
            account.insert(db).await.unwrap();
            let mut transaction = core.db.begin().await.unwrap();
            account
                .enable_authenticator(
                    &mut transaction,
                    &core.encryption,
                    &totp::generate_secret(),
                    1,
                )
                .await
                .unwrap();
            RecoveryCode::replace_all(&mut transaction, account.id)
                .await
                .unwrap();
            transaction.commit().await.unwrap();
            let mut passkey = Passkey {
                user_id: account.id,
                name: "Key".into(),
                credential_id: format!("credential-{}", account.id),
                public_key: "key".into(),
                ..Default::default()
            };
            passkey.insert(db).await.unwrap();
        }

        assert_eq!(
            reset(&core, " lost@example.com ").await.as_deref(),
            Ok("Sign-in protection removed for lost@example.com. Its password signs in alone.")
        );
        let reloaded = User::find(db, user.id).await.unwrap().unwrap();
        assert!(!reloaded.has_authenticator() && reloaded.two_factor_secret.is_none());
        assert!(!reloaded.is_protected(db).await.unwrap());
        assert_eq!(RecoveryCode::remaining(db, user.id).await.unwrap(), 0);
        assert!(reloaded.verify_password("password123").await.unwrap());
        // Nobody else lost anything.
        let untouched = User::find(db, other.id).await.unwrap().unwrap();
        assert!(untouched.has_authenticator());
        assert_eq!(Passkey::count_of_user(db, other.id).await.unwrap(), 1);
        assert_eq!(RecoveryCode::remaining(db, other.id).await.unwrap(), 8);

        assert_eq!(
            reset(&core, "nobody@example.com").await.unwrap_err(),
            "No user found with that email address"
        );
    }
}
