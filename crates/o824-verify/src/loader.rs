//! F03: one loader id, two entries.
//!
//! Two criteria were wrong before this one, and the machine tests are what corrected
//! them:
//!
//! * "A patch row whose id matches a bundle" flagged the *normal* case: a bundle defines
//!   an entry and the patch configures it. That is what `cordis.patch.yml` is for
//!   (2026-11-01).
//! * "A patch row naming an id no bundle provides" looked like the leftover DSH warns
//!   about, but bundles provide entries their package names never mention, so it produced
//!   eight warnings on a profile that works (2026-11-03).
//!
//! What the file can prove about itself: it gives **one id to two entries**. A row with
//! `id` and `name` defines an entry; an `insert:` row defines one too. Two definitions
//! under one id make DSH mount both and say nothing, so the plugin the user thinks is
//! running may not be. A row with `id` and `config` is a patch of an entry defined
//! elsewhere, and never counts as a definition.
//!
//! Warning, never a repair: which of the two entries the user meant is not something the
//! kernel can know, and deleting either would be a guess.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use o824_errs::Code;
use o824_model::{Category, Finding, Severity};
use serde_json::Value;

use crate::{params, sort_findings, PARAM_DUPLICATES, PARAM_IDS, PROFILES_DIR_NAME};

/// F03: a loader entry id the configuration gets wrong.
pub const CHECK_DUP_LOADER: &str = "F03";

const MESSAGE_DUP_LOADER: &str = "Loader entry id problem";

/// Returns the F03 finding for `<root>\profiles\<profile>\cordis.patch.yml`.
///
/// A definition is a row (or an `insert:` element) that carries both an `id` and a
/// `name`: it brings an entry into the tree. Two definitions sharing an id is the
/// finding; the `duplicates` parameter counts them and `ids` names them.
///
/// The check never fabricates F03 from an unreadable file. A `package.json` or
/// `cordis.patch.yml` that cannot be decoded is [`crate::profile`]'s fault (F16), and
/// reporting a loader problem as well would double-count one root cause.
pub fn loader_entries(root: &Path, profile: &str) -> Vec<Finding> {
    let profile_dir = root.join(PROFILES_DIR_NAME).join(profile);

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
    let Some(entries) = document.as_sequence() else {
        return Vec::new();
    };

    let mut definitions: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    let mut count_definition = |id: &str| {
        *definitions.entry(id.to_owned()).or_insert(0) += 1;
    };

    for entry in entries {
        let Some(pairs) = entry.as_mapping() else {
            return Vec::new();
        };
        // A repeated key makes the typed decode fail, so it is F16's.
        if has_repeated_key(pairs) {
            return Vec::new();
        }

        // A row defines an entry when it names both the id and the plugin behind it.
        if let (Some(id), Some(name)) = (entry.get("id"), entry.get("name")) {
            let (Some(id), Some(name)) = (id.as_str(), name.as_str()) else {
                // A non-string id cannot decode into the entry struct, and a null id
                // decodes to "" — the first is F16's, the second is skipped.
                continue;
            };
            if !id.is_empty() && !name.is_empty() {
                count_definition(id);
            }
        }

        // `insert:` defines entries too, one per element.
        if let Some(inserts) = entry.get("insert") {
            let Some(inserts) = inserts.as_sequence() else {
                continue;
            };
            for insert in inserts {
                let Some(id) = insert.get("id") else {
                    continue;
                };
                let Some(id) = id.as_str() else {
                    continue;
                };
                if id.is_empty() {
                    continue;
                }
                count_definition(id);
            }
        }
    }

    let duplicates: Vec<String> = definitions
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(id, _)| id)
        .collect();
    if duplicates.is_empty() {
        return Vec::new();
    }

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
