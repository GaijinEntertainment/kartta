use std::ops::Range;
use std::sync::OnceLock;

use kartta_lang::{Access, CallableRole, SymbolFlags, SymbolKind};

use crate::search::WordSet;
use crate::strings::Str;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SymbolAt {
    pub(crate) file: u32,
    pub(crate) index: u32,
}

impl SymbolAt {
    pub(crate) const fn new(file: u32, index: u32) -> Self {
        Self { file, index }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Site {
    pub(crate) file: u32,
    entry: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct UseAt {
    pub(crate) file: u32,
    pub(crate) index: u32,
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
    pub(crate) value_types: Box<[ValueTypes]>,
    pub(crate) locals: Box<[Local]>,
    pub(crate) callables: Box<[StoredCallable]>,
    pub(crate) folded: Box<[Folded]>,
    // constraint: both sorted by caller, in source order within one, so `calls_by` and `local_calls_by` find a body's
    // calls without a scan
    pub(crate) calls: Box<[StoredCall]>,
    // constraint: the listing keys of the calls and the names of the uses made at file scope, sorted and once each;
    // they have no symbol to resolve from, so they are kept as names the file spells and not as sites
    pub(crate) file_scope_names: Box<[Str]>,
    pub(crate) local_calls: Box<[StoredLocalCall]>,
    pub(crate) uses: Box<[StoredUse]>,
    pub(crate) includes: Box<[Str]>,
    pub(crate) re_exports: Box<[StoredReExport]>,
    pub(crate) bindings: Box<[StoredBinding]>,
    pub(crate) assertions: Box<[StoredAssertion]>,
    pub(crate) words: OnceLock<WordSet>,
}

impl StoredFile {
    pub(crate) fn holder(&self, index: u32) -> Option<u32> {
        of_symbol(&self.locals, index, |local| local.symbol).map(|local| local.holder)
    }

    pub(crate) fn callable(&self, index: u32) -> Option<&StoredCallable> {
        of_symbol(&self.callables, index, |callable| callable.symbol)
    }

    pub(crate) fn folded_lines(&self, index: u32) -> impl Iterator<Item = u32> + Clone + '_ {
        self.folded[run(&self.folded, index, |folded| folded.definition)]
            .iter()
            .map(|folded| folded.line)
    }

    pub(crate) fn calls_by(&self, caller: u32) -> Range<usize> {
        run(&self.calls, caller, |call| call.caller())
    }

    pub(crate) fn local_calls_by(&self, caller: u32) -> Range<usize> {
        run(&self.local_calls, caller, |call| call.caller)
    }

    pub(crate) fn parents_of(&self, index: u32) -> &[Parent] {
        &self.parents[run(&self.parents, index, |parent| parent.symbol)]
    }

    // constraint: a symbol's value types are kept as one string joined at `VALUE_TYPE_SEPARATOR`, which no type path
    // holds
    pub(crate) fn value_types(&self, index: u32) -> Option<Str> {
        of_symbol(&self.value_types, index, |held| held.symbol).map(|held| held.types)
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
    pub(crate) exported_and_fewest_arguments: u8,
    pub(crate) most_arguments: u8,
}

impl StoredSymbol {
    // constraint: a symbol holds its fewest arguments beside its export mark, so it stays 28 bytes in every language,
    // and a count this cuts is still a lower bound
    const EXPORTED: u8 = 1 << 7;

    pub(crate) const fn with_export_mark(fewest: u8, exported: bool) -> u8 {
        let fewest = if fewest < Self::EXPORTED {
            fewest
        } else {
            Self::EXPORTED - 1
        };
        if exported {
            fewest | Self::EXPORTED
        } else {
            fewest
        }
    }

    pub(crate) const fn exported(&self) -> bool {
        self.exported_and_fewest_arguments & Self::EXPORTED != 0
    }

    pub(crate) const fn fewest_arguments(&self) -> u8 {
        self.exported_and_fewest_arguments & !Self::EXPORTED
    }
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

pub(crate) const VALUE_TYPE_SEPARATOR: &str = ",";

#[derive(Debug, Clone, Copy)]
pub(crate) struct ValueTypes {
    pub(crate) symbol: u32,
    pub(crate) types: Str,
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
    line_and_arguments: u32,
}

impl StoredCall {
    // constraint: a call holds its count of arguments above its line, so it stays 16 bytes in every language, and a call
    // past the last line this leaves reads as that line
    const LAST_LINE: u32 = (1 << 24) - 1;
    const ARGUMENTS_SHIFT: u32 = Self::LAST_LINE.count_ones();
    const FROM_ROOT: u32 = 1 << 31;
    const QUALIFIED: u32 = 1 << 30;
    const ON_A_CLASS: u32 = 1 << 29;
    const BINDS_APART: u32 = 1 << 28;
    const CALLS_A_METHOD: u32 = 1 << 27;
    const MARKS: u32 = Self::FROM_ROOT
        | Self::QUALIFIED
        | Self::ON_A_CLASS
        | Self::BINDS_APART
        | Self::CALLS_A_METHOD;
    pub(crate) const CALLERS: u32 = Self::CALLS_A_METHOD;

    pub(crate) const fn new(
        caller_index: u32,
        callee: Str,
        receiver: Option<Str>,
        (line, arguments): (u32, u8),
    ) -> Self {
        Self {
            caller: caller_index,
            callee,
            receiver,
            line_and_arguments: (arguments as u32) << Self::ARGUMENTS_SHIFT
                | if line < Self::LAST_LINE {
                    line
                } else {
                    Self::LAST_LINE
                },
        }
    }

    pub(crate) const fn line(self) -> u32 {
        self.line_and_arguments & Self::LAST_LINE
    }

    pub(crate) const fn arguments(self) -> u8 {
        (self.line_and_arguments >> Self::ARGUMENTS_SHIFT) as u8
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

    pub(crate) const fn on_a_class(self) -> Self {
        Self {
            caller: self.caller | Self::ON_A_CLASS,
            ..self
        }
    }

    pub(crate) const fn binding_apart(self) -> Self {
        Self {
            caller: self.caller | Self::BINDS_APART,
            ..self
        }
    }

    pub(crate) const fn calling_a_method(self) -> Self {
        Self {
            caller: self.caller | Self::CALLS_A_METHOD,
            ..self
        }
    }

    pub(crate) const fn caller(self) -> u32 {
        self.caller & !Self::MARKS
    }

    pub(crate) const fn binds_apart(self) -> bool {
        self.caller & Self::BINDS_APART != 0
    }

    pub(crate) const fn calls_a_method(self) -> bool {
        self.caller & Self::CALLS_A_METHOD != 0
    }

    pub(crate) const fn is_on_a_class(self) -> bool {
        self.caller & Self::ON_A_CLASS != 0
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

// constraint: 16 bytes a site, as a call: the receiver's form and the access ride in the top bits of `site_of`, and the
// qualifier or the path in `written`
#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredUse {
    site_of: u32,
    pub(crate) name: Str,
    written: Option<Str>,
    pub(crate) line: u32,
}

/// What a stored use's name is written on, as [`kartta_lang::Receiver`] says, its text interned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UseReceiver {
    Bare,
    Qualifier(Str),
    Own,
    Path(Option<Str>),
}

impl StoredUse {
    pub(crate) const SITES: u32 = 1 << 28;
    const FORM_SHIFT: u32 = 30;
    const ACCESS_SHIFT: u32 = 28;

    pub(crate) const fn new(
        site_of: u32,
        (name, receiver): (Str, UseReceiver),
        access: Access,
        line: u32,
    ) -> Self {
        let (form, written) = match receiver {
            UseReceiver::Bare => (0, None),
            UseReceiver::Qualifier(qualifier) => (1, Some(qualifier)),
            UseReceiver::Own => (2, None),
            UseReceiver::Path(path) => (3, path),
        };
        let access = match access {
            Access::Read => 0,
            Access::Write => 1,
            Access::Update => 2,
            Access::Type => 3,
        };
        Self {
            site_of: site_of | form << Self::FORM_SHIFT | access << Self::ACCESS_SHIFT,
            name,
            written,
            line,
        }
    }

    pub(crate) const fn site_of(self) -> u32 {
        self.site_of & (Self::SITES - 1)
    }

    pub(crate) const fn receiver(self) -> UseReceiver {
        match self.site_of >> Self::FORM_SHIFT {
            0 => UseReceiver::Bare,
            1 => match self.written {
                Some(qualifier) => UseReceiver::Qualifier(qualifier),
                None => UseReceiver::Bare,
            },
            2 => UseReceiver::Own,
            _ => UseReceiver::Path(self.written),
        }
    }

    pub(crate) const fn access(self) -> Access {
        match (self.site_of >> Self::ACCESS_SHIFT) & 0b11 {
            0 => Access::Read,
            1 => Access::Write,
            2 => Access::Update,
            _ => Access::Type,
        }
    }
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredAssertion {
    pub(crate) type_name: Str,
    pub(crate) interface: Str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Occurrence {
    pub(crate) at: SymbolAt,
    pub(crate) role: CallableRole,
}

fn run<T>(entries: &[T], key: u32, key_of: impl Fn(&T) -> u32) -> Range<usize> {
    let start = entries.partition_point(|entry| key_of(entry) < key);
    start..start + entries[start..].partition_point(|entry| key_of(entry) == key)
}

fn of_symbol<T>(entries: &[T], index: u32, symbol_of: impl Fn(&T) -> u32) -> Option<&T> {
    let at = entries.binary_search_by_key(&index, symbol_of).ok()?;
    entries.get(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::Strings;

    #[test]
    fn a_symbol_holds_its_export_mark_beside_a_lower_bound_of_its_fewest_arguments() {
        let stored = |fewest, exported| {
            let mut strings = Strings::default();
            let name = strings.intern("f");
            StoredSymbol {
                name,
                qualified_name: name,
                owner: None,
                signature: None,
                line: 1,
                end_line: 1,
                kind: SymbolKind::Function,
                flags: SymbolFlags::empty(),
                exported_and_fewest_arguments: StoredSymbol::with_export_mark(fewest, exported),
                most_arguments: u8::MAX,
            }
        };

        assert_eq!(size_of::<StoredSymbol>(), 28);
        assert!(stored(3, true).exported());
        assert_eq!(stored(3, true).fewest_arguments(), 3);
        assert!(!stored(3, false).exported());
        assert_eq!(stored(3, false).fewest_arguments(), 3);
        assert_eq!(stored(u8::MAX, true).fewest_arguments(), 127);
        assert!(stored(u8::MAX, false).fewest_arguments() < u8::MAX);
        assert!(!stored(u8::MAX, false).exported());
    }

    #[test]
    fn a_site_names_a_call_or_a_local_call_by_its_position() {
        let call = Site::new(3, Entry::Call(7));
        let local = Site::new(3, Entry::LocalCall(7));

        assert_eq!(call.entry(), Entry::Call(7));
        assert_eq!(local.entry(), Entry::LocalCall(7));
        assert!(call < local);
    }

    #[test]
    fn a_call_keeps_its_caller_apart_from_how_its_callee_and_its_receiver_are_written() {
        let mut strings = Strings::default();
        let last = StoredCall::CALLERS - 1;
        let call = StoredCall::new(last, strings.intern("ns::f"), None, (9, u8::MAX));

        let marked = call
            .written_from_root()
            .qualified()
            .on_a_class()
            .binding_apart()
            .calling_a_method();

        assert_eq!((call.caller(), marked.caller()), (last, last));
        assert_eq!((marked.line(), marked.arguments()), (9, u8::MAX));
        assert_eq!(size_of::<StoredCall>(), 16);
        assert!(marked.is_written_from_root() && marked.is_qualified() && marked.is_on_a_class());
        assert!(marked.binds_apart() && marked.calls_a_method());
        assert!(!call.is_written_from_root() && !call.is_qualified() && !call.is_on_a_class());
        assert!(!call.binds_apart() && !call.calls_a_method());
    }

    #[test]
    fn a_call_past_the_last_line_reads_as_that_line_and_keeps_its_arguments() {
        let mut strings = Strings::default();
        let call = StoredCall::new(0, strings.intern("f"), None, (u32::MAX, 1));

        assert_eq!((call.line(), call.arguments()), (StoredCall::LAST_LINE, 1));
    }

    #[test]
    fn a_use_keeps_its_site_apart_from_its_receiver_and_its_access() {
        let mut strings = Strings::default();
        let (name, path) = (strings.intern("timeout"), strings.intern("Server.options"));
        let last = StoredUse::SITES - 1;
        let receivers = [
            UseReceiver::Bare,
            UseReceiver::Qualifier(path),
            UseReceiver::Own,
            UseReceiver::Path(Some(path)),
            UseReceiver::Path(None),
        ];

        for receiver in receivers {
            for access in [Access::Read, Access::Write, Access::Update, Access::Type] {
                let stored = StoredUse::new(last, (name, receiver), access, 9);

                assert_eq!(
                    (stored.site_of(), stored.receiver(), stored.access()),
                    (last, receiver, access)
                );
            }
        }
        assert_eq!(size_of::<StoredUse>(), 16);
    }
}
