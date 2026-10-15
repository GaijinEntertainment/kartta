use std::path::PathBuf;
use std::time::Duration;

use super::{Found, WatchError};
use crate::index::Index;

#[derive(Debug)]
pub(super) enum Backend {}

impl Backend {
    pub(super) fn start(index: &Index, _watched: &[PathBuf]) -> Result<Self, WatchError> {
        Err(WatchError::Unsupported {
            native_path: index.root().to_path_buf(),
        })
    }

    pub(super) fn poll(&self, _timeout: Duration, _found: &mut Found) -> Result<bool, WatchError> {
        match *self {}
    }
}
