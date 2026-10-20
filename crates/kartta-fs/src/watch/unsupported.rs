use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Found, WatchError};

#[derive(Debug)]
pub(super) enum Backend {}

impl Backend {
    pub(super) fn start(root: &Path, _prefixes: &[(PathBuf, String)]) -> Result<Self, WatchError> {
        Err(WatchError::Unsupported {
            native_path: root.to_path_buf(),
        })
    }

    pub(super) fn poll(&self, _timeout: Duration, _found: &mut Found) -> Result<bool, WatchError> {
        match *self {}
    }

    pub(super) fn queued(&self) -> Option<usize> {
        match *self {}
    }

    pub(super) fn descending(&self) -> Option<fn(&Path)> {
        match *self {}
    }

    pub(super) fn descended(&mut self) -> Result<(), WatchError> {
        match *self {}
    }
}
