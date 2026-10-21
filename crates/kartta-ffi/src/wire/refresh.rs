use std::path::Path;

use kartta::{EngineRefresh, Widened};
use serde::{Deserialize, Serialize};

use super::build::StoredErrors;
use crate::error::{Error, ErrorCode};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RefreshParams {
    pub(crate) id: Option<u64>,
    pub(crate) binding_id: Option<u64>,
    paths: Option<Vec<String>>,
}

impl RefreshParams {
    /// The root-relative paths that changed, `None` for the whole tree, or `invalid_params` when one is absolute; the
    /// engine refuses a path with a `..` component itself.
    pub(crate) fn paths(&self) -> Result<Option<&[String]>, Error> {
        if let Some(path) = self
            .paths
            .iter()
            .flatten()
            .find(|path| path.starts_with('/') || Path::new(path).is_absolute())
        {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                format!(
                    "cannot refresh {path:?}: a path is relative to the root, as the index's answers spell it"
                ),
            ));
        }
        Ok(self.paths.as_deref())
    }
}

#[derive(Serialize)]
pub(crate) struct RefreshAnswer<'r> {
    walked: usize,
    tree: bool,
    added: usize,
    removed: usize,
    extracted: usize,
    resolved_again: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    dropped: Option<usize>,
    reconfigured: &'r [String],
    rebuilt: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    widened: Option<WidenedView>,
    errors: &'r StoredErrors,
}

#[derive(Serialize)]
struct WidenedView {
    named: usize,
    parts: usize,
    limit: usize,
}

impl<'r> RefreshAnswer<'r> {
    /// What `refresh` did, and `failed`, the failures it collected.
    pub(crate) fn new(refresh: &'r EngineRefresh, failed: &'r StoredErrors) -> Self {
        let walked = &refresh.index.walked;
        Self {
            walked: walked.len(),
            tree: walked.iter().any(String::is_empty),
            added: refresh.index.added.len(),
            removed: refresh.index.removed.len(),
            extracted: refresh.extracted,
            resolved_again: refresh.resolved_again,
            dropped: refresh.dropped,
            reconfigured: &refresh.reconfigured,
            rebuilt: refresh.rebuilt,
            widened: refresh.widened.map(
                |Widened {
                     named,
                     parts,
                     limit,
                 }| WidenedView {
                    named,
                    parts,
                    limit,
                },
            ),
            errors: failed,
        }
    }
}
