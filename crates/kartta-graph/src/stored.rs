use std::ops::Range;

use kartta_lang::{CallableRole, SymbolFlags, SymbolKind};

use crate::strings::Str;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SymbolAt {
    pub(crate) file: u32,
    pub(crate) index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Site {
    pub(crate) file: u32,
    entry: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entry {
    Call(u32),
    LocalCall(u32),
}

impl Site {
    const LOCAL: u32 = 1 << 31;

    pub(crate) const fn new(file: u32, entry: Entry) -> Self {
        Self {
            file,
            entry: match entry {
                Entry::Call(index) => index,
                Entry::LocalCall(index) => index | Self::LOCAL,
            },
        }
    }

    pub(crate) const fn entry(self) -> Entry {
        if self.entry & Self::LOCAL == 0 {
            Entry::Call(self.entry)
        } else {
            Entry::LocalCall(self.entry & !Self::LOCAL)
        }
    }
}

#[derive(Debug)]
pub(crate) struct StoredFile {
    pub(crate) path: Str,
    pub(crate) dir: Str,
    pub(crate) parent_dir: Str,
    pub(crate) package: Option<Str>,
    pub(crate) symbols: Box<[StoredSymbol]>,
    pub(crate) parents: Box<[Parent]>,
    pub(crate) locals: Box<[Local]>,
    pub(crate) callables: Box<[StoredCallable]>,
    pub(crate) folded: Box<[Folded]>,
    // constraint: both sorted by caller, in source order within one, so `calls_by` and `local_calls_by` find a body's
    // calls without a scan
    pub(crate) calls: Box<[StoredCall]>,
    pub(crate) local_calls: Box<[StoredLocalCall]>,
    pub(crate) includes: Box<[Str]>,
    pub(crate) re_exports: Box<[StoredReExport]>,
    pub(crate) bindings: Box<[StoredBinding]>,
}

impl StoredFile {
    pub(crate) fn holder(&self, index: u32) -> Option<u32> {
        let at = self
            .locals
            .binary_search_by_key(&index, |local| local.symbol)
            .ok()?;
        Some(self.locals[at].holder)
    }

    pub(crate) fn callable(&self, index: u32) -> Option<&StoredCallable> {
        let at = self
            .callables
            .binary_search_by_key(&index, |callable| callable.symbol)
            .ok()?;
        Some(&self.callables[at])
    }

    pub(crate) fn folded_lines(&self, index: u32) -> impl Iterator<Item = u32> + Clone + '_ {
        let start = self
            .folded
            .partition_point(|folded| folded.definition < index);
        self.folded[start..]
            .iter()
            .take_while(move |folded| folded.definition == index)
            .map(|folded| folded.line)
    }

    pub(crate) fn calls_by(&self, caller: u32) -> Range<usize> {
        caller_range(&self.calls, caller, |call| call.caller())
    }

    pub(crate) fn local_calls_by(&self, caller: u32) -> Range<usize> {
        caller_range(&self.local_calls, caller, |call| call.caller)
    }

    pub(crate) fn parents_of(&self, index: u32) -> &[Parent] {
        let start = self.parents.partition_point(|parent| parent.symbol < index);
        let len = self.parents[start..].partition_point(|parent| parent.symbol == index);
        &self.parents[start..start + len]
    }

    pub(crate) fn package_stamp(&self) -> Option<Str> {
        self.package
            .filter(|_| self.symbols.iter().any(|symbol| !symbol.flags.is_test()))
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredSymbol {
    pub(crate) name: Str,
    pub(crate) qualified_name: Str,
    pub(crate) owner: Option<Str>,
    pub(crate) signature: Option<Str>,
    pub(crate) line: u32,
    pub(crate) end_line: u32,
    pub(crate) kind: SymbolKind,
    pub(crate) flags: SymbolFlags,
    pub(crate) exported: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Parent {
    pub(crate) symbol: u32,
    pub(crate) written: Str,
    pub(crate) written_from_root: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Local {
    pub(crate) symbol: u32,
    pub(crate) holder: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredCallable {
    pub(crate) symbol: u32,
    pub(crate) key: Str,
    pub(crate) role: CallableRole,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Folded {
    pub(crate) definition: u32,
    pub(crate) line: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredCall {
    caller: u32,
    pub(crate) callee: Str,
    pub(crate) receiver: Option<Str>,
    pub(crate) line: u32,
}

impl StoredCall {
    const FROM_ROOT: u32 = 1 << 31;
    const QUALIFIED: u32 = 1 << 30;

    pub(crate) const fn new(
        caller_index: u32,
        callee: Str,
        receiver: Option<Str>,
        line: u32,
    ) -> Self {
        Self {
            caller: caller_index,
            callee,
            receiver,
            line,
        }
    }

    pub(crate) const fn written_from_root(self) -> Self {
        Self {
            caller: self.caller | Self::FROM_ROOT,
            ..self
        }
    }

    pub(crate) const fn qualified(self) -> Self {
        Self {
            caller: self.caller | Self::QUALIFIED,
            ..self
        }
    }

    pub(crate) const fn caller(self) -> u32 {
        self.caller & !(Self::FROM_ROOT | Self::QUALIFIED)
    }

    pub(crate) const fn is_written_from_root(self) -> bool {
        self.caller & Self::FROM_ROOT != 0
    }

    pub(crate) const fn is_qualified(self) -> bool {
        self.caller & Self::QUALIFIED != 0
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredLocalCall {
    pub(crate) caller: u32,
    pub(crate) local: u32,
    pub(crate) line: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredReExport {
    pub(crate) local_name: Str,
    pub(crate) source_name: Str,
    pub(crate) source_file: Option<Str>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredBinding {
    pub(crate) alias: Option<Str>,
    pub(crate) target_dir: Option<Str>,
    pub(crate) last_segment: Str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Occurrence {
    pub(crate) at: SymbolAt,
    pub(crate) role: CallableRole,
}

fn caller_range<T>(entries: &[T], caller: u32, caller_of: impl Fn(&T) -> u32) -> Range<usize> {
    let start = entries.partition_point(|entry| caller_of(entry) < caller);
    start..start + entries[start..].partition_point(|entry| caller_of(entry) == caller)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::Strings;

    #[test]
    fn a_site_names_a_call_or_a_local_call_by_its_position() {
        let call = Site::new(3, Entry::Call(7));
        let local = Site::new(3, Entry::LocalCall(7));

        assert_eq!(call.entry(), Entry::Call(7));
        assert_eq!(local.entry(), Entry::LocalCall(7));
        assert!(call < local);
    }

    #[test]
    fn a_call_keeps_its_caller_apart_from_how_its_callee_is_written() {
        let mut strings = Strings::default();
        let call = StoredCall::new(5, strings.intern("ns::f"), None, 9);

        let marked = call.written_from_root().qualified();

        assert_eq!(marked.caller(), 5);
        assert!(marked.is_written_from_root() && marked.is_qualified());
        assert!(!call.is_written_from_root() && !call.is_qualified());
    }
}
