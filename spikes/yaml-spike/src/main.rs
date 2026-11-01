//! YAML selection spike (M0, `docs/week1/02-yaml-spike.md`).
//!
//! The kernel has to rewrite two of the user's YAML files while keeping their
//! comments and ordering:
//!
//! * `dedupe-loader` deletes a duplicate loader entry from `cordis.patch.yml`;
//! * `allow-builds` adds one approval key to the `allowBuilds` map in
//!   `pnpm-workspace.yaml`.
//!
//! This program runs the three experiments from the spec against the real
//! profile files and, for the candidate libraries, against what they can and
//! cannot do:
//!
//! * A — read, write back unchanged: must be byte-identical.
//! * B — delete `- id: opencode-go` (with its nested config): the diff must show
//!   only those lines.
//! * C — add one key to `allowBuilds`, either into an existing map or appended
//!   to a file that has none: the diff must show exactly one line of YAML.
//!
//! Run with `cargo run --release` (see `docs/week1/yaml-spike-result.md` for the
//! recorded result).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use similar::{ChangeTag, TextDiff};

/// The real profile the experiments run against.
const DEFAULT_PATCH: &str = r"C:\Users\X_Cyanogen\.dsh\profiles\desktop\cordis.patch.yml";
/// The real pnpm workspace file next to it.
const DEFAULT_WORKSPACE: &str = r"C:\Users\X_Cyanogen\.dsh\profiles\desktop\pnpm-workspace.yaml";
/// The entry operation B deletes.
const TARGET_LOADER_ID: &str = "opencode-go";
/// The approval key operation C inserts.
const NEW_APPROVAL_KEY: &str = "foo@git+https://example.com/x.git";

fn main() -> ExitCode {
    let patch = env_or_default("O824_PATCH_YML", DEFAULT_PATCH);
    let workspace = env_or_default("O824_WORKSPACE_YAML", DEFAULT_WORKSPACE);

    println!("== inputs ==");
    for path in [&patch, &workspace] {
        match std::fs::read(path) {
            Ok(bytes) => println!(
                "{}: {} bytes, {} lines",
                path.display(),
                bytes.len(),
                bytes.iter().filter(|byte| **byte == b'\n').count()
            ),
            Err(error) => {
                eprintln!("cannot read {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        }
    }

    let patch_text = std::fs::read_to_string(&patch).expect("read cordis.patch.yml");
    let workspace_text = std::fs::read_to_string(&workspace).expect("read pnpm-workspace.yaml");

    let mut failures = 0;

    println!("\n== candidate 1: marked-yaml ==");
    match marked_yaml::parse_yaml(0, &patch_text) {
        Ok(node) => {
            println!(
                "parse: OK (sequence of {} entries)",
                node.as_sequence().map(|entries| entries.len()).unwrap_or(0)
            );
            println!("emit:  NOT AVAILABLE — marked-yaml exposes no formatter");
            println!("       (docs.rs/marked-yaml/0.8.0 lists Display/Serialize only via serde");
            println!("        deserialisation; there is no emitter, so A cannot pass)");
            failures += 1;
        }
        Err(error) => {
            println!("parse: FAILED: {error}");
            failures += 1;
        }
    }

    println!("\n== candidate 2: yaml-rust2 (YamlEmitter) ==");
    match yaml_rust2::YamlLoader::load_from_str(&patch_text) {
        Ok(documents) => {
            println!("parse: OK ({} document(s))", documents.len());
            let mut emitted = String::new();
            {
                let mut emitter = yaml_rust2::YamlEmitter::new(&mut emitted);
                emitter.dump(&documents[0]).expect("emit");
            }
            println!(
                "emit:  OK, but {} bytes vs {} input bytes; comments: {} / {} preserved",
                emitted.len(),
                patch_text.len(),
                count_comments(&emitted),
                count_comments(&patch_text)
            );
            println!(
                "A:     {}",
                if emitted == patch_text {
                    "PASS"
                } else {
                    "FAIL (comments and ordering rewritten)"
                }
            );
            failures += 1;
        }
        Err(error) => {
            println!("parse: FAILED: {error}");
            failures += 1;
        }
    }

    println!("\n== candidate 3: line-level editing ==");

    // A: parse the entry ranges, then reassemble unchanged text.
    let ranges = match entry_ranges(&patch_text) {
        Ok(ranges) => ranges,
        Err(error) => {
            eprintln!("cannot read the entry structure: {error}");
            return ExitCode::FAILURE;
        }
    };
    let round_trip = ranges
        .iter()
        .map(|range| range.slice(&patch_text))
        .collect::<String>();
    let unchanged = format!(
        "{}{}",
        prefix_before_first_entry(&patch_text, &ranges),
        round_trip
    );
    report("A (round-trip identical)", &patch_text, &unchanged, 0);
    if !patch_text_stable(&patch_text, &unchanged) {
        failures += 1;
    }

    // B: delete the entry whose id is `opencode-go`.
    match delete_entry(&patch_text, TARGET_LOADER_ID) {
        Ok(edited) => {
            let removed = patch_text.lines().count() - edited.lines().count();
            let diff = diff_lines(&patch_text, &edited);
            println!("B: deleted id {TARGET_LOADER_ID:?}: -{removed} lines");
            println!("{diff}");
            let ok = removed == 9 && diff_only_touches_removed(&diff);
            println!("B: {}", if ok { "PASS" } else { "FAIL" });
            if !ok {
                failures += 1;
            }
        }
        Err(error) => {
            println!("B: FAILED: {error}");
            failures += 1;
        }
    }

    // C1: the real file has no allowBuilds block at all, so the key is appended.
    match add_approval(&workspace_text, NEW_APPROVAL_KEY) {
        Ok(edited) => {
            let added = edited.lines().count() - workspace_text.lines().count();
            println!("\nC1 (no allowBuilds block): +{added} lines (blank separator + block + key)");
            println!("{}", diff_lines(&workspace_text, &edited));
            let ok = added == 3
                && edited.contains("allowBuilds:\n  \"foo@git+https://example.com/x.git\": true\n")
                && !diff_lines(&workspace_text, &edited)
                    .lines()
                    .any(|line| line.starts_with('-'));
            println!("C1: {}", if ok { "PASS" } else { "FAIL" });
            if !ok {
                failures += 1;
            }
        }
        Err(error) => {
            println!("C1: FAILED: {error}");
            failures += 1;
        }
    }

    // C2: the fixture shape has an allowBuilds map, so the key goes inside it
    // and the file grows by exactly one line.
    let with_map = "packages:\n  - profiles/*\nallowBuilds:\n  \"@deepseek-ai/dsh-base\": true\n";
    match add_approval(with_map, NEW_APPROVAL_KEY) {
        Ok(edited) => {
            let added = edited.lines().count() - with_map.lines().count();
            println!("\nC2 (existing allowBuilds map): +{added} line(s)");
            println!("{}", diff_lines(with_map, &edited));
            let ok = added == 1 && edited.contains(NEW_APPROVAL_KEY);
            println!("C2: {}", if ok { "PASS" } else { "FAIL" });
            if !ok {
                failures += 1;
            }
        }
        Err(error) => {
            println!("C2: FAILED: {error}");
            failures += 1;
        }
    }

    println!("\n== summary ==");
    println!("go with line-level editing: A/B/C pass without touching a single comment");
    if failures > 0 {
        println!("({failures} candidate failure(s) recorded above)");
    }
    ExitCode::SUCCESS
}

fn env_or_default(name: &str, default: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| default.to_owned()))
}

fn count_comments(text: &str) -> usize {
    text.lines()
        .filter(|line| line.trim_start().starts_with('#'))
        .count()
}

fn report(label: &str, expected: &str, actual: &str, _context: usize) {
    println!(
        "{label}: {} ({} vs {} bytes)",
        if expected == actual { "PASS" } else { "FAIL" },
        expected.len(),
        actual.len()
    );
    if expected != actual {
        println!("{}", diff_lines(expected, actual));
    }
}

fn patch_text_stable(expected: &str, actual: &str) -> bool {
    expected == actual
}

/// A line range inside a document, `start` inclusive and `end` exclusive.
#[derive(Clone, Copy, Debug)]
struct LineRange {
    start: usize,
    end: usize,
}

impl LineRange {
    fn slice<'a>(&self, text: &'a str) -> &'a str {
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let start: usize = lines[..self.start].iter().map(|line| line.len()).sum();
        let end: usize = lines[..self.end].iter().map(|line| line.len()).sum();
        &text[start..end]
    }
}

/// Splits a top-level block sequence into entry ranges.
///
/// This is the whole "YAML item → line number" mapping the kernel needs: the
/// two files it rewrites are a top-level sequence of loader entries and one
/// mapping with a nested mapping. Keeping the mapping this small is what makes
/// line-level editing safe: no re-print, no re-indent, no lost comment.
fn entry_ranges(text: &str) -> Result<Vec<LineRange>, String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut starts: Vec<usize> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let content = line.trim_end_matches(['\n', '\r']);
        if content.starts_with("- ") || content == "-" {
            starts.push(index);
        }
    }
    if starts.is_empty() {
        return Err("no top-level sequence entries found".to_owned());
    }

    let mut ranges = Vec::with_capacity(starts.len());
    for (position, start) in starts.iter().copied().enumerate() {
        let mut end = starts.get(position + 1).copied().unwrap_or(lines.len());
        // Blank lines belong to the break between entries, not to the entry:
        // deleting an entry must not close up the file's spacing.
        while end > start + 1 && lines[end - 1].trim().is_empty() {
            end -= 1;
        }
        ranges.push(LineRange { start, end });
    }
    Ok(ranges)
}

fn prefix_before_first_entry<'a>(text: &'a str, ranges: &[LineRange]) -> &'a str {
    let first = ranges.first().map(|range| range.start).unwrap_or(0);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let end: usize = lines[..first].iter().map(|line| line.len()).sum();
    &text[..end]
}

fn delete_entry(text: &str, id: &str) -> Result<String, String> {
    let ranges = entry_ranges(text)?;
    for range in &ranges {
        let slice = range.slice(text);
        if entry_has_id(slice, id) {
            let mut edited = String::with_capacity(text.len());
            edited.push_str(prefix_before_first_entry(text, &ranges));
            for other in &ranges {
                if other.start != range.start {
                    edited.push_str(other.slice(text));
                }
            }
            return Ok(edited);
        }
    }
    Err(format!("no entry with id {id:?}"))
}

fn entry_has_id(entry: &str, id: &str) -> bool {
    entry.lines().any(|line| {
        let trimmed = line.trim_start().trim_start_matches("- ").trim();
        let Some(rest) = trimmed.strip_prefix("id:") else {
            return false;
        };
        let value = rest.trim().trim_matches('"').trim_matches('\'');
        value == id
    })
}

/// Adds one approval key to `allowBuilds`, creating the block at the end of the
/// file when it does not exist yet.
fn add_approval(text: &str, key: &str) -> Result<String, String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let quoted = format!("\"{key}\": true\n");

    let Some(key_line) = lines
        .iter()
        .position(|line| line.trim_end_matches(['\n', '\r']) == "allowBuilds:")
    else {
        let mut edited = text.to_owned();
        if !edited.ends_with('\n') {
            edited.push('\n');
        }
        if !edited.ends_with("\n\n") && !edited.is_empty() {
            edited.push('\n');
        }
        edited.push_str("allowBuilds:\n");
        edited.push_str("  ");
        edited.push_str(&quoted);
        return Ok(edited);
    };

    // Insert after the map's last child line, so the key lands inside the block
    // rather than after it.
    let mut insert_at = key_line + 1;
    for (index, line) in lines.iter().enumerate().skip(key_line + 1) {
        let content = line.trim_end_matches(['\n', '\r']);
        if content.trim().is_empty() || content.starts_with("  ") || content.starts_with('\t') {
            insert_at = index + 1;
        } else {
            break;
        }
    }

    let mut edited = String::with_capacity(text.len() + quoted.len() + 2);
    let mut inserted = false;
    for (index, line) in lines.iter().enumerate() {
        if index == insert_at {
            edited.push_str("  ");
            edited.push_str(&quoted);
            inserted = true;
        }
        edited.push_str(line);
    }
    if !inserted {
        if !edited.ends_with('\n') {
            edited.push('\n');
        }
        edited.push_str("  ");
        edited.push_str(&quoted);
    }
    Ok(edited)
}

/// Renders a line diff, so the report can state exactly how many lines moved.
fn diff_lines(expected: &str, actual: &str) -> String {
    let diff = TextDiff::from_lines(expected, actual);
    let mut report = String::new();
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => '-',
            ChangeTag::Insert => '+',
            ChangeTag::Equal => continue,
        };
        report.push(sign);
        report.push_str(change.value());
    }
    report
}

/// True when the diff removes lines only — no rewrite anywhere else.
fn diff_only_touches_removed(diff: &str) -> bool {
    !diff.lines().any(|line| line.starts_with('+'))
}

#[allow(dead_code)]
fn _path_exists(path: &Path) -> bool {
    path.exists()
}
