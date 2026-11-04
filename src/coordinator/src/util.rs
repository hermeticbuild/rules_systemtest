//! Small concurrency utilities.

use futures::Future;

/// Awaits all futures and returns the first error, if any.
pub async fn join_all_ignoring_oks<I, R, E>(futures: I) -> Result<(), E>
where
    I: IntoIterator,
    I::Item: Future<Output = Result<R, E>>,
{
    for result in futures::future::join_all(futures).await {
        result?;
    }
    Ok(())
}

/// A one-shot broadcast used to coordinate task-loop shutdown: once
/// [`notify`](Condvar::notify) fires, every current and future
/// [`wait`](Condvar::wait) returns.
#[derive(Default)]
pub struct Condvar {
    notify: tokio::sync::Notify,
    fired: tokio::sync::Mutex<bool>,
}

impl Condvar {
    pub fn new() -> Condvar {
        Condvar::default()
    }

    pub async fn wait(&self) {
        loop {
            // Copy the flag out rather than holding the lock across the await.
            let fired = *self.fired.lock().await;
            if fired {
                // Ensure another waiter also wakes.
                self.notify.notify_one();
                return;
            }
            self.notify.notified().await;
        }
    }

    pub async fn notify(&self) {
        *self.fired.lock().await = true;
        self.notify.notify_waiters();
        self.notify.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn wait_returns_after_notify() {
        let cv = Condvar::new();
        cv.notify().await;
        cv.wait().await;
    }

    #[tokio::test]
    async fn join_surfaces_first_error() {
        type BoxFut =
            std::pin::Pin<Box<dyn std::future::Future<Output = Result<i32, &'static str>> + Send>>;
        let futs: Vec<BoxFut> = vec![Box::pin(async { Ok(1) }), Box::pin(async { Err("boom") })];
        assert!(join_all_ignoring_oks(futs).await.is_err());
    }
}
