//! YAML reading on top of libyaml, with the reference implementation's
//! parse-error line.
//!
//! # Why libyaml
//!
//! The kernel has to answer one question about a profile file: *does the
//! loader's YAML parser accept this document, and if not, which line does it
//! blame?* That answer is part of the golden contract (F16 carries
//! `params.line`), so the Rust side cannot answer it with a different parser
//! and hope the line matches.
//!
//! `gopkg.in/yaml.v3` is a Go port of libyaml, and `unsafe-libyaml` is libyaml
//! itself, translated to Rust. Using it gives byte-identical acceptance *and*
//! the same error marks — including the marks that look wrong but are part of
//! the contract, such as reporting line 1 for an unclosed flow sequence on
//! line 2. `yaml-rust2` is also a libyaml descendant but is not faithful here:
//! it accepted a tab-indented document that yaml.v3 rejects, and its error mark
//! pointed at the end of the file.
//!
//! # Tags
//!
//! libyaml's document API does not resolve implicit tags: a plain `true` comes
//! back tagged `!!str`. This module applies the same implicit resolution the
//! reference implementation does, because F16 distinguishes `id: 123` (not a
//! string) from `id: "123"` and F17 needs `false` to be a boolean.
//!
//! One limitation is documented rather than hidden: libyaml's document API
//! discards the "was it explicitly tagged" flag, so an explicit `!!str 123` is
//! indistinguishable from plain `123` here and resolves to an integer. That
//! shape does not occur in a cordis patch file; if it ever does, this module has
//! to move to the event API.

// The whole point of this crate is the wrapper around a C transliteration; the
// safe surface above it is what the rest of the workspace uses, and every
// `unsafe` block below carries its own justification.
#![allow(unsafe_code)]
#![deny(clippy::undocumented_unsafe_blocks)]

use std::mem::MaybeUninit;

use unsafe_libyaml::{
    yaml_document_delete, yaml_document_t, yaml_error_type_t, yaml_node_t, yaml_node_type_t,
    yaml_parser_delete, yaml_parser_initialize, yaml_parser_load, yaml_parser_set_input_string,
    yaml_parser_t, yaml_scalar_style_t,
};

/// The tag of a scalar, resolved the way the reference implementation resolves
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tag {
    /// A string.
    Str,
    /// A null (`~`, `null`, or an empty plain scalar).
    Null,
    /// A boolean.
    Bool,
    /// An integer.
    Int,
    /// A float.
    Float,
    /// A timestamp.
    Timestamp,
    /// A tag this module does not classify, kept verbatim.
    Other(String),
}

/// A scalar node: its text and its resolved tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scalar {
    /// The scalar's text, exactly as it appears after YAML escaping.
    pub value: String,
    /// The resolved tag.
    pub tag: Tag,
}

/// A YAML node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// A scalar.
    Scalar(Scalar),
    /// A sequence.
    Sequence(Vec<Node>),
    /// A mapping. Pairs are kept in document order, and a repeated key is kept
    /// as two pairs: the shape check reports it, so it must not be collapsed
    /// here.
    Mapping(Vec<(Node, Node)>),
}

impl Node {
    /// The scalar at this node, if it is one.
    pub fn as_scalar(&self) -> Option<&Scalar> {
        match self {
            Node::Scalar(scalar) => Some(scalar),
            _ => None,
        }
    }

    /// The items of this node, if it is a sequence.
    pub fn as_sequence(&self) -> Option<&[Node]> {
        match self {
            Node::Sequence(items) => Some(items),
            _ => None,
        }
    }

    /// The pairs of this node, if it is a mapping.
    pub fn as_mapping(&self) -> Option<&[(Node, Node)]> {
        match self {
            Node::Mapping(pairs) => Some(pairs),
            _ => None,
        }
    }

    /// The first value stored under a scalar key, exactly like the reference
    /// implementation's mapping lookup.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.as_mapping()?
            .iter()
            .find(|(name, _)| name.as_scalar().map(|s| s.value.as_str()) == Some(key))
            .map(|(_, value)| value)
    }

    /// The text of this node when it is a scalar whose tag is "string".
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::Scalar(scalar) if scalar.tag == Tag::Str => Some(&scalar.value),
            _ => None,
        }
    }

    /// The text of this node when it is a scalar of any tag.
    pub fn scalar_text(&self) -> Option<&str> {
        self.as_scalar().map(|scalar| scalar.value.as_str())
    }

    /// True when this node is an explicit or implicit null.
    pub fn is_null(&self) -> bool {
        matches!(self.as_scalar(), Some(scalar) if scalar.tag == Tag::Null)
    }
}

/// A document that could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// The line the reference implementation blames, computed with its own
    /// rule; `0` when it reports none.
    pub line: u64,
    /// The parser's own message.
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(formatter, "yaml: {}", self.message)
        } else {
            write!(formatter, "yaml: line {}: {}", self.line, self.message)
        }
    }
}

impl std::error::Error for ParseError {}

/// Parses the first document in `text`.
///
/// Returns `Ok(None)` for an empty stream. Only the first document is decoded,
/// which is what the reference implementation's `Unmarshal` does.
pub fn parse(text: &str) -> Result<Option<Node>, ParseError> {
    // SAFETY: `parser` and `document` are zeroed, initialised by libyaml, and
    // released exactly once on every path below. The input string outlives the
    // parser: it is borrowed from the caller and not mutated.
    unsafe {
        let mut parser: yaml_parser_t = MaybeUninit::zeroed().assume_init();
        if !yaml_parser_initialize(&mut parser).ok {
            return Err(ParseError {
                line: 0,
                message: "cannot initialise the YAML parser".to_owned(),
            });
        }
        yaml_parser_set_input_string(&mut parser, text.as_ptr(), text.len() as u64);

        let mut document: yaml_document_t = MaybeUninit::zeroed().assume_init();
        let loaded = yaml_parser_load(&mut parser, &mut document).ok;

        let result = if !loaded {
            // libyaml has already released the half-built document on failure.
            Err(ParseError {
                line: error_line(&parser),
                message: cstr(parser.problem).to_owned(),
            })
        } else if document.nodes.start.is_null() || document.nodes.start == document.nodes.top {
            // No document was produced: the stream ended before one started.
            // (libyaml allocates the node stack eagerly, so emptiness is
            // "start == top", not a null pointer.)
            Ok(None)
        } else {
            // SAFETY: the document owns at least one node, and index 1 is its
            // root (`yaml_document_get_root_node`).
            let root = convert(&document, 1, 0);
            // An empty stream still produces one implicit null scalar. The
            // reference implementation leaves the document node empty instead,
            // and every caller reads that as "no document", so the two shapes
            // are folded together here. An explicit `null` keeps its text and
            // stays a real node.
            if matches!(&root, Node::Scalar(scalar) if scalar.tag == Tag::Null && scalar.value.is_empty())
            {
                Ok(None)
            } else {
                Ok(Some(root))
            }
        };

        if loaded {
            yaml_document_delete(&mut document);
        }
        yaml_parser_delete(&mut parser);
        result
    }
}

/// The deepest nesting this module will follow. Aliases make a YAML document a
/// graph, so a self-referential alias would otherwise recurse forever.
const MAX_DEPTH: u32 = 64;

/// Computes the line the reference implementation reports for a failed parse.
///
/// `gopkg.in/yaml.v3` prints the 0-based `context_mark.line`, falling back to
/// `problem_mark.line`, and adds one only for scanner errors. The value is
/// therefore not the 1-based line of the offending construct -- for an unclosed
/// flow sequence it is the line *before* it -- but it is the contract, and F16
/// carries it into the golden files.
fn error_line(parser: &yaml_parser_t) -> u64 {
    let scanner = parser.error == yaml_error_type_t::YAML_SCANNER_ERROR;
    let mut line = 0u64;
    if parser.context_mark.line != 0 {
        line = parser.context_mark.line;
        if scanner {
            line += 1;
        }
    } else if parser.problem_mark.line != 0 {
        line = parser.problem_mark.line;
        if scanner {
            line += 1;
        }
    }
    line
}

/// Reads a NUL-terminated string out of libyaml.
///
/// # Safety
///
/// `pointer` must be null or point at a NUL-terminated byte string that stays
/// alive for the call -- libyaml owns these strings for the parser's lifetime.
unsafe fn cstr<'a>(pointer: *const std::os::raw::c_char) -> &'a str {
    if pointer.is_null() {
        return "";
    }
    let bytes = pointer as *const u8;
    let mut length = 0usize;
    while *bytes.add(length) != 0 {
        length += 1;
    }
    std::str::from_utf8(std::slice::from_raw_parts(bytes, length)).unwrap_or("")
}

/// Converts one libyaml node (1-based index) into a [`Node`].
///
/// # Safety
///
/// `index` must name a node of `document`, which must be a fully loaded
/// document that is still alive.
unsafe fn convert(document: &yaml_document_t, index: i32, depth: u32) -> Node {
    if depth > MAX_DEPTH || index <= 0 {
        return Node::Scalar(Scalar {
            value: String::new(),
            tag: Tag::Null,
        });
    }
    let node: *const yaml_node_t = document.nodes.start.add(index as usize - 1);
    match (*node).type_ {
        yaml_node_type_t::YAML_SCALAR_NODE => {
            let scalar = (*node).data.scalar;
            // An empty scalar may carry a null pointer; `from_raw_parts`
            // requires a non-null pointer even for a zero length.
            let bytes: &[u8] = if scalar.value.is_null() || scalar.length == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(scalar.value, scalar.length as usize)
            };
            let value = String::from_utf8_lossy(bytes).into_owned();
            let explicit = cstr((*node).tag.cast());
            let tag = resolve_tag(explicit, scalar.style, &value);
            Node::Scalar(Scalar { value, tag })
        }
        yaml_node_type_t::YAML_SEQUENCE_NODE => {
            let items = (*node).data.sequence.items;
            let mut nodes = Vec::new();
            let mut cursor = items.start;
            while !cursor.is_null() && cursor < items.top {
                let item = *cursor;
                nodes.push(convert(document, item, depth + 1));
                cursor = cursor.add(1);
            }
            Node::Sequence(nodes)
        }
        yaml_node_type_t::YAML_MAPPING_NODE => {
            let pairs = (*node).data.mapping.pairs;
            let mut nodes = Vec::new();
            let mut cursor = pairs.start;
            while !cursor.is_null() && cursor < pairs.top {
                let pair = *cursor;
                nodes.push((
                    convert(document, pair.key, depth + 1),
                    convert(document, pair.value, depth + 1),
                ));
                cursor = cursor.add(1);
            }
            Node::Mapping(nodes)
        }
        // A node libyaml left empty. Treating it as null matches what the
        // reference implementation does with an empty document value.
        _ => Node::Scalar(Scalar {
            value: String::new(),
            tag: Tag::Null,
        }),
    }
}

/// Resolves a scalar's tag.
///
/// An explicit non-string tag wins. Otherwise a plain scalar is resolved the
/// way the reference implementation resolves it -- `true` is a boolean, `123`
/// an integer, `~` null -- and every quoted or block scalar is a string.
fn resolve_tag(explicit: &str, style: yaml_scalar_style_t, value: &str) -> Tag {
    match explicit {
        "" => resolve_implicit(style, value),
        TAG_STR => {
            if style == yaml_scalar_style_t::YAML_PLAIN_SCALAR_STYLE {
                resolve_implicit(style, value)
            } else {
                Tag::Str
            }
        }
        TAG_NULL => Tag::Null,
        TAG_BOOL => Tag::Bool,
        TAG_INT => Tag::Int,
        TAG_FLOAT => Tag::Float,
        TAG_TIMESTAMP => Tag::Timestamp,
        other => Tag::Other(other.to_owned()),
    }
}

/// Resolves the implicit tag of a plain scalar.
fn resolve_implicit(style: yaml_scalar_style_t, value: &str) -> Tag {
    if style != yaml_scalar_style_t::YAML_PLAIN_SCALAR_STYLE {
        return Tag::Str;
    }
    if is_null(value) {
        return Tag::Null;
    }
    if is_bool(value) {
        return Tag::Bool;
    }
    if is_int(value) {
        return Tag::Int;
    }
    if is_float(value) {
        return Tag::Float;
    }
    Tag::Str
}

fn is_null(value: &str) -> bool {
    matches!(value, "" | "~" | "null" | "Null" | "NULL")
}

fn is_bool(value: &str) -> bool {
    matches!(
        value,
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE"
    )
}

fn is_int(value: &str) -> bool {
    let digits = value.strip_prefix(['-', '+']).unwrap_or(value);
    if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        return !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit() || c == '_');
    }
    if let Some(octal) = digits
        .strip_prefix("0o")
        .or_else(|| digits.strip_prefix("0O"))
    {
        return !octal.is_empty() && octal.chars().all(|c| ('0'..='7').contains(&c) || c == '_');
    }
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '_')
}

fn is_float(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    if matches!(lowered.as_str(), ".inf" | "+.inf" | "-.inf" | ".nan") {
        return true;
    }
    if !value.contains(['.', 'e', 'E']) {
        return false;
    }
    value.parse::<f64>().is_ok()
}

const TAG_STR: &str = "tag:yaml.org,2002:str";
const TAG_NULL: &str = "tag:yaml.org,2002:null";
const TAG_BOOL: &str = "tag:yaml.org,2002:bool";
const TAG_INT: &str = "tag:yaml.org,2002:int";
const TAG_FLOAT: &str = "tag:yaml.org,2002:float";
const TAG_TIMESTAMP: &str = "tag:yaml.org,2002:timestamp";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_stream_has_no_document() {
        assert_eq!(parse("").expect("parse"), None);
        assert_eq!(parse("# only a comment\n").expect("parse"), None);
        assert_eq!(parse("\n\n").expect("parse"), None);
        // An explicit null is a real node, not an empty document.
        let explicit = parse("null\n").expect("parse").expect("document");
        assert!(explicit.is_null());
    }

    #[test]
    fn a_sequence_of_mappings_keeps_ids() {
        let text = "- id: one\n  name: a\n- id: two\n  name: b\n";
        let document = parse(text).expect("parse").expect("document");
        let items = document.as_sequence().expect("sequence");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].get("id").and_then(Node::as_str), Some("one"));
        assert_eq!(items[1].get("id").and_then(Node::as_str), Some("two"));
    }

    /// The golden contract for F16: the broken profile file is reported at
    /// line 1, because that is the mark the reference implementation uses --
    /// libyaml blames the line *before* the unclosed flow sequence.
    #[test]
    fn the_broken_cordis_is_reported_at_the_contract_line() {
        let text = "- id: dsh-base\n  enabled: [true, false\n";
        let error = parse(text).expect_err("must not parse");
        assert_eq!(error.line, 1);
        assert!(error.message.contains("expected"), "{}", error.message);
    }

    /// The same rule on inputs the golden files do not cover, measured against
    /// the reference implementation so a future parser swap cannot drift.
    #[test]
    fn parse_error_lines_match_the_reference_implementation() {
        let cases = [
            ("a: [1,\n 2,\n 3\n", 3u64),
            ("x: 1\ny: [1,\n 2\n", 1),
            ("x: 1\ny: 2\nz: [1,\n 2\n", 2),
            ("a: 1\n b: 2\n", 2),
            ("a:\n\t- 1\n", 2),
            ("a: \"abc\n", 2),
        ];
        for (text, expected) in cases {
            let error = parse(text).expect_err("must not parse");
            assert_eq!(error.line, expected, "for {text:?}");
        }
    }

    #[test]
    fn plain_scalars_resolve_to_their_implicit_tags() {
        let document = parse("a: true\nb: 123\nc: 1.5\nd: ~\ne: \"true\"\nf: text\n")
            .expect("parse")
            .expect("document");
        assert_eq!(
            document
                .get("a")
                .and_then(Node::as_scalar)
                .map(|s| s.tag.clone()),
            Some(Tag::Bool)
        );
        assert_eq!(
            document
                .get("b")
                .and_then(Node::as_scalar)
                .map(|s| s.tag.clone()),
            Some(Tag::Int)
        );
        assert_eq!(
            document
                .get("c")
                .and_then(Node::as_scalar)
                .map(|s| s.tag.clone()),
            Some(Tag::Float)
        );
        assert_eq!(
            document
                .get("d")
                .and_then(Node::as_scalar)
                .map(|s| s.tag.clone()),
            Some(Tag::Null)
        );
        // A quoted scalar stays a string even when its text looks like a bool.
        assert_eq!(
            document
                .get("e")
                .and_then(Node::as_scalar)
                .map(|s| s.tag.clone()),
            Some(Tag::Str)
        );
        assert_eq!(document.get("f").and_then(Node::as_str), Some("text"));
    }

    #[test]
    fn a_repeated_key_is_kept_as_two_pairs() {
        let document = parse("- id: a\n  id: b\n")
            .expect("parse")
            .expect("document");
        let entry = &document.as_sequence().expect("sequence")[0];
        let pairs = entry.as_mapping().expect("mapping");
        assert_eq!(pairs.len(), 2, "{pairs:?}");
    }

    #[test]
    fn a_tab_indented_document_is_rejected() {
        // yaml-rust2 accepted this; libyaml, and therefore yaml.v3, does not.
        assert!(parse("a:\n\t- 1\n").is_err());
    }

    #[test]
    fn nested_mappings_and_null_values_are_readable() {
        let document = parse("allowBuilds:\n  \"pkg\": true\n  other: false\n")
            .expect("parse")
            .expect("document");
        let block = document.get("allowBuilds").expect("allowBuilds");
        assert_eq!(
            block
                .get("pkg")
                .and_then(Node::as_scalar)
                .map(|s| s.tag.clone()),
            Some(Tag::Bool)
        );
        assert_eq!(
            block
                .get("other")
                .and_then(Node::as_scalar)
                .map(|s| s.tag.clone()),
            Some(Tag::Bool)
        );
    }
}
