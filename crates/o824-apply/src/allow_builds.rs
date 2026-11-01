//! The `allow-builds` step: add approval keys to a pnpm workspace whitelist.
//!
//! pnpm 11 reads `allowBuilds` as a **mapping** — `allowBuilds: {pkg: true}` —
//! and a sequence is a hard parse error ("The allowBuilds field should be an
//! object, but got array"). The key is an approval key, not necessarily a package
//! name: pnpm approves a dependency it fetches from source only by its repository
//! form (`name@git+<url>`).
//!
//! # Why this edits lines instead of re-emitting a YAML tree
//!
//! The reference implementation parses the file into `yaml.v3` nodes and
//! re-emits it. A Rust YAML library cannot round-trip comments (see
//! `docs/week1/yaml-spike-result.md`), so this module edits the file's lines in
//! place instead: every comment, blank line and unrelated key keeps its exact
//! bytes. The trade is that the new key's spelling is ours rather than the
//! emitter's — always valid YAML, sometimes double-quoted where a tree emitter
//! would have left it bare. pnpm accepts both, and the post-repair diagnosis is
//! what proves the file still reads.

use std::fs;
use std::io;
use std::path::Path;

use o824_errs::{Code, Error};

/// The whitelist key pnpm 11 reads.
const ALLOW_BUILDS_KEY: &str = "allowBuilds";

/// What the edit did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// True when the file changed.
    pub changed: bool,
    /// How many pre-v11 sequence entries were migrated to the map form.
    pub migrated: usize,
}

/// Sets `key: true` for every key in `keys`, keeping every other byte of the file
/// untouched.
///
/// A missing file becomes the smallest valid whitelist. An absent `allowBuilds`
/// key is appended; a `allowBuilds:` with no value becomes an empty mapping; the
/// pre-v11 sequence form is migrated in place to the map form; an explicit
/// `false` is flipped to `true`, because withholding the allowance is exactly the
/// fault being repaired.
pub fn set_approvals(path: &Path, keys: &[String]) -> Result<Outcome, Error> {
    let wanted: Vec<&str> = keys
        .iter()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
        .collect();
    if wanted.is_empty() {
        return Ok(Outcome::default());
    }

    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(file_error("read the build whitelist", path, &error)),
    };

    let mut lines: Vec<String> = split_keep_newlines(&text);
    let mut outcome = Outcome::default();

    match find_key_line(&lines, ALLOW_BUILDS_KEY) {
        None => append_block(&mut lines, &wanted),
        Some(key_line) => {
            let shape = shape_of(&lines, key_line);
            match shape {
                Shape::Mapping { last_child } => {
                    insert_into_mapping(&mut lines, last_child, &wanted, &mut outcome);
                }
                Shape::Sequence { items } => {
                    outcome.migrated = migrate_sequence(&mut lines, &items, &wanted);
                }
                Shape::Empty => {
                    // `allowBuilds:` with no value is an empty mapping, not a
                    // broken one.
                    let mut insertion: Vec<String> = Vec::new();
                    for key in &wanted {
                        insertion.push(format!("  {}\n", entry(key)));
                    }
                    lines.splice(key_line + 1..key_line + 1, insertion);
                }
                Shape::InlineSequence => {
                    // An inline `allowBuilds: [a, b]` is rewritten as a block
                    // mapping; the pre-v11 shape cannot grant anything.
                    let existing = inline_sequence_items(&lines[key_line]);
                    outcome.migrated = existing.len();
                    lines[key_line] = format!("{ALLOW_BUILDS_KEY}:\n");
                    let mut insertion: Vec<String> = Vec::new();
                    for item in existing
                        .iter()
                        .map(String::as_str)
                        .chain(wanted.iter().copied())
                    {
                        insertion.push(format!("  {}\n", entry(item)));
                    }
                    lines.splice(key_line + 1..key_line + 1, insertion);
                }
                Shape::Other => {
                    return Err(Error::new(
                        Code::Internal,
                        format!(
                            "apply: the build whitelist {:?} has an allowBuilds that is neither a mapping nor the pre-v11 sequence",
                            path.display().to_string()
                        ),
                    ));
                }
            }
        }
    }

    let updated: String = lines.into_iter().collect();
    if updated == text {
        return Ok(outcome);
    }
    write_atomically(path, updated.as_bytes())?;
    outcome.changed = true;
    Ok(outcome)
}

/// The shape of the value that follows an `allowBuilds:` line.
enum Shape {
    /// A block mapping whose last child line is `last_child` (0-based).
    Mapping { last_child: usize },
    /// A block sequence: the 0-based indices of its item lines.
    Sequence { items: Vec<usize> },
    /// No value at all.
    Empty,
    /// An inline `[a, b]` sequence.
    InlineSequence,
    /// Something this editor does not know how to repair.
    Other,
}

/// Classifies the value of the key at `key_line`.
fn shape_of(lines: &[String], key_line: usize) -> Shape {
    let value = value_of(&lines[key_line]);
    if !value.is_empty() {
        return if value.starts_with('[') {
            Shape::InlineSequence
        } else {
            Shape::Other
        };
    }

    let mut last_child = key_line;
    let mut items: Vec<usize> = Vec::new();
    let mut block: Option<bool> = None;
    for (index, line) in lines.iter().enumerate().skip(key_line + 1) {
        let content = trim_newline(line);
        if content.trim().is_empty() {
            continue;
        }
        if !is_indented(content) {
            break;
        }
        last_child = index;
        let trimmed = content.trim_start();
        let is_item = trimmed.starts_with("- ") || trimmed == "-";
        match block {
            None => block = Some(is_item),
            Some(sequence) if sequence != is_item => {
                // Mixed shapes: not a document this editor understands.
                return Shape::Other;
            }
            Some(_) => {}
        }
        if is_item {
            items.push(index);
        }
    }

    match block {
        None => Shape::Empty,
        Some(true) => Shape::Sequence { items },
        Some(false) => Shape::Mapping { last_child },
    }
}

/// Adds the wanted keys at the end of an existing mapping block.
fn insert_into_mapping(
    lines: &mut Vec<String>,
    last_child: usize,
    wanted: &[&str],
    outcome: &mut Outcome,
) {
    let mut insertion: Vec<String> = Vec::new();
    for key in wanted {
        if let Some(index) = find_entry_line(lines, key) {
            // An explicit `false` withholds the allowance, which is the fault
            // being repaired; flip it rather than leave it denied.
            if is_false_value(&lines[index]) {
                lines[index] = format!("  {}\n", entry(key));
                outcome.changed = true;
            }
            continue;
        }
        insertion.push(format!("  {}\n", entry(key)));
    }
    if insertion.is_empty() {
        return;
    }
    lines.splice(last_child + 1..last_child + 1, insertion);
    outcome.changed = true;
}

/// Rewrites a pre-v11 sequence as the map form, preserving each item's trailing
/// comment. Returns how many of the file's own entries were migrated.
fn migrate_sequence(lines: &mut Vec<String>, items: &[usize], wanted: &[&str]) -> usize {
    let mut seen: Vec<String> = Vec::new();
    let mut entries: Vec<String> = Vec::new();
    for index in items {
        let content = trim_newline(&lines[*index]);
        let item = content.trim_start().trim_start_matches('-').trim();
        if item.is_empty() {
            continue;
        }
        let plain = unquote(item);
        if seen.contains(&plain) {
            continue;
        }
        seen.push(plain.clone());
        entries.push(format!(
            "  {}
",
            entry(&plain)
        ));
    }
    let migrated = entries.len();
    for key in wanted {
        if seen.iter().any(|existing| existing == key) {
            continue;
        }
        seen.push((*key).to_owned());
        entries.push(format!(
            "  {}
",
            entry(key)
        ));
    }

    // Replace the whole item block, from the first item line to the last.
    let first = *items.first().expect("a sequence has at least one item");
    let last = *items.last().expect("a sequence has at least one item");
    lines.splice(first..=last, entries);
    migrated
}

/// Appends a fresh `allowBuilds` block at the end of the file.
fn append_block(lines: &mut Vec<String>, wanted: &[&str]) {
    if let Some(last) = lines.last_mut() {
        if !last.ends_with('\n') {
            last.push('\n');
        }
    }
    if !lines.is_empty()
        && !lines
            .last()
            .map(|line| line.trim().is_empty())
            .unwrap_or(false)
    {
        lines.push("\n".to_owned());
    }
    lines.push(format!("{ALLOW_BUILDS_KEY}:\n"));
    for key in wanted {
        lines.push(format!("  {}\n", entry(key)));
    }
}

/// Finds the top-level line that declares `key`.
fn find_key_line(lines: &[String], key: &str) -> Option<usize> {
    lines.iter().position(|line| {
        let content = trim_newline(line);
        if is_indented(content) {
            return false;
        }
        let Some((name, _)) = content.split_once(':') else {
            return false;
        };
        name.trim() == key
    })
}

/// Finds an entry line for `key` inside the `allowBuilds` block.
fn find_entry_line(lines: &[String], key: &str) -> Option<usize> {
    lines.iter().position(|line| {
        let content = trim_newline(line);
        if !is_indented(content) {
            return false;
        }
        let trimmed = content.trim_start();
        if trimmed.starts_with('-') {
            return false;
        }
        let Some((name, _)) = trimmed.split_once(':') else {
            return false;
        };
        unquote(name.trim()) == key
    })
}

/// The text after the colon on a key line, trimmed, with any trailing comment
/// removed.
fn value_of(line: &str) -> String {
    let content = trim_newline(line);
    let Some((_, value)) = content.split_once(':') else {
        return String::new();
    };
    let mut value = value.trim();
    if let Some(index) = value.find(" #") {
        value = value[..index].trim();
    }
    value.trim_matches('"').trim_matches('\'').trim().to_owned()
}

/// The items of an inline `[a, b]` sequence on a key line.
fn inline_sequence_items(line: &str) -> Vec<String> {
    let value = value_of(line);
    let inner = value.trim_start_matches('[').trim_end_matches(']').trim();
    if inner.is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(|item| unquote(item.trim()))
        .filter(|item| !item.is_empty())
        .collect()
}

/// True when an entry line's value is an explicit `false`.
fn is_false_value(line: &str) -> bool {
    value_of(line).eq_ignore_ascii_case("false")
}

/// True when a line is indented (a child of the previous key).
fn is_indented(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

/// Strips a trailing newline and carriage return.
fn trim_newline(line: &str) -> &str {
    line.trim_end_matches('\n').trim_end_matches('\r')
}

/// Removes matching quotes.
fn unquote(text: &str) -> String {
    let trimmed = text.trim();
    for quote in ['"', '\''] {
        if trimmed.len() >= 2 && trimmed.starts_with(quote) && trimmed.ends_with(quote) {
            return trimmed[1..trimmed.len() - 1].to_owned();
        }
    }
    trimmed.to_owned()
}

/// Renders `key: true` with the key quoted only when it has to be.
fn entry(key: &str) -> String {
    if needs_quotes(key) {
        format!(
            "\"{}\": true",
            key.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        format!("{key}: true")
    }
}

/// True when a key cannot be written as a plain scalar.
fn needs_quotes(key: &str) -> bool {
    key.is_empty()
        || key.starts_with([
            '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%',
            '@', '`',
        ])
        || key.contains([':', '#', '\n', '\r', '\t'])
        || key.ends_with(' ')
        || key.starts_with(' ')
}

/// Writes `data` over `path` through a temporary file and a rename.
fn write_atomically(path: &Path, data: &[u8]) -> Result<(), Error> {
    let Some(directory) = path.parent() else {
        return Err(file_error(
            "derive the whitelist directory",
            path,
            &io::Error::other("the path has no parent directory"),
        ));
    };
    fs::create_dir_all(directory)
        .map_err(|error| file_error("create the whitelist directory", directory, &error))?;
    let temporary = directory.join(format!(
        "{}.o824-new-{}-{}",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "pnpm-workspace.yaml".to_owned()),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.subsec_nanos())
            .unwrap_or(0)
    ));
    let written = (|| -> io::Result<()> {
        fs::write(&temporary, data)?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(file_error("write the build whitelist", path, &error));
    }
    Ok(())
}

/// Splits text into lines that keep their terminators.
fn split_keep_newlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if ch == '\n' {
            lines.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn file_error(operation: &str, path: &Path, cause: &io::Error) -> Error {
    let code = if cause.kind() == io::ErrorKind::PermissionDenied {
        Code::Permission
    } else {
        Code::Internal
    };
    Error::new(
        code,
        format!(
            "apply: cannot {operation} {:?}: {cause}",
            path.display().to_string()
        ),
    )
    .with_param("error", cause.to_string())
    .with_param("path", path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_fixtures::TempDir;

    fn file_with(contents: &str) -> (TempDir, std::path::PathBuf) {
        let directory = TempDir::new("o824-allow-").expect("temp");
        let path = directory.path().join("pnpm-workspace.yaml");
        fs::write(&path, contents).expect("write");
        (directory, path)
    }

    fn keys(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn an_existing_mapping_only_gains_the_new_key() {
        let (_dir, path) = file_with(
            "# keep this comment\npackages:\n  - profiles/*\nallowBuilds:\n  \"@deepseek-ai/dsh-base\": true\n",
        );
        let outcome = set_approvals(&path, &keys(&["dshmarket@git+https://github.com/x/y.git"]))
            .expect("edit");
        assert!(outcome.changed);
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.starts_with("# keep this comment\n"), "{text}");
        assert!(
            text.contains("  \"@deepseek-ai/dsh-base\": true\n"),
            "{text}"
        );
        assert!(
            text.contains("  \"dshmarket@git+https://github.com/x/y.git\": true\n"),
            "{text}"
        );
    }

    #[test]
    fn a_missing_key_is_appended_as_a_map() {
        let (_dir, path) = file_with("packages:\n  - .\n");
        set_approvals(&path, &keys(&["pkg"])).expect("edit");
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.ends_with("allowBuilds:\n  pkg: true\n"), "{text}");
    }

    #[test]
    fn a_missing_file_becomes_the_smallest_whitelist() {
        let directory = TempDir::new("o824-allow-").expect("temp");
        let path = directory.path().join("pnpm-workspace.yaml");
        set_approvals(&path, &keys(&["pkg"])).expect("edit");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "allowBuilds:\n  pkg: true\n"
        );
    }

    /// The pre-v11 sequence form is what pnpm 11 rejects, so the repair migrates
    /// it in place.
    #[test]
    fn a_sequence_is_migrated_to_the_map_form() {
        let (_dir, path) = file_with("allowBuilds:\n  - pkg-a\n  - pkg-b\n");
        let outcome = set_approvals(&path, &keys(&["pkg-c"])).expect("edit");
        assert_eq!(outcome.migrated, 2);
        let text = fs::read_to_string(&path).expect("read");
        assert_eq!(
            text,
            "allowBuilds:\n  pkg-a: true\n  pkg-b: true\n  pkg-c: true\n"
        );
    }

    #[test]
    fn an_empty_allow_builds_becomes_a_mapping() {
        let (_dir, path) = file_with("allowBuilds:\n");
        set_approvals(&path, &keys(&["pkg"])).expect("edit");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "allowBuilds:\n  pkg: true\n"
        );
    }

    #[test]
    fn an_inline_sequence_is_migrated() {
        let (_dir, path) = file_with("allowBuilds: [pkg-a]\n");
        set_approvals(&path, &keys(&["pkg-b"])).expect("edit");
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains("pkg-a: true"), "{text}");
        assert!(text.contains("pkg-b: true"), "{text}");
    }

    #[test]
    fn an_explicit_false_is_flipped() {
        let (_dir, path) = file_with("allowBuilds:\n  pkg: false\n");
        let outcome = set_approvals(&path, &keys(&["pkg"])).expect("edit");
        assert!(outcome.changed);
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "allowBuilds:\n  pkg: true\n"
        );
    }

    #[test]
    fn a_key_already_approved_is_left_alone() {
        let (_dir, path) = file_with("allowBuilds:\n  pkg: true\n");
        let outcome = set_approvals(&path, &keys(&["pkg"])).expect("edit");
        assert!(!outcome.changed);
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "allowBuilds:\n  pkg: true\n"
        );
    }

    #[test]
    fn the_edited_file_is_still_valid_yaml() {
        let (_dir, path) = file_with("packages:\n  - profiles/*\n");
        set_approvals(&path, &keys(&["dshmarket@git+https://github.com/x/y.git"])).expect("edit");
        let text = fs::read_to_string(&path).expect("read");
        let document = o824_yaml::parse(&text).expect("parse").expect("document");
        let allowed = document.get("allowBuilds").expect("allowBuilds");
        assert!(allowed
            .get("dshmarket@git+https://github.com/x/y.git")
            .is_some());
    }
}
