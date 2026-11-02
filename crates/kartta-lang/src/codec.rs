use std::collections::HashMap;

use crate::extraction::{
    Access, CallRef, Callable, CallableRole, FoldedDeclaration, ImplementsAssertion, LocalCall,
    RawDirective, RawExtraction, RawReExport, Receiver, UseRef,
};
use crate::symbol::{Symbol, SymbolFlags, SymbolId, SymbolKind};

// constraint: a symbol's kind byte holds its kind in its low bits and these marks in its high ones, so the byte after
// it holds every `SymbolFlags` bit; a symbol that takes any number of arguments spends no byte on its count
const COUNTS_ARGUMENTS: u8 = 1 << 5;
const QUALIFIED_AS_NAME: u8 = 1 << 6;
const EXPORTED: u8 = 1 << 7;

// constraint: a call's marks byte is the one an optional text writes for its receiver, with the apart and method marks
// beside it, so a call that carries neither is written as before the marks; a call with no count spends no byte on it
const CALL_RECEIVER: u8 = 1;
const BINDS_APART: u8 = 1 << 1;
const CALLS_A_METHOD: u8 = 1 << 2;
const CALL_ARGUMENTS: u8 = 1 << 3;

const FORM: u8 = 0b11;
const BARE: u8 = 0;
const QUALIFIER: u8 = 1;
const OWN: u8 = 2;
const PATH: u8 = 3;
const READ: u8 = 0;
const WRITE: u8 = 1 << 2;
const UPDATE: u8 = 2 << 2;
const TYPE: u8 = 3 << 2;

impl RawExtraction {
    /// Appends this extraction to `out` in a compact binary form that [`RawExtraction::decode`] reads back.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut writer = Writer(out);
        writer.seq(&self.symbols, Writer::symbol);
        writer.seq(&self.callables, Writer::callable);
        writer.seq(&self.folded_declarations, |writer, folded| {
            writer.id(folded.definition);
            writer.uint(folded.line);
        });
        writer.seq(&self.calls, Writer::call);
        writer.seq(&self.local_calls, |writer, call| {
            writer.id(call.site_of);
            writer.id(call.local);
            writer.uint(call.line);
        });
        let mut spelled = HashMap::new();
        writer.seq(&self.uses, |writer, site| {
            writer.use_ref(site, &mut spelled);
        });
        writer.seq(&self.local_symbols, |writer, &id| writer.id(id));
        writer.seq(&self.re_exports, Writer::re_export);
        writer.optional(self.package.as_deref());
        writer.seq(&self.assertions, |writer, assertion| {
            writer.text(&assertion.type_name);
            writer.text(&assertion.interface);
        });
        writer.seq(&self.directives, Writer::directive);
        writer.seq(&self.entry_candidates, |writer, &id| writer.id(id));
    }

    /// The extraction [`RawExtraction::encode`] wrote into `bytes`, or `None` when `bytes` is not exactly one such
    /// encoding.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut reader = Reader { bytes, at: 0 };
        let raw = Self::read(&mut reader).ok()?;

        (reader.at == bytes.len()).then_some(raw)
    }

    fn read(reader: &mut Reader<'_>) -> Read<Self> {
        Ok(Self {
            symbols: reader.seq(Reader::symbol)?,
            callables: reader.seq(Reader::callable)?,
            folded_declarations: reader.seq(|reader| {
                Ok(FoldedDeclaration {
                    definition: reader.id()?,
                    line: reader.uint()?,
                })
            })?,
            calls: reader.seq(Reader::call)?,
            local_calls: reader.seq(|reader| {
                Ok(LocalCall {
                    site_of: reader.id()?,
                    local: reader.id()?,
                    line: reader.uint()?,
                })
            })?,
            uses: {
                let mut spelled = Vec::new();
                reader.seq(|reader| reader.use_ref(&mut spelled))?
            },
            local_symbols: reader.seq(Reader::id)?,
            re_exports: reader.seq(Reader::re_export)?,
            package: reader.optional()?,
            assertions: reader.seq(|reader| {
                Ok(ImplementsAssertion {
                    type_name: reader.text()?,
                    interface: reader.text()?,
                })
            })?,
            directives: reader.seq(Reader::directive)?,
            entry_candidates: reader.seq(Reader::id)?,
        })
    }
}

struct Writer<'a>(&'a mut Vec<u8>);

impl Writer<'_> {
    fn uint(&mut self, mut value: u32) {
        while value >= 0x80 {
            self.0.push(value.to_le_bytes()[0] | 0x80);
            value >>= 7;
        }
        self.0.push(value.to_le_bytes()[0]);
    }

    // constraint: a length is written in 32 bits, so a text or a sequence past `u32::MAX` is cut there — a text at a
    // char boundary — and what is cut is not written, so the bytes still decode
    fn len(&mut self, len: usize) -> usize {
        let written = u32::try_from(len).unwrap_or(u32::MAX);
        self.uint(written);
        usize::try_from(written).unwrap_or(len)
    }

    fn id(&mut self, id: SymbolId) {
        self.uint(id.0.wrapping_add(1));
    }

    fn text(&mut self, text: &str) {
        let kept = u32::try_from(text.len()).map_or_else(
            |_| &text[..text.floor_char_boundary(usize::try_from(u32::MAX).unwrap_or(usize::MAX))],
            |_| text,
        );
        self.len(kept.len());
        self.0.extend_from_slice(kept.as_bytes());
    }

    // constraint: the low bit of the first number tells a new text, its length above it, from the index of a text
    // spelled before, so a new text past `u32::MAX >> 1` bytes is cut there, at a char boundary
    fn spelled<'t>(&mut self, text: &'t str, spelled: &mut HashMap<&'t str, u32>) {
        if let Some(&index) = spelled.get(text) {
            self.uint(index << 1 | 1);
            return;
        }
        let limit = usize::try_from(u32::MAX >> 1).unwrap_or(usize::MAX);
        let kept = &text[..text.floor_char_boundary(limit)];
        self.uint(u32::try_from(kept.len()).unwrap_or(u32::MAX >> 1) << 1);
        self.0.extend_from_slice(kept.as_bytes());
        if let Ok(index) = u32::try_from(spelled.len())
            && index <= u32::MAX >> 1
        {
            spelled.insert(text, index);
        }
    }

    fn optional(&mut self, text: Option<&str>) {
        match text {
            Some(text) => {
                self.0.push(1);
                self.text(text);
            }
            None => self.0.push(0),
        }
    }

    fn seq<'i, T>(&mut self, items: &'i [T], mut each: impl FnMut(&mut Self, &'i T)) {
        let written = self.len(items.len());
        for item in &items[..written] {
            each(self, item);
        }
    }

    fn symbol(&mut self, symbol: &Symbol) {
        let qualified_as_name = symbol.qualified_name == symbol.name;
        let mut kind = symbol.kind as u8;
        if qualified_as_name {
            kind |= QUALIFIED_AS_NAME;
        }
        if symbol.exported {
            kind |= EXPORTED;
        }
        let counts_arguments = symbol.most_arguments != u8::MAX || symbol.fewest_arguments != 0;
        if counts_arguments {
            kind |= COUNTS_ARGUMENTS;
        }
        self.0.push(kind);
        self.0.push(symbol.flags.bits());
        if counts_arguments {
            self.0.push(symbol.most_arguments);
            self.0.push(symbol.fewest_arguments);
        }
        self.text(&symbol.name);
        if !qualified_as_name {
            self.text(&symbol.qualified_name);
        }
        self.optional(symbol.owner.as_deref());
        self.seq(&symbol.parents, |writer, parent| writer.text(parent));
        self.uint(symbol.line);
        self.uint(symbol.end_line);
        self.optional(symbol.signature.as_deref());
        self.seq(&symbol.value_types, |writer, path| writer.text(path));
    }

    fn callable(&mut self, callable: &Callable) {
        self.id(callable.symbol);
        self.text(&callable.key);
        self.0.push(match callable.role {
            CallableRole::Declaration => 0,
            CallableRole::Definition => 1,
        });
    }

    fn call(&mut self, call: &CallRef) {
        self.id(call.site_of);
        self.text(&call.callee);
        let mark = |set, mark| if set { mark } else { 0 };
        let marks = mark(call.binds_apart, BINDS_APART)
            | mark(call.calls_a_method, CALLS_A_METHOD)
            | mark(call.arguments != 0, CALL_ARGUMENTS);
        match call.receiver.as_deref() {
            Some(receiver) => {
                self.0.push(marks | CALL_RECEIVER);
                self.text(receiver);
            }
            None => self.0.push(marks),
        }
        self.uint(call.line);
        if call.arguments != 0 {
            self.0.push(call.arguments);
        }
    }

    fn use_ref<'t>(&mut self, site: &'t UseRef, spelled: &mut HashMap<&'t str, u32>) {
        self.id(site.site_of);
        self.spelled(&site.name, spelled);
        let (form, written) = match &site.receiver {
            Receiver::Bare => (BARE, None),
            Receiver::Qualifier(qualifier) => (QUALIFIER, Some(qualifier)),
            Receiver::Own => (OWN, None),
            Receiver::Path(path) => (PATH, Some(path)),
        };
        let access = match site.access {
            Access::Read => READ,
            Access::Write => WRITE,
            Access::Update => UPDATE,
            Access::Type => TYPE,
        };
        self.0.push(form | access);
        if let Some(written) = written {
            self.spelled(written, spelled);
        }
        self.uint(site.line);
    }

    fn re_export(&mut self, re_export: &RawReExport) {
        self.text(&re_export.local_name);
        self.text(&re_export.source_name);
        self.text(&re_export.source_path);
    }

    fn directive(&mut self, directive: &RawDirective) {
        self.uint(directive.form);
        self.text(&directive.spec);
        self.seq(&directive.names, |writer, name| writer.text(name));
        self.uint(directive.line);
    }
}

struct Malformed;

type Read<T> = Result<T, Malformed>;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Read<u8> {
        let byte = *self.bytes.get(self.at).ok_or(Malformed)?;
        self.at += 1;
        Ok(byte)
    }

    fn uint(&mut self) -> Read<u32> {
        let mut value = 0_u32;
        for shift in (0..u32::BITS).step_by(7) {
            let byte = self.byte()?;
            let payload = u32::from(byte & 0x7F);
            let shifted = payload << shift;
            let loses_bits = shifted >> shift != payload;
            let redundant_last_group = shift > 0 && byte == 0;
            if loses_bits || redundant_last_group {
                return Err(Malformed);
            }
            value |= shifted;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Malformed)
    }

    fn len(&mut self) -> Read<usize> {
        usize::try_from(self.uint()?).map_err(|_| Malformed)
    }

    fn id(&mut self) -> Read<SymbolId> {
        Ok(SymbolId(self.uint()?.wrapping_sub(1)))
    }

    fn str(&mut self, len: usize) -> Read<&'a str> {
        let end = self.at.checked_add(len).ok_or(Malformed)?;
        let bytes = self.bytes.get(self.at..end).ok_or(Malformed)?;
        let text = std::str::from_utf8(bytes).map_err(|_| Malformed)?;
        self.at = end;
        Ok(text)
    }

    fn text(&mut self) -> Read<String> {
        let len = self.len()?;
        Ok(self.str(len)?.to_owned())
    }

    fn optional(&mut self) -> Read<Option<String>> {
        match self.byte()? {
            0 => Ok(None),
            1 => Ok(Some(self.text()?)),
            _ => Err(Malformed),
        }
    }

    fn seq<T>(&mut self, mut each: impl FnMut(&mut Self) -> Read<T>) -> Read<Vec<T>> {
        let len = self.len()?;
        let remaining = self.bytes.len() - self.at;
        let mut items = Vec::with_capacity(len.min(remaining));
        for _ in 0..len {
            items.push(each(self)?);
        }
        Ok(items)
    }

    fn symbol(&mut self) -> Read<Symbol> {
        let marks = self.byte()?;
        let kind = *SymbolKind::ALL
            .get(usize::from(
                marks & !(COUNTS_ARGUMENTS | QUALIFIED_AS_NAME | EXPORTED),
            ))
            .ok_or(Malformed)?;
        let flags = self.byte()?;
        let (most_arguments, fewest_arguments) = if marks & COUNTS_ARGUMENTS == 0 {
            (u8::MAX, 0)
        } else {
            (self.byte()?, self.byte()?)
        };
        let name = self.text()?;
        let qualified_name = if marks & QUALIFIED_AS_NAME == 0 {
            self.text()?
        } else {
            name.clone()
        };
        let owner = self.optional()?;
        let parents = self.seq(Reader::text)?;
        let line = self.uint()?;
        let end_line = self.uint()?;
        let signature = self.optional()?;
        let value_types = self.seq(Reader::text)?;
        let mut symbol = Symbol::new(name, kind, line, end_line);
        symbol.qualified_name = qualified_name;
        symbol.owner = owner;
        symbol.parents = parents;
        symbol.exported = marks & EXPORTED != 0;
        symbol.signature = signature;
        symbol.value_types = value_types;
        symbol.flags = SymbolFlags::from_bits(flags);
        symbol.most_arguments = most_arguments;
        symbol.fewest_arguments = fewest_arguments;
        Ok(symbol)
    }

    fn callable(&mut self) -> Read<Callable> {
        Ok(Callable {
            symbol: self.id()?,
            key: self.text()?,
            role: match self.byte()? {
                0 => CallableRole::Declaration,
                1 => CallableRole::Definition,
                _ => return Err(Malformed),
            },
        })
    }

    fn call(&mut self) -> Read<CallRef> {
        let site_of = self.id()?;
        let callee = self.text()?;
        let marks = self.byte()?;
        if marks & !(BINDS_APART | CALLS_A_METHOD | CALL_ARGUMENTS | CALL_RECEIVER) != 0 {
            return Err(Malformed);
        }
        let receiver = if marks & CALL_RECEIVER == 0 {
            None
        } else {
            Some(self.text()?)
        };
        let line = self.uint()?;
        let arguments = if marks & CALL_ARGUMENTS == 0 {
            0
        } else {
            self.byte()?
        };
        Ok(CallRef {
            site_of,
            callee,
            receiver,
            line,
            binds_apart: marks & BINDS_APART != 0,
            calls_a_method: marks & CALLS_A_METHOD != 0,
            arguments,
        })
    }

    fn spelled(&mut self, spelled: &mut Vec<&'a str>) -> Read<String> {
        let first = self.uint()?;
        let index_or_len = usize::try_from(first >> 1).map_err(|_| Malformed)?;
        if first & 1 == 1 {
            let text = spelled.get(index_or_len).ok_or(Malformed)?;
            return Ok((*text).to_owned());
        }
        let text = self.str(index_or_len)?;
        spelled.push(text);
        Ok(text.to_owned())
    }

    fn use_ref(&mut self, spelled: &mut Vec<&'a str>) -> Read<UseRef> {
        let site_of = self.id()?;
        let name = self.spelled(spelled)?;
        let marks = self.byte()?;
        let receiver = match marks & FORM {
            BARE => Receiver::Bare,
            QUALIFIER => Receiver::Qualifier(self.spelled(spelled)?),
            OWN => Receiver::Own,
            _ => Receiver::Path(self.spelled(spelled)?),
        };
        let access = match marks & !FORM {
            READ => Access::Read,
            WRITE => Access::Write,
            UPDATE => Access::Update,
            TYPE => Access::Type,
            _ => return Err(Malformed),
        };
        Ok(UseRef {
            site_of,
            name,
            receiver,
            access,
            line: self.uint()?,
        })
    }

    fn re_export(&mut self) -> Read<RawReExport> {
        Ok(RawReExport {
            local_name: self.text()?,
            source_name: self.text()?,
            source_path: self.text()?,
        })
    }

    fn directive(&mut self) -> Read<RawDirective> {
        Ok(RawDirective {
            form: self.uint()?,
            spec: self.text()?,
            names: self.seq(Reader::text)?,
            line: self.uint()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FILE_SCOPE;

    fn every_channel() -> RawExtraction {
        let mut method = Symbol::new("parse", SymbolKind::Method, 3, 9);
        method.most_arguments = 2;
        method.fewest_arguments = 1;
        method.qualified_name = "Parser.parse".to_owned();
        method.owner = Some("Parser".to_owned());
        method.parents = vec!["Reader".to_owned(), "io.Closer".to_owned()];
        method.exported = true;
        method.signature = Some("func (p *Parser) parse() error".to_owned());
        method.value_types = vec!["[]Token".to_owned(), String::new(), "io.Reader".to_owned()];
        method.flags = SymbolFlags::empty()
            .with_test(true)
            .with_generated(true)
            .with_vendored(true)
            .with_fallback(true)
            .with_reflected(true)
            .with_made(true)
            .with_label(true);
        let free = Symbol::new("ünïcode", SymbolKind::EnumVariant, 1, 1);

        RawExtraction {
            symbols: vec![method, free],
            callables: vec![Callable {
                symbol: SymbolId(0),
                key: "Parser::parse()".to_owned(),
                role: CallableRole::Definition,
            }],
            folded_declarations: vec![FoldedDeclaration {
                definition: SymbolId(0),
                line: 2,
            }],
            calls: every_call_mark(),
            local_calls: vec![LocalCall {
                site_of: SymbolId(0),
                local: SymbolId(1),
                line: 5,
            }],
            uses: every_receiver_and_access(),
            local_symbols: vec![SymbolId(1)],
            re_exports: vec![RawReExport {
                local_name: "*".to_owned(),
                source_name: "Foo".to_owned(),
                source_path: "./foo".to_owned(),
            }],
            package: Some("parser".to_owned()),
            assertions: vec![ImplementsAssertion {
                type_name: "pkg.T".to_owned(),
                interface: "I".to_owned(),
            }],
            directives: vec![RawDirective {
                form: 2,
                spec: "example.com/app/lib".to_owned(),
                names: vec!["lib".to_owned()],
                line: 1,
            }],
            entry_candidates: vec![SymbolId(0)],
        }
    }

    fn every_call_mark() -> Vec<CallRef> {
        let call = |(site_of, callee, receiver): (SymbolId, &str, Option<&str>),
                    line,
                    (apart, method, arguments)| CallRef {
            site_of,
            callee: callee.to_owned(),
            receiver: receiver.map(str::to_owned),
            line,
            binds_apart: apart,
            calls_a_method: method,
            arguments,
        };
        vec![
            call((SymbolId(0), "read", Some("")), 4, (false, true, 0)),
            call((FILE_SCOPE, "init", None), 300_000, (false, false, 3)),
            call((SymbolId(0), "ready", None), 6, (true, false, 0)),
            call(
                (SymbolId(0), "expand", Some("Parser")),
                6,
                (true, false, u8::MAX),
            ),
        ]
    }

    fn every_receiver_and_access() -> Vec<UseRef> {
        let site = |name: &str, receiver, access| UseRef {
            site_of: SymbolId(0),
            name: name.to_owned(),
            receiver,
            access,
            line: 7,
        };
        vec![
            site(
                "Config",
                Receiver::Qualifier("cfg".to_owned()),
                Access::Read,
            ),
            site("count", Receiver::Bare, Access::Update),
            site("size", Receiver::Own, Access::Write),
            site(
                "timeout",
                Receiver::Path("Parser.options".to_owned()),
                Access::Read,
            ),
            site("timeout", Receiver::Path(String::new()), Access::Write),
            site("Options", Receiver::Bare, Access::Type),
        ]
    }

    fn encoded(raw: &RawExtraction) -> Vec<u8> {
        let mut bytes = Vec::new();
        raw.encode(&mut bytes);
        bytes
    }

    #[test]
    fn an_encoded_extraction_decodes_to_itself() {
        let raw = every_channel();

        let decoded = RawExtraction::decode(&encoded(&raw));

        assert_eq!(decoded, Some(raw));
        assert_eq!(
            RawExtraction::decode(&encoded(&RawExtraction::default())),
            Some(RawExtraction::default())
        );
    }

    #[test]
    fn a_call_that_carries_no_mark_is_written_as_its_receiver_s_optional_text() {
        for receiver in [None, Some("Parser")] {
            let call = CallRef {
                site_of: SymbolId(3),
                callee: "read".to_owned(),
                receiver: receiver.map(str::to_owned),
                line: 200,
                binds_apart: false,
                calls_a_method: false,
                arguments: 0,
            };
            let (mut written, mut expected) = (Vec::new(), Vec::new());
            Writer(&mut written).call(&call);
            let mut writer = Writer(&mut expected);
            writer.id(call.site_of);
            writer.text(&call.callee);
            writer.optional(receiver);
            writer.uint(call.line);

            assert_eq!(written, expected, "receiver {receiver:?}");
        }
    }

    #[test]
    fn bytes_that_are_not_exactly_one_encoding_decode_to_nothing() {
        let bytes = encoded(&every_channel());
        let truncated = &bytes[..bytes.len() - 1];
        let mut trailing = bytes.clone();
        trailing.push(0);
        let mut unknown_kind = encoded(&RawExtraction {
            symbols: vec![Symbol::new("f", SymbolKind::Function, 1, 1)],
            ..RawExtraction::default()
        });
        unknown_kind[1] = !(QUALIFIED_AS_NAME | EXPORTED);
        let one_use = encoded(&RawExtraction {
            uses: vec![UseRef {
                site_of: SymbolId(0),
                name: "f".to_owned(),
                receiver: Receiver::Bare,
                access: Access::Read,
                line: 1,
            }],
            ..RawExtraction::default()
        });
        let name_then_marks = [1 << 1, b'f', BARE | READ];
        let name = one_use
            .windows(name_then_marks.len())
            .position(|window| window == name_then_marks)
            .expect("the use's marks follow its name");
        let mut unknown_access = one_use.clone();
        unknown_access[name + 2] = TYPE + WRITE;
        let name_spelled_before_any = [&one_use[..name], &[1], &one_use[name + 2..]].concat();

        for bytes in [
            truncated,
            &trailing,
            &unknown_kind,
            &unknown_access,
            &name_spelled_before_any,
            &[0xFF; 9],
        ] {
            assert_eq!(RawExtraction::decode(bytes), None, "{bytes:?}");
        }
    }

    #[test]
    fn a_text_the_uses_spelled_before_is_written_once() {
        let raw = RawExtraction {
            uses: every_receiver_and_access()
                .into_iter()
                .chain(every_receiver_and_access())
                .collect(),
            ..RawExtraction::default()
        };
        let bytes = encoded(&raw);
        let spelled = |text: &[u8]| {
            bytes
                .windows(text.len())
                .filter(|window| *window == text)
                .count()
        };

        assert_eq!(
            (
                spelled(b"timeout"),
                spelled(b"Parser.options"),
                spelled(b"cfg")
            ),
            (1, 1, 1)
        );
        assert_eq!(RawExtraction::decode(&bytes), Some(raw));
    }

    #[test]
    fn a_count_spelled_otherwise_than_the_encoder_writes_it_decodes_to_nothing() {
        let empty = encoded(&RawExtraction::default());
        let respelled = |count: &[u8]| [count, &empty[1..]].concat();
        let zero_with_a_redundant_group = respelled(&[0x80, 0x00]);
        let zero_past_thirty_two_bits = respelled(&[0x80, 0x80, 0x80, 0x80, 0x10]);

        assert_eq!(
            RawExtraction::decode(&respelled(&[0x00])),
            Some(RawExtraction::default())
        );
        assert_eq!(RawExtraction::decode(&zero_with_a_redundant_group), None);
        assert_eq!(RawExtraction::decode(&zero_past_thirty_two_bits), None);
    }

    #[test]
    fn every_kind_decodes_to_itself() {
        let raw = RawExtraction {
            symbols: (SymbolKind::ALL.into_iter())
                .map(|kind| Symbol::new("f", kind, 1, 1))
                .collect(),
            ..RawExtraction::default()
        };

        assert_eq!(RawExtraction::decode(&encoded(&raw)), Some(raw));
    }
}
