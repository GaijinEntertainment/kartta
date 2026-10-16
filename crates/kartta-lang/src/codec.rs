use crate::extraction::{
    CallRef, Callable, CallableRole, FoldedDeclaration, ImplementsAssertion, LocalCall,
    RawDirective, RawExtraction, RawReExport, RefSite,
};
use crate::symbol::{Symbol, SymbolFlags, SymbolId, SymbolKind};

const QUALIFIED_AS_NAME: u8 = 1 << 6;
const EXPORTED: u8 = 1 << 7;

impl RawExtraction {
    /// The identity of the bytes [`RawExtraction::encode`] writes: a cache keys an encoded extraction by it as well as
    /// by its language's fingerprint, so an entry in another layout is never decoded.
    // constraint: names the layout `encode` writes, so it changes with any change to it
    pub const CODEC: &'static [u8] = b"kartta-lang raw extraction 2";

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
            writer.id(call.caller_id);
            writer.id(call.local);
            writer.uint(call.line);
        });
        writer.seq(&self.references, Writer::reference);
        writer.seq(&self.local_symbols, |writer, &id| writer.id(id));
        writer.seq(&self.re_exports, Writer::re_export);
        writer.optional(self.package.as_deref());
        writer.seq(&self.assertions, |writer, assertion| {
            writer.text(&assertion.type_name);
            writer.text(&assertion.iface);
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
                    caller_id: reader.id()?,
                    local: reader.id()?,
                    line: reader.uint()?,
                })
            })?,
            references: reader.seq(Reader::reference)?,
            local_symbols: reader.seq(Reader::id)?,
            re_exports: reader.seq(Reader::re_export)?,
            package: reader.optional()?,
            assertions: reader.seq(|reader| {
                Ok(ImplementsAssertion {
                    type_name: reader.text()?,
                    iface: reader.text()?,
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

    fn optional(&mut self, text: Option<&str>) {
        match text {
            Some(text) => {
                self.0.push(1);
                self.text(text);
            }
            None => self.0.push(0),
        }
    }

    fn seq<T>(&mut self, items: &[T], mut each: impl FnMut(&mut Self, &T)) {
        let written = self.len(items.len());
        for item in &items[..written] {
            each(self, item);
        }
    }

    fn symbol(&mut self, symbol: &Symbol) {
        let qualified_as_name = symbol.qualified_name == symbol.name;
        let mut marks = symbol.flags.bits();
        if qualified_as_name {
            marks |= QUALIFIED_AS_NAME;
        }
        if symbol.exported {
            marks |= EXPORTED;
        }
        self.0.push(kind_code(symbol.kind));
        self.0.push(marks);
        self.text(&symbol.name);
        if !qualified_as_name {
            self.text(&symbol.qualified_name);
        }
        self.optional(symbol.owner.as_deref());
        self.seq(&symbol.parents, |writer, parent| writer.text(parent));
        self.uint(symbol.line);
        self.uint(symbol.end_line);
        self.optional(symbol.signature.as_deref());
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
        self.id(call.caller_id);
        self.text(&call.callee);
        self.optional(call.receiver.as_deref());
        self.uint(call.line);
    }

    fn reference(&mut self, reference: &RefSite) {
        self.id(reference.site_of);
        self.text(&reference.name);
        self.optional(reference.qualifier.as_deref());
        self.uint(reference.line);
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

impl Reader<'_> {
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

    fn text(&mut self) -> Read<String> {
        let len = self.len()?;
        let end = self.at.checked_add(len).ok_or(Malformed)?;
        let bytes = self.bytes.get(self.at..end).ok_or(Malformed)?;
        let text = std::str::from_utf8(bytes).map_err(|_| Malformed)?;
        self.at = end;
        Ok(text.to_owned())
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
        let kind = *KINDS.get(usize::from(self.byte()?)).ok_or(Malformed)?;
        let marks = self.byte()?;
        if marks & !(SymbolFlags::ALL | QUALIFIED_AS_NAME | EXPORTED) != 0 {
            return Err(Malformed);
        }
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
        let mut symbol = Symbol::new(name, kind, line, end_line);
        symbol.qualified_name = qualified_name;
        symbol.owner = owner;
        symbol.parents = parents;
        symbol.exported = marks & EXPORTED != 0;
        symbol.signature = signature;
        symbol.flags = SymbolFlags::from_bits(marks);
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
        Ok(CallRef {
            caller_id: self.id()?,
            callee: self.text()?,
            receiver: self.optional()?,
            line: self.uint()?,
        })
    }

    fn reference(&mut self) -> Read<RefSite> {
        Ok(RefSite {
            site_of: self.id()?,
            name: self.text()?,
            qualifier: self.optional()?,
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

fn kind_code(kind: SymbolKind) -> u8 {
    let code = KINDS.iter().position(|&known| known == kind).unwrap_or(0);
    u8::try_from(code).unwrap_or(0)
}

// constraint: a kind's position here is its byte in an encoding, so a new kind goes at the end and none moves
const KINDS: [SymbolKind; 14] = [
    SymbolKind::Function,
    SymbolKind::Method,
    SymbolKind::Class,
    SymbolKind::Struct,
    SymbolKind::Namespace,
    SymbolKind::Interface,
    SymbolKind::Variable,
    SymbolKind::Constant,
    SymbolKind::Enum,
    SymbolKind::TypeAlias,
    SymbolKind::Trait,
    SymbolKind::Macro,
    SymbolKind::Field,
    SymbolKind::EnumVariant,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NO_CALLER_ID;

    fn every_channel() -> RawExtraction {
        let mut method = Symbol::new("parse", SymbolKind::Method, 3, 9);
        method.qualified_name = "Parser.parse".to_owned();
        method.owner = Some("Parser".to_owned());
        method.parents = vec!["Reader".to_owned(), "io.Closer".to_owned()];
        method.exported = true;
        method.signature = Some("func (p *Parser) parse() error".to_owned());
        method.flags = SymbolFlags::empty()
            .with_test(true)
            .with_generated(true)
            .with_vendored(true);
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
            calls: vec![
                CallRef {
                    caller_id: SymbolId(0),
                    callee: "read".to_owned(),
                    receiver: Some(String::new()),
                    line: 4,
                },
                CallRef {
                    caller_id: NO_CALLER_ID,
                    callee: "init".to_owned(),
                    receiver: None,
                    line: 300_000,
                },
            ],
            local_calls: vec![LocalCall {
                caller_id: SymbolId(0),
                local: SymbolId(1),
                line: 5,
            }],
            references: vec![RefSite {
                site_of: SymbolId(1),
                name: "Config".to_owned(),
                qualifier: Some("cfg".to_owned()),
                line: 7,
            }],
            local_symbols: vec![SymbolId(1)],
            re_exports: vec![RawReExport {
                local_name: "*".to_owned(),
                source_name: "Foo".to_owned(),
                source_path: "./foo".to_owned(),
            }],
            package: Some("parser".to_owned()),
            assertions: vec![ImplementsAssertion {
                type_name: "pkg.T".to_owned(),
                iface: "I".to_owned(),
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
    fn bytes_that_are_not_exactly_one_encoding_decode_to_nothing() {
        let bytes = encoded(&every_channel());
        let truncated = &bytes[..bytes.len() - 1];
        let mut trailing = bytes.clone();
        trailing.push(0);
        let mut unknown_kind = encoded(&RawExtraction {
            symbols: vec![Symbol::new("f", SymbolKind::Function, 1, 1)],
            ..RawExtraction::default()
        });
        let mut unknown_mark = unknown_kind.clone();
        unknown_kind[1] = 200;
        unknown_mark[2] |= 1 << 5;

        for bytes in [
            truncated,
            &trailing,
            &unknown_kind,
            &unknown_mark,
            &[0xFF; 9],
        ] {
            assert_eq!(RawExtraction::decode(bytes), None, "{bytes:?}");
        }
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
    fn every_kind_has_its_own_byte() {
        for (at, &kind) in KINDS.iter().enumerate() {
            assert_eq!(usize::from(kind_code(kind)), at, "{kind:?}");
        }
    }
}
