use std::collections::HashMap;
use std::ops::Range;

use kartta_lang::{LocalCall, RawExtraction, SymbolId};

/// A local and the span its name is in scope over: its function for a `var`, its block otherwise.
type ScopedLocal = (Range<usize>, SymbolId);

/// The variables and constants the bodies of one top-level function declare, and the bare calls in them that no binding
/// covered where they were written, which a declaration later in a scope around them may still bind.
#[derive(Default)]
pub(super) struct Locals<'s> {
    scopes: HashMap<&'s [u8], Vec<ScopedLocal>>,
    deferred: Vec<(usize, usize)>,
}

impl<'s> Locals<'s> {
    pub(super) fn declare(&mut self, name: &'s [u8], scope: Range<usize>, local: SymbolId) {
        self.scopes.entry(name).or_default().push((scope, local));
    }

    /// Defers the call at `call` in the extraction's calls, written at byte `at`.
    pub(super) fn defer(&mut self, call: usize, at: usize) {
        self.deferred.push((call, at));
    }

    // constraint: a body runs its closures after its declarations, and a `var` belongs to its whole function, so a call
    // of a name that the innermost scope around it declares later calls that local
    pub(super) fn settle(&mut self, raw: &mut RawExtraction) {
        let mut bound = Vec::new();
        for &(call, at) in &self.deferred {
            let written = &raw.calls[call];
            let declared = self.scopes.get(written.callee.as_bytes());
            let innermost = (declared.into_iter().flatten())
                .filter(|(scope, _)| scope.contains(&at))
                .min_by_key(|(scope, _)| scope.len());
            if let Some(&(_, local)) = innermost {
                raw.local_calls.push(LocalCall {
                    site_of: written.site_of,
                    local,
                    line: written.line,
                });
                bound.push(call);
            }
        }
        if !bound.is_empty() {
            let (mut at, mut bound) = (0, bound.into_iter().peekable());
            raw.calls.retain(|_| {
                let keep = bound.next_if_eq(&at).is_none();
                at += 1;
                keep
            });
        }
        self.scopes.clear();
        self.deferred.clear();
    }
}
