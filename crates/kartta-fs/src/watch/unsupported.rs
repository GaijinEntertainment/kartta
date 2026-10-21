use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Found, WatchError};

#[derive(Debug)]
pub(super) struct Backend(Infallible);

impl Backend {
    pub(super) fn start(root: &Path, _prefixes: &[(PathBuf, String)]) -> Result<Self, WatchError> {
        Err(WatchError::Unsupported {
            native_path: root.to_path_buf(),
        })
    }

    pub(super) const fn poll(
        &self,
        _timeout: Duration,
        _found: &mut Found,
    ) -> Result<bool, WatchError> {
        match self.0 {}
    }

    pub(super) const fn queued(&self) -> usize {
        match self.0 {}
    }

    pub(super) const fn descending(&self) -> Option<fn(&Path)> {
        match self.0 {}
    }

    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "one signature for every platform, and no value of this backend exists, so no body runs"
    )]
    pub(super) const fn descended(&mut self) -> Result<(), WatchError> {
        match self.0 {}
    }

    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "one signature for every platform, and no value of this backend exists, so no body runs"
    )]
    pub(super) const fn release(&mut self, _dirs: &[String]) {
        match self.0 {}
    }
}
