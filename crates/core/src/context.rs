use std::sync::Arc;

use crate::config::Config;
use crate::crypto::Encryption;
use crate::db::Db;
use crate::error::Result;

/// What every part of the server shares: the configuration, the database and
/// the encryption key. Built once at startup, or once per test.
#[derive(Debug)]
pub struct Core {
    pub config: Config,
    pub db: Db,
    pub encryption: Encryption,
}

impl Core {
    /// Open the database named by the configuration and migrate it.
    pub async fn boot(config: Config) -> Result<Arc<Self>> {
        let db = Db::open(&config.database_path()).await?;
        for migration in db.migrate().await? {
            tracing::info!(migration, "Migrated the database");
        }
        Ok(Self::new(config, db))
    }

    pub fn new(config: Config, db: Db) -> Arc<Self> {
        let encryption = Encryption::new(&config.app_key);
        Arc::new(Self {
            config,
            db,
            encryption,
        })
    }

    /// Encrypt a secret column: see [`crate::secrets::encrypt_secret`].
    pub fn encrypt_secret(&self, value: Option<&str>) -> Option<String> {
        crate::secrets::encrypt_secret(&self.encryption, value)
    }

    /// Decrypt a secret column: see [`crate::secrets::decrypt_secret`].
    pub fn decrypt_secret(&self, value: Option<&str>) -> Option<String> {
        crate::secrets::decrypt_secret(&self.encryption, value)
    }
}

/// A [`Core`] on a fresh, migrated database in a temporary directory, for
/// tests in any crate. The directory is removed when this is dropped.
#[cfg(feature = "test-util")]
pub struct TestCore {
    pub core: Arc<Core>,
    pub dir: tempfile::TempDir,
}

#[cfg(feature = "test-util")]
impl TestCore {
    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    /// Like [`TestCore::new`], after adjusting the test configuration.
    pub async fn with_config(adjust: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory for the test database");
        let mut config = Config::for_tests(dir.path());
        adjust(&mut config);
        let core = Core::boot(config).await.expect("a migrated test database");
        Self { core, dir }
    }
}

#[cfg(feature = "test-util")]
impl std::ops::Deref for TestCore {
    type Target = Arc<Core>;

    fn deref(&self) -> &Arc<Core> {
        &self.core
    }
}
