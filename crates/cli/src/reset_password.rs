//! `mymcps user:reset-password <email>`: recover an account from the server
//! itself, without its current password.

use mymcps_core::Core;
use mymcps_core::models::User;
use mymcps_web::validators::user::RESET_PASSWORD_VALIDATOR;
use serde_json::json;

/// Reset the password of the account with this email address, asking for the
/// new password twice through `prompt`. Passwords are never passed as
/// command-line arguments. `Ok` is what to print on success, `Err` what to
/// print before exiting with a failure.
pub async fn reset_password(
    core: &Core,
    email: &str,
    mut prompt: impl FnMut(&str) -> std::io::Result<String>,
) -> Result<String, String> {
    let lookup = User::find_by_email(&*core.db, email.trim()).await;
    let mut user = match lookup {
        Ok(Some(user)) => user,
        Ok(None) => return Err("No user found with that email address".into()),
        Err(error) => return Err(format!("The account could not be read: {error}")),
    };

    let mut ask = |label: &str| {
        prompt(label).map_err(|error| format!("The password could not be read: {error}"))
    };
    let new_password = ask("New password")?;
    let password_confirmation = ask("Confirm new password")?;

    let payload =
        json!({ "newPassword": new_password, "passwordConfirmation": password_confirmation });
    if RESET_PASSWORD_VALIDATOR.validate(&payload).is_err() {
        return Err("Passwords must match and contain between 8 and 32 characters".into());
    }

    user.change_password(&core.db, &new_password)
        .await
        .map_err(|error| format!("The password could not be changed: {error}"))?;
    Ok(format!(
        "Password reset for {}. Sessions and remember-me tokens revoked.",
        user.email
    ))
}

#[cfg(test)]
mod tests {
    use mymcps_core::models::UserRole;
    use mymcps_core::{TestCore, Timestamp};

    use super::*;

    async fn create_user(core: &Core, email: &str, role: UserRole) -> User {
        let mut user = User::with_password(email, Some("Test User"), "password123", role)
            .await
            .unwrap();
        user.insert(&*core.db).await.unwrap();
        user
    }

    async fn remember(core: &Core, user: &User) {
        let now = Timestamp::now().timestamp_millis();
        sqlx::query("insert into `remember_me_tokens` (`tokenable_id`, `hash`, `created_at`, `updated_at`, `expires_at`) values (?, hex(randomblob(32)), ?, ?, ?)")
            .bind(user.id)
            .bind(now)
            .bind(now)
            .bind(now + 1_000_000)
            .execute(&*core.db)
            .await
            .unwrap();
    }

    async fn remembered(core: &Core, user: &User) -> i64 {
        sqlx::query_scalar("select count(*) from `remember_me_tokens` where `tokenable_id` = ?")
            .bind(user.id)
            .fetch_one(&*core.db)
            .await
            .unwrap()
    }

    /// Answers the two prompts, and says which were asked.
    fn answers<'a>(
        password: &'a str,
        confirmation: &'a str,
        asked: &'a mut Vec<String>,
    ) -> impl FnMut(&str) -> std::io::Result<String> + 'a {
        move |label| {
            asked.push(label.to_string());
            Ok(if label == "New password" {
                password
            } else {
                confirmation
            }
            .to_string())
        }
    }

    #[tokio::test]
    async fn recovers_an_account_and_revokes_only_its_remember_me_tokens() {
        for role in [UserRole::Admin, UserRole::Member] {
            let core = TestCore::new().await;
            let user = create_user(&core, "lost@example.com", role).await;
            let other = create_user(&core, "other@example.com", UserRole::Member).await;
            remember(&core, &user).await;
            remember(&core, &user).await;
            remember(&core, &other).await;

            let mut asked = Vec::new();
            let message = reset_password(
                &core,
                " lost@example.com ",
                answers("new-password-123", "new-password-123", &mut asked),
            )
            .await
            .unwrap();

            assert_eq!(
                message,
                "Password reset for lost@example.com. Sessions and remember-me tokens revoked."
            );
            assert_eq!(asked, ["New password", "Confirm new password"]);
            let reloaded = User::find(&*core.db, user.id).await.unwrap().unwrap();
            assert_ne!(reloaded.password, "new-password-123");
            assert!(reloaded.verify_password("new-password-123").await.unwrap());
            assert!(!reloaded.verify_password("password123").await.unwrap());
            assert_eq!(reloaded.role, role);
            assert_eq!(reloaded.session_version, user.session_version + 1);
            assert_eq!(remembered(&core, &user).await, 0);
            assert_eq!(remembered(&core, &other).await, 1);
            assert!(
                User::verify_credentials(&core.db, "lost@example.com", "new-password-123")
                    .await
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[tokio::test]
    async fn rejects_an_unknown_account_without_asking_or_creating_a_user() {
        let core = TestCore::new().await;
        let mut asked = Vec::new();
        let error = reset_password(
            &core,
            "missing@example.com",
            answers("new-password-123", "new-password-123", &mut asked),
        )
        .await
        .unwrap_err();
        assert_eq!(error, "No user found with that email address");
        assert!(asked.is_empty());
        assert!(
            User::find_by_email(&*core.db, "missing@example.com")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn rejects_invalid_or_unconfirmed_passwords_without_changing_credentials() {
        let too_long = "x".repeat(33);
        for (password, confirmation) in [
            ("", ""),
            ("short", "short"),
            (too_long.as_str(), too_long.as_str()),
            ("new-password-123", "different-password"),
        ] {
            let core = TestCore::new().await;
            let user = create_user(&core, "user@example.com", UserRole::Member).await;
            remember(&core, &user).await;

            let mut asked = Vec::new();
            let error = reset_password(
                &core,
                "user@example.com",
                answers(password, confirmation, &mut asked),
            )
            .await
            .unwrap_err();

            assert_eq!(
                error,
                "Passwords must match and contain between 8 and 32 characters"
            );
            let reloaded = User::find(&*core.db, user.id).await.unwrap().unwrap();
            assert_eq!(reloaded.password, user.password);
            assert_eq!(reloaded.session_version, user.session_version);
            assert_eq!(remembered(&core, &user).await, 1);
        }
    }

    #[tokio::test]
    async fn rolls_back_the_password_when_remember_me_revocation_fails() {
        let core = TestCore::new().await;
        let user = create_user(&core, "user@example.com", UserRole::Member).await;
        remember(&core, &user).await;
        sqlx::query("CREATE TRIGGER fail_remember_token_delete BEFORE DELETE ON remember_me_tokens BEGIN SELECT RAISE(ABORT, 'simulated revocation failure'); END")
            .execute(&*core.db)
            .await
            .unwrap();

        let mut asked = Vec::new();
        let error = reset_password(
            &core,
            "user@example.com",
            answers("new-password-123", "new-password-123", &mut asked),
        )
        .await
        .unwrap_err();

        assert!(error.starts_with("The password could not be changed"));
        let reloaded = User::find(&*core.db, user.id).await.unwrap().unwrap();
        assert_eq!(reloaded.password, user.password);
        assert_eq!(reloaded.session_version, 1);
        assert_eq!(remembered(&core, &user).await, 1);
    }
}
