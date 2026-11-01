//! F03: duplicated loader entry ids.
//!
//! F03 is **advisory, not a fault**: on DSH 0.2.1-rc.3 the criterion is
//! unproven. A patch row's id is a loader entry id (`timer`, `dsh-ops/shell`),
//! not a bundle package name, and a row that names an entry the loader does not
//! know only produces a warning — `patch: entry "X" not found` — while DSH boots
//! normally. A genuinely duplicated loader entry id, injected with `insert:`,
//! did not break `--dump-config` either (see `docs/PORTING-NOTES.md` §1).
//!
//! The finding therefore has **no repair** and must never drive an automatic
//! edit: deleting a patch row on the strength of this intersection could remove
//! an override the user wrote on purpose. The criterion has to be re-derived
//! before this check can become repairable again.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use o824_errs::Code;
use o824_model::{Category, Finding, Severity};
use serde_json::Value;

use crate::link::read_manifest;
use crate::{params, sort_findings, PARAM_DUPLICATES, PARAM_IDS, PROFILES_DIR_NAME};

/// F03: a duplicated loader entry.
pub const CHECK_DUP_LOADER: &str = "F03";

const MESSAGE_DUP_LOADER: &str = "Duplicate plugin entry";

/// Returns the F03 finding for `<root>\profiles\<profile>\cordis.patch.yml`: an
/// id that the profile's `dsh.profile.bundles` and the patch file both name.
///
/// A bundle is identified by the last segment of its package name — the id
/// cordis loads it under — so `@deepseek-ai/dsh-base` intersects with a patch
/// entry id `dsh-base`. The two id sets are intersected; a non-empty
/// intersection is one finding whose `duplicates` parameter is the
/// intersection's size.
///
/// The check never fabricates F03 from an unreadable file. A `package.json` or
/// `cordis.patch.yml` that cannot be decoded is [`crate::profile`]'s fault
/// (F16), and reporting a duplicate as well would double-count one root cause.
/// That is the ruled division of labour, not an oversight. For the same reason a
/// missing patch file, or a manifest that omits `dsh.profile.bundles`, means "no
/// duplicates": there is nothing to compare, not a fault.
pub fn loader_entries(root: &Path, profile: &str) -> Vec<Finding> {
    let profile_dir = root.join(PROFILES_DIR_NAME).join(profile);

    // Malformed or missing manifest: F16 owns it.
    let Some(manifest) = read_manifest(&profile_dir.join("package.json")) else {
        return Vec::new();
    };
    let bundles = &manifest.dsh.profile.bundles;
    if bundles.is_empty() {
        // No bundle ids means nothing can collide.
        return Vec::new();
    }

    // A missing patch file has no entries; an unreadable one is F16's.
    let Ok(patch_text) = fs::read(profile_dir.join("cordis.patch.yml")) else {
        return Vec::new();
    };
    let Ok(patch_text) = String::from_utf8(patch_text) else {
        return Vec::new();
    };
    let Ok(Some(document)) = o824_yaml::parse(&patch_text) else {
        // Invalid YAML (or a root that is not an array): F16 owns it.
        return Vec::new();
    };
    // The typed decode in the reference implementation rejects anything that is
    // not a sequence of mappings with a string `id`; such a file is F16's.
    let Some(entries) = document.as_sequence() else {
        return Vec::new();
    };

    let mut patch_ids: BTreeSet<String> = BTreeSet::new();
    for entry in entries {
        let Some(pairs) = entry.as_mapping() else {
            return Vec::new();
        };
        // A repeated key makes the typed decode fail, so it is F16's.
        if has_repeated_key(pairs) {
            return Vec::new();
        }
        let Some(id) = entry.get("id") else {
            continue;
        };
        let Some(id) = id.as_str() else {
            // A non-string id cannot decode into the entry struct, and a null
            // id decodes to "" — the first is F16's, the second is skipped.
            return Vec::new();
        };
        if !id.is_empty() {
            patch_ids.insert(id.to_owned());
        }
    }
    if patch_ids.is_empty() {
        return Vec::new();
    }

    // Deduplicate the bundle ids first, so `duplicates` counts the size of the
    // id sets' intersection even if a manifest lists one package twice.
    let mut seen_bundles: BTreeSet<String> = BTreeSet::new();
    let mut duplicates: Vec<String> = Vec::new();
    for bundle in bundles {
        let id = loader_id(bundle);
        if id.is_empty() {
            continue;
        }
        if !seen_bundles.insert(id.clone()) {
            continue;
        }
        if patch_ids.contains(&id) {
            duplicates.push(id);
        }
    }
    if duplicates.is_empty() {
        return Vec::new();
    }
    duplicates.sort();

    let finding = Finding::new(
        CHECK_DUP_LOADER,
        Severity::Warning,
        Code::DupLoader,
        Category::Config,
    )
    .with_params(params(&[
        (PARAM_DUPLICATES, Value::from(duplicates.len())),
        (PARAM_IDS, Value::from(duplicates)),
    ]))
    .with_fallback_message(MESSAGE_DUP_LOADER);

    let mut findings = vec![finding];
    sort_findings(&mut findings);
    findings
}

/// True when a mapping repeats a scalar key.
fn has_repeated_key(pairs: &[(o824_yaml::Node, o824_yaml::Node)]) -> bool {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (key, _) in pairs {
        let Some(name) = key.as_scalar() else {
            continue;
        };
        if !seen.insert(name.value.as_str()) {
            return true;
        }
    }
    false
}

/// Returns the loader entry id a bundle package name registers under: the last
/// slash-separated segment. `@deepseek-ai/dsh-base` is loaded as `dsh-base`.
pub fn loader_id(package: &str) -> String {
    let name = package.trim();
    match name.rsplit_once('/') {
        Some((_, last)) => last.to_owned(),
        None => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loader_id_is_the_last_segment() {
        assert_eq!(loader_id("@deepseek-ai/dsh-base"), "dsh-base");
        assert_eq!(loader_id("dsh-echo"), "dsh-echo");
        assert_eq!(loader_id("  @a/b  "), "b");
        assert_eq!(loader_id(""), "");
    }

    #[test]
    fn the_dup_loader_fixture_reports_one_collision() {
        let fixture = o824_fixtures::build("dup-loader").expect("build");
        let findings = loader_entries(fixture.root(), "desktop");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].check_id, CHECK_DUP_LOADER);
        assert_eq!(findings[0].params[PARAM_DUPLICATES], serde_json::json!(1));
        assert_eq!(
            findings[0].params[PARAM_IDS],
            serde_json::json!(["dsh-echo"])
        );
        assert!(findings[0].repair.is_none(), "F03 is advisory: no repair");
    }

    #[test]
    fn the_ok_fixture_has_no_collision() {
        let fixture = o824_fixtures::build("ok").expect("build");
        assert!(loader_entries(fixture.root(), "desktop").is_empty());
    }

    #[test]
    fn a_missing_profile_directory_is_not_a_fault() {
        let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
        assert!(loader_entries(dir.path(), "desktop").is_empty());
    }
}
