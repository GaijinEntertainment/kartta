use kartta_lang::tree::{self, preorder};
use kartta_lang::{RawDirective, RawExtraction};
use tree_sitter::{Node, Tree};

use crate::text::find;

pub(crate) const INCLUDE_FORM: u32 = 0;
pub(crate) const MOUNT_BLOCK_FORM: u32 = 1;
pub(crate) const MOUNT_PARAMETER_FORM: u32 = 2;

const MOUNT_BLOCK_NAME: &[u8] = b"mountPoints";

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut raw = RawExtraction::default();
    let mut mounts_read = false;

    // constraint: an application registers the entries of the first `mountPoints` block at the root of the file and
    // reads no other; a statement nests only in a block, so no parameter holds one
    preorder(tree, |node| {
        if !node.is_named() {
            return false;
        }
        match node.kind() {
            "include" => {
                if let Some(file) = node.child_by_field_name("file") {
                    raw.directives
                        .extend(record_include(file, source, line(node)));
                }
                false
            }
            "block" => {
                if !mounts_read
                    && node
                        .parent()
                        .is_some_and(|parent| parent.kind() == "root_block")
                    && name_of(node, source).as_deref() == Some(MOUNT_BLOCK_NAME)
                {
                    mounts_read = true;
                    record_mounts(node, source, &mut raw.directives);
                }
                true
            }
            "parameter" | "comment" => false,
            _ => true,
        }
    });

    raw
}

fn record_mounts(block: Node<'_>, source: &[u8], directives: &mut Vec<RawDirective>) {
    let mut cursor = block.walk();
    for entry in block.named_children(&mut cursor) {
        let (form, mount_dir) = match entry.kind() {
            "block" => (MOUNT_BLOCK_FORM, source_dir(entry, source)),
            "parameter" => match string_value(entry, source) {
                Some(value) => (MOUNT_PARAMETER_FORM, Some(value)),
                None => continue,
            },
            _ => continue,
        };
        let Some(name) = name_of(entry, source) else {
            continue;
        };

        let mut directive = RawDirective::new(
            form,
            String::from_utf8_lossy(&name).into_owned(),
            line(entry),
        );
        directive.names.extend(mount_dir);
        directives.push(directive);
    }
}

fn name_of(node: Node<'_>, source: &[u8]) -> Option<Vec<u8>> {
    read_value(node.child_by_field_name("name")?, source)
}

// constraint: an application takes a mount block's directory from its first `forSource` parameter
fn source_dir(block: Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = block.walk();
    let parameter = block.named_children(&mut cursor).find(|node| {
        node.kind() == "parameter"
            && name_of(*node, source).as_deref() == Some(b"forSource".as_slice())
    })?;

    string_value(parameter, source)
}

fn string_value(parameter: Node<'_>, source: &[u8]) -> Option<String> {
    let type_node = parameter.child_by_field_name("type")?;
    let type_name = match type_node.named_child(0) {
        Some(quoted) => read_value(quoted, source)?,
        None => source[type_node.byte_range()].to_vec(),
    };
    let value = parameter.child_by_field_name("value")?;
    if type_name != b"t" || value.kind() == "array" {
        return None;
    }

    Some(String::from_utf8_lossy(&read_value(value, source)?).into_owned())
}

fn line(node: Node<'_>) -> u32 {
    tree::line(node.start_position().row)
}

fn record_include(file: Node<'_>, source: &[u8], line: u32) -> Option<RawDirective> {
    let read = read_value(file, source)?;
    if read.is_empty() {
        return None;
    }
    let token = &source[file.byte_range()];
    let written = if file.kind() == "string" {
        let (body, triple) = quoted_body(token)?;
        quoted_text(body, triple)
    } else {
        token
    };

    let mut directive = RawDirective::new(
        INCLUDE_FORM,
        String::from_utf8_lossy(written).into_owned(),
        line,
    );
    let read = String::from_utf8(read)
        .unwrap_or_else(|invalid| String::from_utf8_lossy(invalid.as_bytes()).into_owned());
    if read != directive.spec {
        directive.names.push(read);
    }

    Some(directive)
}

// constraint: the engine's `getValue` decodes the `~` escapes of a quoted value, and for a triple-quoted one drops its
// CR bytes and one final line end; it drops the block comments and the trailing blanks of an unquoted value
fn read_value(node: Node<'_>, source: &[u8]) -> Option<Vec<u8>> {
    let token = &source[node.byte_range()];
    if node.kind() == "string" {
        let (body, triple) = quoted_body(token)?;
        Some(quoted_value(body, triple))
    } else {
        Some(unquoted_value(token))
    }
}

// constraint: a triple-quoted value starts after the blank rest of the line that opens it
fn quoted_body(token: &[u8]) -> Option<(&[u8], bool)> {
    let triple = token.len() >= 6 && token[0] == token[1] && token[1] == token[2];
    let quotes = if triple { 3 } else { 1 };
    let mut body = token.get(quotes..token.len().saturating_sub(quotes))?;
    if triple
        && let Some(first) = body
            .iter()
            .position(|byte| !matches!(byte, b' ' | b'\t' | b'\r'))
        && body[first] == b'\n'
    {
        body = &body[first + 1..];
    }

    Some((body, triple))
}

fn quoted_text(body: &[u8], triple: bool) -> &[u8] {
    if !triple {
        return body;
    }
    match body.strip_suffix(b"\n") {
        Some(text) if !text.is_empty() => text.strip_suffix(b"\r").unwrap_or(text),
        _ => body,
    }
}

fn quoted_value(body: &[u8], triple: bool) -> Vec<u8> {
    let mut value = Vec::with_capacity(body.len());
    let mut bytes = body.iter();
    while let Some(&byte) = bytes.next() {
        match byte {
            b'~' => value.extend(bytes.next().map(|&escaped| unescape(escaped))),
            b'\r' if triple => {}
            _ => value.push(byte),
        }
    }
    if triple && value.len() > 1 && value.last() == Some(&b'\n') {
        value.pop();
    }

    value
}

const fn unescape(byte: u8) -> u8 {
    match byte {
        b'r' => b'\r',
        b'n' => b'\n',
        b't' => b'\t',
        other => other,
    }
}

fn unquoted_value(token: &[u8]) -> Vec<u8> {
    let mut value = Vec::with_capacity(token.len());
    let mut rest = token;
    while let Some(open) = find(rest, b"/*") {
        value.extend_from_slice(&rest[..open]);
        let comment = &rest[open + 2..];
        rest = find(comment, b"*/").map_or(&[], |close| &comment[close + 2..]);
    }
    value.extend_from_slice(rest);

    let end = value
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(0, |last| last + 1);
    value.truncate(end);

    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw_extraction;

    fn mounts(raw: &RawExtraction) -> Vec<(u32, &str, Vec<&str>, u32)> {
        raw.directives
            .iter()
            .filter(|directive| directive.form != INCLUDE_FORM)
            .map(|directive| {
                let names = directive.names.iter().map(String::as_str).collect();
                (
                    directive.form,
                    directive.spec.as_str(),
                    names,
                    directive.line,
                )
            })
            .collect()
    }

    #[test]
    fn the_walk_records_the_entries_of_the_first_root_mount_block() {
        let source = br#"debug{ mountPoints{ "%nested"{ forSource:t="../n"; } } }
mountPoints{
  "%gameBase"{ forVromfs:t=""; forSource:t="..\prog\gameBase"; forSource:t="../second"; }
  darg{ forVromfs:t="daRg"; }
  "%count"{ forSource:i=1; }
  "%dng~das":t="../../prog/das"
  "%flag":b=yes
  sub{ include "inner.blk" }
}
mountPoints{ "%second"{ forSource:t="../s"; } }
"#;
        let raw = raw_extraction(source);

        assert_eq!(
            mounts(&raw),
            [
                (MOUNT_BLOCK_FORM, "%gameBase", vec!["..\\prog\\gameBase"], 3),
                (MOUNT_BLOCK_FORM, "darg", vec![], 4),
                (MOUNT_BLOCK_FORM, "%count", vec![], 5),
                (MOUNT_PARAMETER_FORM, "%dngdas", vec!["../../prog/das"], 6),
                (MOUNT_BLOCK_FORM, "sub", vec![], 8),
            ]
        );
        let includes: Vec<(&str, u32)> = raw
            .directives
            .iter()
            .filter(|directive| directive.form == INCLUDE_FORM)
            .map(|directive| (directive.spec.as_str(), directive.line))
            .collect();
        assert_eq!(includes, [("inner.blk", 8)]);
    }

    #[test]
    fn a_quoted_mount_block_name_is_read_as_the_engine_reads_it() {
        let quoted = b"x:i=1\n\"mountPoints\" { a{ forSource:t=\"../a\"; } }\n";
        let unnamed = raw_extraction(b"mountPoints-x{}\n");

        assert!(unnamed.directives.is_empty(), "{:?}", mounts(&unnamed));
        assert_eq!(
            mounts(&raw_extraction(quoted)),
            [(MOUNT_BLOCK_FORM, "a", vec!["../a"], 2)]
        );
    }
}
