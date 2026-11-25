use kartta::{EngineRefresh, Watch, Widened};
use serde::{Deserialize, Serialize};

use super::build::{StoredErrors, WatchView};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RefreshParams {
    pub(crate) id: Option<u64>,
    pub(crate) binding_id: Option<u64>,
    /// The root-relative paths that changed, `None` for the whole tree.
    pub(crate) paths: Option<Vec<String>>,
}

#[derive(Serialize)]
pub(crate) struct RefreshAnswer<'r> {
    walked: usize,
    tree: bool,
    added: usize,
    removed: usize,
    extracted: usize,
    resolved_again: usize,
    dropped: usize,
    reconfigured: &'r [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    widened: Option<WidenedView>,
    errors: &'r StoredErrors,
    watch: WatchView<'r>,
}

#[derive(Serialize)]
struct WidenedView {
    named: usize,
    parts: usize,
    limit: usize,
}

impl<'r> RefreshAnswer<'r> {
    /// What `refresh` did, `failed`, the failures it collected, and `watch`, what the engine's follow reports, `None`
    /// for a build that asked for none.
    pub(crate) fn new(
        refresh: &'r EngineRefresh,
        failed: &'r StoredErrors,
        watch: Option<&'r Watch>,
    ) -> Self {
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
            widened: refresh.widened.map(
                |Widened {
                     named,
                     parts,
                     limit,
                     ..
                 }| WidenedView {
                    named,
                    parts,
                    limit,
                },
            ),
            errors: failed,
            watch: WatchView::new(watch),
        }
    }
}
