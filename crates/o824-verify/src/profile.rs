//! F16: a profile file the loader cannot read.
//!
//! A *syntax* error is reported with the parser's line number. A file that
//! parses but carries an entry the loader cannot decode is reported with the
//! shape rule it breaks. The shape rule is not decoration: F03 reads the same
//! two files and stays silent when it cannot decode them, so without F16 such a
//! profile would be reported as perfectly healthy while the loader rejects it.
//!
//! `pnpm-workspace.yaml` is here because F17 has to read the same file, and a
//! whitelist that cannot be parsed is a corrupt file, not a withheld allowance:
//! reporting one F17 per source dependency would name the wrong cause and aim an
//! automatic rewrite at the broken file. F16 owns that fault instead, with the
//! reason `workspace-not-parseable`, and F17 stays silent.
//!
//! A missing file is not an F16: absence has its own meaning — a profile with no
//! patch file simply has no patch entries, an install with no workspace file
//! simply allows no builds — and the checks that read those files already treat
//! it that way.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use o824_errs::Code;
use o824_model::{Category, Finding, Repair, Severity};
use o824_yaml::{Node, Tag};
use serde_json::Value;

use crate::{
    params, rel_path, sort_findings, PARAM_FILE, PARAM_LINE, PARAM_REASON, PROFILES_DIR_NAME,
    REPAIR_KIND_AUTO,
};

/// F16: a profile file the loader cannot read.
pub const CHECK_PROFILE_PARSE: &str = "F16";

const MESSAGE_PROFILE_PARSE: &str = "Profile file is not parseable";
/// Asks plan to restore the profile's files from the installation's snapshot.
/// The profile tree is machine-generated, so a file that is not parseable is
/// restored rather than patched in place.
const PLAN_HINT_RESTORE_SNAPSHOT: &str = "restore-snapshot";

/// The three profile files F16 reads, relative to the profile directory.
pub const PROFILE_MANIFEST_FILE: &str = "package.json";
/// The loader's own entry list.
pub const PROFILE_CORDIS_FILE: &str = "cordis.yml";
/// The user's patch layer.
pub const PROFILE_PATCH_FILE: &str = "cordis.patch.yml";
/// The pnpm workspace file; note that it lives in the profile directory, not at
/// the install root.
pub const WORKSPACE_FILE: &str = "pnpm-workspace.yaml";

/// The shape reason: `package.json` is not an object.
pub const REASON_PROFILE_NOT_OBJECT: &str = "profile-not-object";
/// The shape reason: the bundle list is not a sequence of strings.
pub const REASON_BUNDLES_NOT_STRINGS: &str = "bundles-not-string-sequence";
/// The shape reason: the patch root is not a sequence.
pub const REASON_PATCH_ROOT_NOT_SEQUENCE: &str = "patch-root-not-sequence";
/// The shape reason: a patch entry is not a mapping.
pub const REASON_PATCH_ENTRY_NOT_MAPPING: &str = "patch-entry-not-mapping";
/// The shape reason: a patch entry has no usable id.
pub const REASON_PATCH_ENTRY_MISSING_ID: &str = "patch-entry-missing-id";
/// The shape reason: a patch entry's id is not a string.
pub const REASON_PATCH_ENTRY_ID_NOT_STRING: &str = "patch-entry-id-not-string";
/// The shape reason: a patch entry repeats one of its own keys.
pub const REASON_PATCH_ENTRY_DUPLICATE_KEY: &str = "patch-entry-duplicate-key";
/// The shape reason: the install's build whitelist cannot be parsed.
pub const REASON_WORKSPACE_NOT_PARSEABLE: &str = "workspace-not-parseable";

/// A profile file as the checks see it.
enum ProfileFile {
    /// The file is not there. Every caller reads that as "nothing to validate".
    Missing,
    /// The file is there but cannot be read at all.
    Unreadable,
    /// The file's contents, with a leading UTF-8 BOM stripped.
    Data(Vec<u8>),
}

/// Reads one profile file.
///
/// A leading UTF-8 BOM is stripped first. Node, pnpm and every editor that
/// writes one treat the file as valid, so reporting F16 for it would be a false
/// positive on a file the host reads happily — and on Windows a BOM is one
/// `Set-Content` away.
fn read_profile_file(path: &Path) -> ProfileFile {
    match fs::read(path) {
        Ok(bytes) => ProfileFile::Data(strip_utf8_bom(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => ProfileFile::Missing,
        Err(_) => ProfileFile::Unreadable,
    }
}

/// Removes a leading UTF-8 byte-order mark.
fn strip_utf8_bom(bytes: Vec<u8>) -> Vec<u8> {
    const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
    if bytes.starts_with(&BOM) {
        bytes[3..].to_vec()
    } else {
        bytes
    }
}

/// Returns the F16 findings for the profile's three files under
/// `<root>\profiles\<profile>` and for the profile's build whitelist.
pub fn profile_parse(root: &Path, profile: &str) -> Vec<Finding> {
    let dir = root.join(PROFILES_DIR_NAME).join(profile);

    let mut findings = Vec::new();
    findings.extend(parse_profile_manifest(
        root,
        &dir.join(PROFILE_MANIFEST_FILE),
    ));
    findings.extend(parse_profile_cordis(root, &dir.join(PROFILE_CORDIS_FILE)));
    findings.extend(parse_profile_patch(root, &dir.join(PROFILE_PATCH_FILE)));
    findings.extend(parse_profile_workspace(root, &dir.join(WORKSPACE_FILE)));

    sort_findings(&mut findings);
    findings
}

/// Checks that the profile's build whitelist can be established.
///
/// It shares [`allowed_builds`] with F17, so the two checks always agree on
/// whether the file is readable, and a file that is absent is not a finding.
fn parse_profile_workspace(root: &Path, path: &Path) -> Vec<Finding> {
    let (_, usable) = allowed_builds(path);
    if usable {
        return Vec::new();
    }
    vec![profile_parse_finding(
        root,
        path,
        shape_params(root, path, REASON_WORKSPACE_NOT_PARSEABLE),
    )]
}

/// Checks `package.json`: JSON syntax first, then the one block F03 decodes.
fn parse_profile_manifest(root: &Path, path: &Path) -> Vec<Finding> {
    let data = match read_profile_file(path) {
        ProfileFile::Missing => return Vec::new(),
        ProfileFile::Unreadable => {
            // The file is there but unreadable: report it, with no line to
            // point at.
            return vec![profile_parse_finding(
                root,
                path,
                parse_params(root, path, 0),
            )];
        }
        ProfileFile::Data(data) => data,
    };

    let document: Value = match serde_json::from_slice(&data) {
        Ok(document) => document,
        Err(error) => {
            return vec![profile_parse_finding(
                root,
                path,
                parse_params(root, path, error.line() as u64),
            )];
        }
    };
    match manifest_shape_reason(&document) {
        Some(reason) => vec![profile_parse_finding(
            root,
            path,
            shape_params(root, path, reason),
        )],
        None => Vec::new(),
    }
}

/// Checks `cordis.yml`'s syntax. Its root is the loader's own entry list and its
/// shape is the loader's business, not F16's: only a document that cannot be
/// parsed at all is a finding here.
fn parse_profile_cordis(root: &Path, path: &Path) -> Vec<Finding> {
    let data = match read_profile_file(path) {
        ProfileFile::Missing => return Vec::new(),
        ProfileFile::Unreadable => {
            return vec![profile_parse_finding(
                root,
                path,
                parse_params(root, path, 0),
            )];
        }
        ProfileFile::Data(data) => data,
    };
    let Ok(text) = String::from_utf8(data) else {
        return vec![profile_parse_finding(
            root,
            path,
            parse_params(root, path, 0),
        )];
    };
    match o824_yaml::parse(&text) {
        Ok(_) => Vec::new(),
        Err(error) => vec![profile_parse_finding(
            root,
            path,
            parse_params(root, path, error.line),
        )],
    }
}

/// Checks `cordis.patch.yml`'s syntax and then its shape.
///
/// The root must be a sequence, and every element a mapping with a non-empty
/// string id. A file with several violations yields one finding — the first —
/// because one broken file has one root cause and one repair.
fn parse_profile_patch(root: &Path, path: &Path) -> Vec<Finding> {
    let data = match read_profile_file(path) {
        ProfileFile::Missing => return Vec::new(),
        ProfileFile::Unreadable => {
            return vec![profile_parse_finding(
                root,
                path,
                parse_params(root, path, 0),
            )];
        }
        ProfileFile::Data(data) => data,
    };
    let Ok(text) = String::from_utf8(data) else {
        return vec![profile_parse_finding(
            root,
            path,
            parse_params(root, path, 0),
        )];
    };

    let document = match o824_yaml::parse(&text) {
        Ok(document) => document,
        Err(error) => {
            return vec![profile_parse_finding(
                root,
                path,
                parse_params(root, path, error.line),
            )];
        }
    };
    let Some(document) = document else {
        // An empty file is an empty document: it has no entries, which is what
        // a missing patch file means too. Nothing to load, nothing to report.
        return Vec::new();
    };
    let Some(entries) = document.as_sequence() else {
        return vec![profile_parse_finding(
            root,
            path,
            shape_params(root, path, REASON_PATCH_ROOT_NOT_SEQUENCE),
        )];
    };
    for entry in entries {
        if let Some(reason) = patch_entry_reason(entry) {
            return vec![profile_parse_finding(
                root,
                path,
                shape_params(root, path, reason),
            )];
        }
    }
    Vec::new()
}

/// Returns the rule a decoded `package.json` breaks, or `None` when its
/// loader-facing shape is usable.
///
/// A null value spells absence in JSON, and the typed manifest reader accepts
/// it, so `dsh`, `dsh.profile` and `dsh.profile.bundles` that are absent or null
/// are not findings: there is nothing to decode, not a wrong shape.
pub fn manifest_shape_reason(document: &Value) -> Option<&'static str> {
    let Some(manifest) = document.as_object() else {
        // package.json is an object by definition; anything else cannot carry
        // the blocks every consumer reads.
        return Some(REASON_PROFILE_NOT_OBJECT);
    };

    let value = manifest.get("dsh")?;
    if value.is_null() {
        return None;
    }
    let Some(dsh) = value.as_object() else {
        // `dsh` is the block that must hold profile.bundles, so a `dsh` that is
        // not an object cannot hold a bundle list.
        return Some(REASON_BUNDLES_NOT_STRINGS);
    };

    let value = dsh.get("profile")?;
    if value.is_null() {
        return None;
    }
    let Some(profile) = value.as_object() else {
        return Some(REASON_BUNDLES_NOT_STRINGS);
    };

    let value = profile.get("bundles")?;
    if value.is_null() {
        return None;
    }
    let Some(bundles) = value.as_array() else {
        return Some(REASON_BUNDLES_NOT_STRINGS);
    };
    if bundles.iter().any(|bundle| !bundle.is_string()) {
        return Some(REASON_BUNDLES_NOT_STRINGS);
    }
    None
}

/// Returns the rule one `cordis.patch.yml` element breaks, or `None` when it is
/// a mapping that either names an entry with a usable `id` or adds rows with `insert`.
fn patch_entry_reason(entry: &Node) -> Option<&'static str> {
    let Some(pairs) = entry.as_mapping() else {
        return Some(REASON_PATCH_ENTRY_NOT_MAPPING);
    };

    // The typed decode only rejects a repeated key when it decodes into a map,
    // which is exactly what F03 does. One repeated id would therefore switch F03
    // off silently, so the shape check reports it here.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (key, _) in pairs {
        let Some(name) = key.as_scalar() else {
            continue;
        };
        if !seen.insert(name.value.as_str()) {
            return Some(REASON_PATCH_ENTRY_DUPLICATE_KEY);
        }
    }

    // A patch row does one of two things: it names an existing entry with `id`, or it
    // adds rows with `insert`. DSH 0.2.1-rc.5 accepts the second shape — it composed
    // `- insert: [{id: probe-only, name: "@acme/probe"}]` into the config and exited 0 —
    // so demanding an `id` on every entry called a legal file unparseable.
    let has_insert = entry
        .get("insert")
        .map(|value| value.as_sequence().is_some())
        .unwrap_or(false);
    let Some(id) = entry.get("id") else {
        if has_insert {
            return None;
        }
        return Some(REASON_PATCH_ENTRY_MISSING_ID);
    };
    if has_insert && id.is_null() {
        // Both shapes in one row: the insert is what this row is for.
        return None;
    }
    if id.is_null() {
        return Some(REASON_PATCH_ENTRY_MISSING_ID);
    }
    let Some(scalar) = id.as_scalar() else {
        return Some(REASON_PATCH_ENTRY_ID_NOT_STRING);
    };
    if scalar.tag != Tag::Str {
        return Some(REASON_PATCH_ENTRY_ID_NOT_STRING);
    }
    if scalar.value.trim().is_empty() {
        return Some(REASON_PATCH_ENTRY_MISSING_ID);
    }
    None
}

/// Builds one F16 finding.
fn profile_parse_finding(
    _root: &Path,
    _path: &Path,
    params: serde_json::Map<String, Value>,
) -> Finding {
    Finding::new(
        CHECK_PROFILE_PARSE,
        Severity::Error,
        Code::ProfileParse,
        Category::Config,
    )
    .with_params(params)
    .with_fallback_message(MESSAGE_PROFILE_PARSE)
    .with_repair(Repair {
        kind: REPAIR_KIND_AUTO.to_owned(),
        plan_hint: PLAN_HINT_RESTORE_SNAPSHOT.to_owned(),
    })
}

/// Describes a file that could not be parsed: its root-relative path and the
/// parser's line, `0` when the parser reported none.
fn parse_params(root: &Path, path: &Path, line: u64) -> serde_json::Map<String, Value> {
    params(&[
        (PARAM_FILE, Value::from(rel_path(root, path))),
        (PARAM_LINE, Value::from(line)),
    ])
}

/// Describes a file that parsed but carries an entry of the wrong shape: its
/// root-relative path and the rule it breaks.
fn shape_params(root: &Path, path: &Path, reason: &str) -> serde_json::Map<String, Value> {
    params(&[
        (PARAM_FILE, Value::from(rel_path(root, path))),
        (PARAM_REASON, Value::from(reason)),
    ])
}

/// The approval keys the profile's pnpm workspace holds, and whether the
/// whitelist could be established at all.
///
/// pnpm 11 reads `allowBuilds` as a mapping — `allowBuilds: {pkg: true}` — and
/// rejects a sequence with a parse error, so a sequence approves nothing here.
/// It is not a corrupt file, though: it is the pre-v11 shape the repair
/// migrates, and the caller must treat the whitelist as established (`usable`)
/// with an empty set. `onlyBuiltDependencies` was removed in pnpm 11, so a
/// sequence under it grants nothing either; a mapping under it is still read for
/// tolerance.
///
/// A missing workspace file establishes the empty set: nothing is allowed, which
/// is how a tree without a whitelist behaves. A file that is there but cannot be
/// read, parsed, or is not a mapping establishes nothing — `usable` is false —
/// and the caller must not read that as "this dependency was withheld": a
/// corrupt whitelist is a parse fault, and F16 reports it.
pub(crate) fn allowed_builds(path: &Path) -> (BTreeSet<String>, bool) {
    let mut allowed = BTreeSet::new();

    let data = match read_profile_file(path) {
        ProfileFile::Missing => {
            // No whitelist at all: nothing is allowed.
            return (allowed, true);
        }
        ProfileFile::Unreadable => return (BTreeSet::new(), false),
        ProfileFile::Data(data) => data,
    };
    let Ok(text) = String::from_utf8(data) else {
        return (BTreeSet::new(), false);
    };
    let document = match o824_yaml::parse(&text) {
        Ok(document) => document,
        Err(_) => return (BTreeSet::new(), false),
    };
    let Some(document) = document else {
        // An empty document is a valid, empty workspace: it whitelists nothing.
        return (allowed, true);
    };
    if document.as_mapping().is_none() {
        // A whitelist that is not a mapping cannot be established.
        return (BTreeSet::new(), false);
    }

    for key in ["allowBuilds", "onlyBuiltDependencies"] {
        if let Some(list) = document.get(key) {
            collect_approvals(&mut allowed, list);
        }
    }
    (allowed, true)
}

/// Adds the keys of one whitelist node to `allowed`.
///
/// Only a mapping approves anything: pnpm 11 rejects the sequence form, so a
/// sequence — under either key — grants nothing. A mapping entry with an
/// explicit `false` withholds its approval.
fn collect_approvals(allowed: &mut BTreeSet<String>, list: &Node) {
    let Some(pairs) = list.as_mapping() else {
        return;
    };
    for (key, value) in pairs {
        let Some(name) = key.as_scalar() else {
            continue;
        };
        if name.value.is_empty() {
            continue;
        }
        if denies_build(value) {
            continue;
        }
        allowed.insert(name.value.clone());
    }
}

/// True when a map-form whitelist value withholds the allowance.
fn denies_build(value: &Node) -> bool {
    match value.as_scalar() {
        Some(scalar) => scalar.tag == Tag::Bool && scalar.value.eq_ignore_ascii_case("false"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_fixtures::{build, DSH_VERSION};

    #[test]
    fn the_parse_error_fixture_reports_line_one() {
        let fixture = build("parse-error").expect("build");
        let findings = profile_parse(fixture.root(), "desktop");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].check_id, CHECK_PROFILE_PARSE);
        assert_eq!(
            findings[0].params[PARAM_FILE],
            serde_json::json!("profiles\\desktop\\cordis.yml")
        );
        assert_eq!(findings[0].params[PARAM_LINE], serde_json::json!(1));
    }

    #[test]
    fn a_clean_tree_has_no_parse_findings() {
        let fixture = build("ok").expect("build");
        assert!(profile_parse(fixture.root(), "desktop").is_empty());
    }

    #[test]
    fn a_missing_profile_directory_is_not_a_fault() {
        let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
        assert!(profile_parse(dir.path(), "desktop").is_empty());
    }

    #[test]
    fn a_workspace_without_allow_builds_is_establishable() {
        let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
        let path = dir.path().join(WORKSPACE_FILE);
        std::fs::write(&path, "packages:\n  - .\n").expect("write");
        let (allowed, usable) = allowed_builds(&path);
        assert!(usable);
        assert!(allowed.is_empty());
    }

    #[test]
    fn a_sequence_allow_builds_grants_nothing_but_is_not_corrupt() {
        let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
        let path = dir.path().join(WORKSPACE_FILE);
        std::fs::write(&path, "allowBuilds:\n  - pkg\n").expect("write");
        let (allowed, usable) = allowed_builds(&path);
        assert!(usable, "the pre-v11 shape is migratable, not corrupt");
        assert!(allowed.is_empty());
    }

    #[test]
    fn an_unparseable_workspace_is_not_establishable() {
        let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
        let path = dir.path().join(WORKSPACE_FILE);
        std::fs::write(&path, "allowBuilds: [\n").expect("write");
        let (_, usable) = allowed_builds(&path);
        assert!(!usable);
    }

    #[test]
    fn explicit_false_withholds_an_approval() {
        let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
        let path = dir.path().join(WORKSPACE_FILE);
        std::fs::write(&path, "allowBuilds:\n  yes-pkg: true\n  no-pkg: false\n").expect("write");
        let (allowed, usable) = allowed_builds(&path);
        assert!(usable);
        assert!(allowed.contains("yes-pkg"));
        assert!(!allowed.contains("no-pkg"));
    }

    #[test]
    fn a_manifest_that_is_not_an_object_is_f16() {
        assert_eq!(
            manifest_shape_reason(&serde_json::json!([1, 2])),
            Some(REASON_PROFILE_NOT_OBJECT)
        );
        assert_eq!(
            manifest_shape_reason(&serde_json::json!({"dsh": {"profile": {"bundles": [1]}}})),
            Some(REASON_BUNDLES_NOT_STRINGS)
        );
        // Absent or null blocks are absence, not a wrong shape.
        assert_eq!(manifest_shape_reason(&serde_json::json!({})), None);
        assert_eq!(
            manifest_shape_reason(&serde_json::json!({"dsh": null})),
            None
        );
        assert_eq!(
            manifest_shape_reason(&serde_json::json!({
                "dsh": {"profile": {"bundles": [DSH_VERSION]}}
            })),
            None
        );
    }
}
