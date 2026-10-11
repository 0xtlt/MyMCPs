//! Work several callers wait for and none of them owns: the renewal of the
//! OAuth tokens of an MCP, a run of npm updates.

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::sync::oneshot;

use crate::error::UpstreamError;

/// The outcome of a task, for every caller that waits for it.
pub(crate) type SharedOutcome<T> = Shared<BoxFuture<'static, Result<T, UpstreamError>>>;

/// What the callers of a task wait on, and what the task hands its outcome
/// to. A task that ends without an outcome, because it panicked or the
/// server is stopping, fails its callers with `interrupted`.
///
/// The task is started after the callers can find what they wait on, and
/// outside of the lock that guards it: a task that cannot start is dropped at
/// once, and takes that lock to clear its entry.
pub(crate) fn outcome_channel<T: Clone + Send + Sync + 'static>(
    interrupted: &'static str,
) -> (oneshot::Sender<Result<T, UpstreamError>>, SharedOutcome<T>) {
    let (sender, receiver) = oneshot::channel();
    let outcome = async move {
        receiver
            .await
            .unwrap_or_else(|_| Err(UpstreamError::other(interrupted)))
    }
    .boxed()
    .shared();
    (sender, outcome)
}
