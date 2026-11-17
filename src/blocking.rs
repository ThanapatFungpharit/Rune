//! Running synchronous file and database work off the async executor.

/// Run `f` on tokio's blocking pool and wait for it. A panic in `f` is
/// re-raised here, as if `f` had been called directly. Dropping the future
/// does not stop `f`: work that has started runs to its end, so a write is
/// never left half done by a cancelled caller.
pub(crate) async fn run<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(v) => v,
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        // Only when the runtime shuts down, which drops this future too.
        Err(e) => panic!("blocking task did not finish: {e}"),
    }
}
