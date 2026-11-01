//! F01, F02 and F04: the plugin-link checks.
//!
//! A profile resolves its bundles through `node_modules`: every bundle is a link
//! to a package, and an out-of-tree plugin reaches the profile's bundles through
//! bridges that pnpm builds into its own `node_modules`. The three checks here
//! test *reachability*, never intent: a link that reaches a `package.json` is
//! healthy, and the checks never guess where a broken one was supposed to point.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use o824_errs::Code;
use o824_model::{Category, Finding, Repair, Severity};
use serde_json::Value;

use crate::{
    params, rel_path, sort_findings, PARAM_PACKAGE, PARAM_PATH, PARAM_PLUGIN, PARAM_TARGET,
    PROFILES_DIR_NAME, REPAIR_KIND_AUTO,
};

/// F01: a profile `node_modules` link whose target exists but is not a package.
pub const CHECK_BROKEN_LINK: &str = "F01";
/// F02: a profile `node_modules` link whose target no longer exists.
pub const CHECK_MISSING_BUNDLE: &str = "F02";
/// F04: a plugin's dependency bridge into the profile's closure is missing.
pub const CHECK_BRIDGE_DANGLING: &str = "F04";

const MESSAGE_BROKEN_LINK: &str = "Broken plugin link";
const MESSAGE_MISSING_BUNDLE: &str = "Missing profile bundle";
const MESSAGE_BRIDGE_DANGLING: &str = "Dangling dependency bridge";

/// Asks plan to recreate a link from its target.
const PLAN_HINT_REBUILD_LINK: &str = "rebuild-link";
/// Asks plan to recreate a plugin's dependency bridge from the profile's own
/// bundle closure.
const PLAN_HINT_REBUILD_BRIDGE: &str = "rebuild-bridge";

/// The pnpm dependency tree inside a profile or plugin.
pub const NODE_MODULES_DIR_NAME: &str = "node_modules";
/// Marks a pnpm file-link dependency spec.
const LINK_PROTOCOL_PREFIX: &str = "link:";
/// The package scope a plugin bridges its DSH bundles from.
const DSH_SCOPE: &str = "@deepseek-ai/";

/// Returns the F01 and F02 findings for the entries under
/// `<root>\profiles\<profile>\node_modules`.
///
/// Every link below that tree is tested through, exactly as the app resolves a
/// bundle: `<link>\package.json` must open. A link that reaches a package is
/// healthy and produces no finding. A link that does not is F01 when its target
/// still exists (the target is there but is not a package) and F02 when the
/// target is gone (the bundle itself is missing).
pub fn links(root: &Path, profile: &str) -> Vec<Finding> {
    let node_modules = root
        .join(PROFILES_DIR_NAME)
        .join(profile)
        .join(NODE_MODULES_DIR_NAME);

    let mut findings = Vec::new();
    walk_no_follow(&node_modules, &mut |path, is_link| {
        if !is_link {
            return;
        }
        if fs::metadata(path.join("package.json")).is_ok() {
            // The link reaches a package: healthy.
            return;
        }

        let mut pairs: Vec<(&str, Value)> = vec![(PARAM_PATH, Value::from(rel_path(root, path)))];
        let target = fs::read_link(path)
            .ok()
            .map(|target| rel_path(root, &resolve_link_target(path, &target)));
        if let Some(target) = target {
            pairs.push((PARAM_TARGET, Value::from(target)));
        }

        // The link is unusable either way; only an absent target separates a
        // missing bundle from a broken link. A target we cannot stat, or that
        // exists but is not a package, is a broken link: never claim a bundle
        // is missing on the strength of an error that is not "not found".
        let (check_id, code, message) = match fs::metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => (
                CHECK_MISSING_BUNDLE,
                Code::BundleMissing,
                MESSAGE_MISSING_BUNDLE,
            ),
            _ => (CHECK_BROKEN_LINK, Code::LinkBroken, MESSAGE_BROKEN_LINK),
        };
        findings.push(plugin_finding(
            check_id,
            code,
            message,
            PLAN_HINT_REBUILD_LINK,
            params(&pairs),
        ));
    });

    sort_findings(&mut findings);
    findings
}

/// Returns the F04 findings for the `link:` plugins of
/// `<root>\profiles\<profile>\package.json`.
///
/// pnpm links an out-of-tree plugin instead of installing it, so the plugin's
/// own `@deepseek-ai` dependencies are satisfied by bridges the profile builds
/// into `<plugin>\node_modules`. Each peer dependency of the plugin in the
/// `@deepseek-ai` scope must resolve through its bridge to a `package.json`. A
/// missing or dangling bridge is F04; the bridge's intended target is never
/// guessed, only its reachability is tested.
///
/// A profile whose manifest cannot be read has no dependency closure to check,
/// and a plugin that did not land is reported by [`links`], so both are skipped
/// here rather than turned into bridge findings.
pub fn bridge(root: &Path, profile: &str) -> Vec<Finding> {
    let profile_dir = root.join(PROFILES_DIR_NAME).join(profile);
    let Some(manifest) = read_manifest(&profile_dir.join("package.json")) else {
        return Vec::new();
    };

    let mut findings = Vec::new();
    for name in sorted_keys(&manifest.dependencies) {
        let spec = &manifest.dependencies[&name];
        let Some(spec_path) = spec.trim().strip_prefix(LINK_PROTOCOL_PREFIX) else {
            continue;
        };
        // A `link:` spec is resolved by pnpm relative to the package that
        // declares it, which is the profile.
        let spec_path = spec_path.trim();
        if spec_path.is_empty() {
            continue;
        }
        // The spec is relative and may climb out of the profile
        // (`link:../../vendor/plugins/x`), so the joined path has to be cleaned
        // before it reaches a finding.
        let plugin_dir = o824_model::path::clean(&profile_dir.join(spec_path.replace('/', "\\")));
        let Some(plugin) = read_manifest(&plugin_dir.join("package.json")) else {
            // The plugin itself is missing or unreadable: `links` reports that,
            // and without its manifest its bridges cannot be known.
            continue;
        };

        for package in sorted_keys(&plugin.peer_dependencies) {
            if !package.starts_with(DSH_SCOPE) {
                continue;
            }
            let bridge = o824_model::path::clean(
                &plugin_dir
                    .join(NODE_MODULES_DIR_NAME)
                    .join(package.replace('/', "\\")),
            );
            if fs::metadata(bridge.join("package.json")).is_ok() {
                // The bridge reaches the profile's bundle: healthy.
                continue;
            }
            findings.push(plugin_finding(
                CHECK_BRIDGE_DANGLING,
                Code::BridgeDangling,
                MESSAGE_BRIDGE_DANGLING,
                PLAN_HINT_REBUILD_BRIDGE,
                params(&[
                    (PARAM_PATH, Value::from(rel_path(root, &bridge))),
                    (PARAM_PLUGIN, Value::from(name.clone())),
                    (PARAM_PACKAGE, Value::from(package.clone())),
                ]),
            ));
        }
    }

    sort_findings(&mut findings);
    findings
}

/// Builds one error-severity plugin finding. Every F01, F02 and F04 finding
/// shares this shape.
fn plugin_finding(
    check_id: &str,
    code: Code,
    message: &str,
    plan_hint: &str,
    params: serde_json::Map<String, Value>,
) -> Finding {
    Finding::new(check_id, Severity::Error, code, Category::Plugin)
        .with_params(params)
        .with_fallback_message(message)
        .with_repair(Repair {
            kind: REPAIR_KIND_AUTO.to_owned(),
            plan_hint: plan_hint.to_owned(),
        })
}

/// Walks `root` without ever following a link.
///
/// Every entry below `root` is passed to `visit` with `is_link` reporting
/// whether it is a reparse point. A link is reported but never entered, so a
/// junction cycle cannot loop the walk; `root` itself is not entered either when
/// it is a link.
///
/// The walk is forgiving on purpose, like discovery: a missing root has nothing
/// to report, and an unreadable directory is skipped rather than failing the
/// whole walk.
pub fn walk_no_follow(root: &Path, visit: &mut dyn FnMut(&Path, bool)) {
    match is_reparse_point(root) {
        // Missing or unreadable root: nothing to walk. An install-level fault is
        // owned by its own check, never invented here.
        Err(_) => return,
        // The contract is "never follow a link", and root is a link.
        Ok(true) => return,
        Ok(false) => {}
    }
    walk_dir(root, visit);
}

fn walk_dir(dir: &Path, visit: &mut dyn FnMut(&Path, bool)) {
    let Ok(entries) = fs::read_dir(dir) else {
        // Unreadable directory: skip it, never fail the whole walk.
        return;
    };
    let mut entries: Vec<fs::DirEntry> = entries.flatten().collect();
    // The reference implementation's `ReadDir` sorts by name, so the walk order
    // is deterministic; sorting here keeps the two implementations aligned.
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let is_link = match is_reparse_point(&path) {
            Ok(is_link) => is_link,
            // The entry vanished between listing and inspection.
            Err(_) => continue,
        };
        visit(&path, is_link);
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if is_dir && !is_link {
            walk_dir(&path, visit);
        }
    }
}

/// Reports whether `path` is a reparse point: a symbolic link, an NTFS
/// junction, or any other link-like entry.
///
/// The file attributes are the authoritative test. A junction is a directory
/// *and* a reparse point, which is why the walk checks the reparse bit instead
/// of the directory bit — and why the reply to "is this a link" is `true` for
/// entries whose file type already looks like a directory.
pub fn is_reparse_point(path: &Path) -> io::Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        /// The attribute Windows sets on every symbolic link and junction.
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

        let metadata = fs::symlink_metadata(path)?;
        Ok(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    {
        Ok(fs::symlink_metadata(path)?.file_type().is_symlink())
    }
}

/// Turns a link's stored target into an absolute path.
///
/// Windows stores absolute targets, but a link created elsewhere may hold a
/// relative one, which is relative to the link's own directory.
fn resolve_link_target(link: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        crate::strip_nt_namespace(target)
    } else {
        let parent = link.parent().unwrap_or(link);
        parent.join(target)
    }
}

/// The subset of a `package.json` the P0 checks read. Every field is optional,
/// so a manifest that omits one still parses.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct PackageManifest {
    pub(crate) dependencies: std::collections::BTreeMap<String, String>,
    pub(crate) peer_dependencies: std::collections::BTreeMap<String, String>,
    pub(crate) dsh: DshBlock,
}

/// The `dsh` block of a profile manifest.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct DshBlock {
    #[serde(default)]
    pub(crate) profile: ProfileBlock,
}

/// The `dsh.profile` block: the loader's bundle list.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct ProfileBlock {
    #[serde(default)]
    pub(crate) bundles: Vec<String>,
}

/// Reads the `package.json` at `path`. A missing or malformed file is an error,
/// never a fatal one: both callers treat it as "this node is not a package" and
/// leave the fault to the check that owns the node.
pub(crate) fn read_manifest(path: &Path) -> Option<PackageManifest> {
    let data = fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

/// The keys of a map in ascending order, so map iteration never leaks into
/// finding order.
fn sorted_keys(map: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    map.keys().cloned().collect()
}
