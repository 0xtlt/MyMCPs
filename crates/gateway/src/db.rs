use mymcps_core::Db;
use sqlx::{Sqlite, Transaction};

/// Start the transaction in which a row that was read is then written. A
/// concurrent request may have had its turn between the two: the write says
/// how many rows it changed, and its caller checks that number.
pub(crate) async fn begin(db: &Db) -> Result<Transaction<'static, Sqlite>, sqlx::Error> {
    #[cfg(test)]
    concurrency::run_before_transaction().await;
    db.begin().await
}

/// Lets a test give a concurrent request its turn at the one place where it
/// matters: after a grant or a code was read, before the transaction that
/// writes it opens.
#[cfg(test)]
pub(crate) mod concurrency {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    type Action = Pin<Box<dyn Future<Output = ()> + Send>>;

    tokio::task_local! {
        static BEFORE_NEXT_TRANSACTION: Mutex<Option<Action>>;
    }

    pub(crate) async fn run_before_transaction() {
        let action = BEFORE_NEXT_TRANSACTION
            .try_with(|action| action.lock().ok().and_then(|mut action| action.take()))
            .ok()
            .flatten();
        if let Some(action) = action {
            action.await;
        }
    }

    /// Run `request`, and `action` just before the first transaction it opens.
    pub(crate) async fn before_next_transaction<T>(
        action: impl Future<Output = ()> + Send + 'static,
        request: impl Future<Output = T>,
    ) -> T {
        BEFORE_NEXT_TRANSACTION
            .scope(Mutex::new(Some(Box::pin(action))), request)
            .await
    }
}
