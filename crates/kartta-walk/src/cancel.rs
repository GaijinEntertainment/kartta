//! [`CancelToken`] — the outside signal that ends a walk early.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A shared cancellation flag: one handle given to a walk, any number of clones held by whatever may stop it.
///
/// Cloning shares the flag rather than copying it, so cancelling any clone stops every walk holding one. A cancelled
/// walk returns the tree it built so far with [`WalkOutcome::cancelled`](crate::WalkOutcome::cancelled) set — cancellation truncates
/// an answer, it does not fail one.
///
/// What that partial tree holds depends on scheduling, so it is something to report or discard rather than build on. The
/// caller who cancelled is the one who knows that.
///
/// # Examples
///
/// ```
/// let token = kartta_walk::CancelToken::new();
/// let remote = token.clone();
///
/// assert!(!token.is_cancelled());
/// remote.cancel();
/// assert!(token.is_cancelled());
/// ```
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that has not been cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Every clone observes it, and a walk holding one stops at its next check.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation has been requested.
    ///
    /// The flag is read with relaxed ordering. It publishes no data, so a walker thread may notice the request an
    /// entry or two late; it cannot miss it.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
