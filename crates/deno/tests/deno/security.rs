//! The case of `tests/unit/security.spec.ts` about the Deno runner.

use crate::support::Sandbox;

mod security_boundaries {
    use super::*;

    #[tokio::test]
    async fn redacts_long_npm_environment_secrets_before_truncating_startup_errors() {
        let sandbox = Sandbox::new().await;
        let secret = format!("opaque-{}-tail", "x".repeat(400));
        let mut mcp = sandbox.npm_mcp(&[("OPAQUE_VALUE", &secret)]);
        mcp.npm_package = Some("@example/failing-mcp".to_owned());

        let error =
            sandbox
                .runner()
                .create_startup_error(&mcp, &format!("startup echoed {secret}"), None);

        let message = error.to_string();
        assert!(message.contains("[REDACTED]"), "{message}");
        assert!(!message.contains(&secret));
        assert!(!message.contains(&secret[..300]));
    }
}
