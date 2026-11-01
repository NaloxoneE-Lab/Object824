//! F17: a dependency pnpm fetches from source that the build whitelist does not
//! approve.
//!
//! pnpm 11 refuses to run a git or codeload dependency's build scripts unless
//! the whitelist approves that dependency by key, and a package that never ran
//! its build script is installed but unusable. Four rules decide the key, and
//! every one of them was measured on a real pnpm (see `docs/PORTING-NOTES.md`
//! §2):
//!
//! 1. `allowBuilds` is a **mapping** — `allowBuilds: {pkg: true}` — and a
//!    sequence is a hard parse error.
//! 2. `onlyBuiltDependencies` / `neverBuiltDependencies` were removed in v11.
//! 3. A bare package name never approves a dependency pnpm fetches from source:
//!    pnpm matches the repository form `name@git+https://host/org/repo.git`.
//! 4. That key carries the URL as pnpm normalises it, so a path with a space is
//!    written `%20`.

use std::path::Path;

use o824_errs::Code;
use o824_model::{Category, Finding, Repair, Severity};
use serde_json::Value;

use crate::profile::{allowed_builds, PROFILE_MANIFEST_FILE, WORKSPACE_FILE};
use crate::{params, sort_findings, PARAM_KEY, PARAM_PACKAGE, PROFILES_DIR_NAME, REPAIR_KIND_AUTO};

/// F17: a dependency pnpm would have to build from source without permission.
pub const CHECK_PNPM_BUILD_GATE: &str = "F17";

const MESSAGE_PNPM_BUILD_BLOCKED: &str = "pnpm build scripts are not allowed for this dependency";
/// Asks plan to add the package to the install's build whitelist.
const PLAN_HINT_ALLOW_BUILDS: &str = "allow-builds";

/// THE three `package.json` blocks that can declare a dependency. A name may
/// appear in more than one of them.
const DEPENDENCY_BLOCKS: [&str; 3] = ["dependencies", "devDependencies", "optionalDependencies"];

/// Marks a dependency pnpm fetches from a git repository.
const GIT_SPEC_PREFIX: &str = "git+";
/// Marks a dependency pnpm fetches through GitHub's shorthand.
const GITHUB_SPEC_PREFIX: &str = "github:";
/// Identifies the tarball URL pnpm downloads a GitHub dependency from; it is the
/// same repository as the `github:` shorthand.
const CODELOAD_MARKER: &str = "codeload";
const CODELOAD_HOST: &str = "codeload.github.com";
/// The fixed prefix of the repository form pnpm resolves both a `github:`
/// shorthand and a codeload tarball to.
const GITHUB_REPO_PREFIX: &str = "git+https://github.com/";
/// Separates a repository URL from the ref inside it.
const GIT_FRAGMENT_MARKER: char = '#';

/// One dependency pnpm fetches from source: the package name and the spec that
/// makes pnpm build it instead of unpacking a registry tarball.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDependency {
    /// The package name.
    pub name: String,
    /// The spec that makes it a source dependency.
    pub spec: String,
}

/// Returns the F17 findings for the source dependencies of
/// `<root>\profiles\<profile>\package.json` that the profile's
/// `pnpm-workspace.yaml` does not approve pnpm to build.
///
/// No workspace file at all proves nothing is allowed, so every source
/// dependency is reported: that is genuinely "pnpm would block this build". A
/// workspace file that is present but cannot be read or parsed is a different
/// fault — the whitelist is corrupt, not withheld — and is reported by F16
/// instead, so this check stays silent for it. A sequence under `allowBuilds` is
/// not that fault: it is the pre-v11 shape pnpm 11 rejects, it approves nothing,
/// and the automatic repair migrates it.
pub fn pnpm_build_gate(root: &Path, profile: &str) -> Vec<Finding> {
    let profile_dir = root.join(PROFILES_DIR_NAME).join(profile);
    let source_deps = git_dependencies(&profile_dir.join(PROFILE_MANIFEST_FILE));
    if source_deps.is_empty() {
        return Vec::new();
    }
    let (allowed, usable) = allowed_builds(&profile_dir.join(WORKSPACE_FILE));
    if !usable {
        // The workspace file establishes nothing; F16 reports the corrupt
        // whitelist, and no dependency is accused of being withheld.
        return Vec::new();
    }

    let mut findings: Vec<Finding> = source_deps
        .into_iter()
        .filter(|dependency| !approved(&allowed, dependency))
        .map(pnpm_build_finding)
        .collect();

    sort_findings(&mut findings);
    findings
}

/// Returns the profile's source dependencies, sorted by name.
///
/// A name declared in more than one block keeps the first spec that makes it a
/// source dependency, in [`DEPENDENCY_BLOCKS`] order, so the result is
/// deterministic. It returns nothing when the manifest cannot be read or parsed:
/// the fault belongs to F16.
pub fn git_dependencies(manifest_path: &Path) -> Vec<SourceDependency> {
    let Ok(data) = std::fs::read(manifest_path) else {
        return Vec::new();
    };
    let data = if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
        data[3..].to_vec()
    } else {
        data
    };
    let Ok(document) = serde_json::from_slice::<Value>(&data) else {
        return Vec::new();
    };
    let Some(manifest) = document.as_object() else {
        return Vec::new();
    };

    let mut specs: Vec<(String, String)> = Vec::new();
    for block in DEPENDENCY_BLOCKS {
        let Some(dependencies) = manifest.get(block).and_then(Value::as_object) else {
            // Absent, null or malformed: this block declares nothing F17 can
            // judge.
            continue;
        };
        for (name, spec) in dependencies {
            let Some(spec) = spec.as_str() else {
                continue;
            };
            if !is_source_dependency(spec) {
                continue;
            }
            if specs.iter().any(|(seen, _)| seen == name) {
                continue;
            }
            specs.push((name.clone(), spec.to_owned()));
        }
    }

    specs.sort_by(|left, right| left.0.cmp(&right.0));
    specs
        .into_iter()
        .map(|(name, spec)| SourceDependency { name, spec })
        .collect()
}

/// True when a dependency spec makes pnpm fetch the package from source, which
/// is the only case a build whitelist applies to.
pub fn is_source_dependency(spec: &str) -> bool {
    let value = spec.trim();
    value.starts_with(GIT_SPEC_PREFIX)
        || value.starts_with(GITHUB_SPEC_PREFIX)
        || value.contains(CODELOAD_MARKER)
}

/// Returns the `allowBuilds` keys that describe `dependency`, in order: the bare
/// package name, which is the key a registry dependency is approved by, then the
/// repository form pnpm 11 matches for a dependency it fetches from source.
///
/// The repository form is the URL without its `#fragment`, because pnpm approves
/// the repository and not one commit. A `github:` shorthand and a codeload
/// tarball are the same repository, so both resolve to the https URL pnpm
/// fetches them from.
pub fn approval_keys(dependency: &SourceDependency) -> Vec<String> {
    let mut keys = vec![dependency.name.clone()];
    let spec = dependency.spec.trim();

    if spec.starts_with(GIT_SPEC_PREFIX) {
        let base = spec_without_fragment(spec);
        if base.len() > GIT_SPEC_PREFIX.len() {
            keys.push(format!("{}@{}", dependency.name, normalize_git_spec(&base)));
        }
    } else if let Some(body) = spec.strip_prefix(GITHUB_SPEC_PREFIX) {
        if let Some(repo) = github_repo(body) {
            keys.push(format!(
                "{}@{}{}.git",
                dependency.name, GITHUB_REPO_PREFIX, repo
            ));
        }
    } else if spec.contains(CODELOAD_MARKER) {
        if let Some(repo) = codeload_repo(spec) {
            keys.push(format!(
                "{}@{}{}.git",
                dependency.name, GITHUB_REPO_PREFIX, repo
            ));
        }
    }
    keys
}

/// True when `allowed` holds a key pnpm 11 accepts for `dependency`.
///
/// Only the repository form can approve a dependency pnpm fetches from source: a
/// bare package name grants nothing there, and neither do the keys pnpm 11
/// removed. That is the rule this check exists for, so the bare name is skipped
/// even when the mapping carries it.
pub fn approved(
    allowed: &std::collections::BTreeSet<String>,
    dependency: &SourceDependency,
) -> bool {
    approval_keys(dependency)
        .into_iter()
        .filter(|key| *key != dependency.name)
        .any(|key| allowed.contains(&key))
}

/// The `allowBuilds` key a repair must write: the repository form whenever the
/// spec has one, and the package name only as the fallback for a spec that
/// cannot be spelled as a repository.
pub fn repair_key(dependency: &SourceDependency) -> String {
    approval_keys(dependency)
        .pop()
        .unwrap_or_else(|| dependency.name.clone())
}

/// Returns `spec` up to its `#fragment`: pnpm approves a git dependency by
/// repository, so the ref is not part of the key.
fn spec_without_fragment(spec: &str) -> String {
    match spec.find(GIT_FRAGMENT_MARKER) {
        Some(index) => spec[..index].to_owned(),
        None => spec.to_owned(),
    }
}

/// Returns `org/repo` for a `github:` shorthand body, or `None`.
fn github_repo(body: &str) -> Option<String> {
    let value = spec_without_fragment(body.trim());
    let value = value.strip_suffix(".git").unwrap_or(&value);
    let parts: Vec<&str> = value.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return None;
    }
    Some(value.to_owned())
}

/// Returns `org/repo` for a codeload tarball URL, or `None`.
fn codeload_repo(spec: &str) -> Option<String> {
    let mut reference = spec.trim().to_owned();
    if !reference.contains("://") {
        // pnpm accepts the host without a scheme too.
        reference = format!("https://{reference}");
    }
    let without_scheme = reference.split_once("://")?.1;
    let index = without_scheme.find('/')?;
    let (authority, path) = (&without_scheme[..index], &without_scheme[index + 1..]);
    if !authority.eq_ignore_ascii_case(CODELOAD_HOST) {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.len() < 2 || parts[0].is_empty() {
        return None;
    }
    let repo = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    if repo.is_empty() {
        return None;
    }
    Some(format!("{}/{}", parts[0], repo))
}

/// Spells a git spec the way pnpm prints it in the key it asks the user to
/// approve.
///
/// pnpm parses the spec as a URL, so a path containing a space comes back
/// percent-encoded: `git+file:///D:/dsh work/pkg` becomes
/// `git+file:///D:/dsh%20work/pkg`. The key we write must match that spelling
/// exactly, or the approval silently does nothing. A spec that does not parse as
/// a URL is returned unchanged: better to write the literal spelling than to
/// invent one.
pub fn normalize_git_spec(spec: &str) -> String {
    let raw = spec.strip_prefix(GIT_SPEC_PREFIX).unwrap_or(spec);
    match reserialize_url(raw) {
        Some(url) => format!("{GIT_SPEC_PREFIX}{url}"),
        None => spec.to_owned(),
    }
}

/// Re-spells a URL the way Go's `net/url` does: the path is percent-decoded and
/// then re-encoded, so an existing `%20` stays `%20` and a raw space becomes one.
fn reserialize_url(raw: &str) -> Option<String> {
    let (scheme, rest) = raw.split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    let (authority, tail) = match rest.find(['/', '?', '#']) {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    let (path, suffix) = match tail.find(['?', '#']) {
        Some(index) => (&tail[..index], &tail[index..]),
        None => (tail, ""),
    };
    let decoded = decode_percent(path)?;
    Some(format!(
        "{scheme}://{authority}{}{suffix}",
        encode_path(&decoded)
    ))
}

/// Decodes `%XX` escapes into a byte string.
///
/// A `%` that does not start a valid escape makes the URL unparseable, exactly
/// as it does for the reference implementation.
fn decode_percent(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Encodes a path the way the reference implementation's URL type does: the
/// unreserved characters and the path-legal reserved ones stay literal, and
/// everything else — a space above all — is escaped.
fn encode_path(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        let literal = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_'
                    | b'.'
                    | b'~'
                    | b'$'
                    | b'&'
                    | b'+'
                    | b','
                    | b'/'
                    | b':'
                    | b';'
                    | b'='
                    | b'@'
                    | b'!'
            );
        if literal {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Builds one F17 finding for a dependency the whitelist does not cover.
///
/// It carries the package name for display and, separately, the `allowBuilds`
/// key a repair has to write: for a dependency pnpm fetches from source those
/// are different strings.
fn pnpm_build_finding(dependency: SourceDependency) -> Finding {
    Finding::new(
        CHECK_PNPM_BUILD_GATE,
        Severity::Error,
        Code::PnpmBuildBlocked,
        Category::Plugin,
    )
    .with_params(params(&[
        (PARAM_KEY, Value::from(repair_key(&dependency))),
        (PARAM_PACKAGE, Value::from(dependency.name)),
    ]))
    .with_fallback_message(MESSAGE_PNPM_BUILD_BLOCKED)
    .with_repair(Repair {
        kind: REPAIR_KIND_AUTO.to_owned(),
        plan_hint: PLAN_HINT_ALLOW_BUILDS.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn dependency(name: &str, spec: &str) -> SourceDependency {
        SourceDependency {
            name: name.to_owned(),
            spec: spec.to_owned(),
        }
    }

    #[test]
    fn source_specs_are_recognised() {
        assert!(is_source_dependency("git+https://github.com/a/b.git#v1"));
        assert!(is_source_dependency("github:a/b"));
        assert!(is_source_dependency(
            "https://codeload.github.com/a/b/tar.gz/refs/tags/v1"
        ));
        assert!(!is_source_dependency("1.2.3"));
        assert!(!is_source_dependency("link:../../plugins/x"));
    }

    #[test]
    fn the_repository_form_drops_the_fragment() {
        let dep = dependency(
            "dshmarket",
            "git+https://github.com/deepseek-ai/dshmarket.git#v1.70.2",
        );
        assert_eq!(
            repair_key(&dep),
            "dshmarket@git+https://github.com/deepseek-ai/dshmarket.git"
        );
    }

    #[test]
    fn a_bare_name_never_approves_a_source_dependency() {
        let dep = dependency(
            "dshmarket",
            "git+https://github.com/deepseek-ai/dshmarket.git",
        );
        let mut allowed = BTreeSet::new();
        allowed.insert("dshmarket".to_owned());
        assert!(!approved(&allowed, &dep), "a bare name grants nothing");

        allowed.insert("dshmarket@git+https://github.com/deepseek-ai/dshmarket.git".to_owned());
        assert!(approved(&allowed, &dep));
    }

    #[test]
    fn a_space_in_the_path_is_written_percent_encoded() {
        let dep = dependency("pkg", "git+file:///D:/dsh work/pkg");
        assert_eq!(
            repair_key(&dep),
            "pkg@git+file:///D:/dsh%20work/pkg",
            "pnpm prints the encoded spelling, so the key has to match it"
        );
    }

    #[test]
    fn a_github_shorthand_and_a_codeload_tarball_are_the_same_repository() {
        let shorthand = dependency("pkg", "github:org/repo");
        assert_eq!(
            repair_key(&shorthand),
            "pkg@git+https://github.com/org/repo.git"
        );

        let tarball = dependency(
            "pkg",
            "https://codeload.github.com/org/repo/tar.gz/refs/tags/v1",
        );
        assert_eq!(
            repair_key(&tarball),
            "pkg@git+https://github.com/org/repo.git"
        );
    }

    #[test]
    fn the_git_dep_fixture_is_reported_with_its_repository_key() {
        let fixture = o824_fixtures::build("git-dep").expect("build");
        let findings = pnpm_build_gate(fixture.root(), "desktop");
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].params[PARAM_KEY],
            serde_json::json!("dshmarket@git+https://github.com/deepseek-ai/dshmarket.git")
        );
        assert_eq!(
            findings[0].params[PARAM_PACKAGE],
            serde_json::json!("dshmarket")
        );
    }

    #[test]
    fn the_ok_fixture_has_no_source_dependencies() {
        let fixture = o824_fixtures::build("ok").expect("build");
        assert!(pnpm_build_gate(fixture.root(), "desktop").is_empty());
    }
}
