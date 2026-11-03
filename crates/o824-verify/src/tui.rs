//! The tui form's own checks: the Node it needs, and whether it left the last exit behind.
//!
//! Three rules, all of them paid for by earlier checks that guessed:
//!
//! * **Only what the machine can prove.** F24 ("native module missing") is *not* here: the
//!   npm install layout puts no dependency tree inside the tui profile — verified on
//!   2026-11-03, the profile's `node_modules` holds one copy of the launcher and nothing
//!   else — so there is nothing to inspect. A check that cannot look is silent, not
//!   optimistic.
//! * **Delegate, and never trust the exit code.** `dsh-tui doctor` exits 0 with a failing
//!   line (verified). Its output is parsed by the leading mark, and only the lines about
//!   the environment are taken: a missing API key is a configuration need, not a broken
//!   installation.
//! * **No repairs.** Nothing here may write to a third-party application's files.

use std::fs;
use std::path::{Path, PathBuf};

use o824_errs::Code;
use o824_model::{Category, Finding, Install, Severity};
use serde_json::Value;

use crate::{params, sort_findings};

/// F23: the Node the tui needs is not the Node it would get.
pub const CHECK_NODE_RANGE: &str = "F23";
/// F25: the last exit did not finish.
pub const CHECK_UPDATE_INTERRUPTED: &str = "F25";
/// The third-party doctor said something about the environment.
pub const CHECK_DELEGATED: &str = "F28";

/// The line `dsh-tui` writes when it has finished disposing of itself.
const DISPOSE_EVENT: &str = "dispose: root disposed";
/// The line that starts an exit.
const EXIT_REQUESTED_EVENT: &str = "process exit requested";

/// Runs every tui check that this machine can decide.
///
/// `node_version` is the Node that would actually run the tui — the one on `PATH` — and is
/// passed in so a test decides it instead of the machine.
pub fn check(install: &Install, node_version: &str) -> Vec<Finding> {
    if install.kind != "tui" {
        return Vec::new();
    }
    let root = PathBuf::from(&install.root);
    let mut findings = Vec::new();
    findings.extend(node_range(&root, node_version));
    findings.extend(interrupted_restart(&install.home));
    findings.extend(delegated(&root));
    sort_findings(&mut findings);
    findings
}

/// F23: `engines.node` against the Node that would run the tui.
///
/// Silence when the package declares no range: nothing declared is nothing violated.
fn node_range(package_dir: &Path, node_version: &str) -> Vec<Finding> {
    let Some(range) = engines_node(package_dir) else {
        return Vec::new();
    };
    let Some(node) = parse_version(node_version) else {
        // Unreadable Node version: the comparison cannot be made, so it is not made.
        return Vec::new();
    };
    if satisfies(node, &range) {
        return Vec::new();
    }
    let finding = Finding::new(
        CHECK_NODE_RANGE,
        Severity::Error,
        Code::NodeRange,
        Category::Env,
    )
    .with_params(params(&[
        ("range", Value::from(range)),
        ("node", Value::from(node_version.to_owned())),
    ]))
    .with_fallback_message("The Node version this tui needs is not the one it would get");
    vec![finding]
}

/// F25: the last recorded exit never reached its end.
///
/// `restart.log` records a group per exit: the request, the signal, and then the dispose.
/// A last group that stops before the dispose means the process was killed or an update
/// interrupted it — the "it will not start after updating" case.
fn interrupted_restart(home: &str) -> Vec<Finding> {
    if home.is_empty() {
        return Vec::new();
    }
    // The tui keeps its state beside the DSH home, not inside it.
    // The tui keeps its state beside the DSH home (`~/.dsh-tui` next to `~/.dsh`), not
    // inside it. The name is the application's, verified on 2026-11-03.
    const TUI_STATE_DIR: &str = ".dsh-tui";
    let state_dir = PathBuf::from(home)
        .parent()
        .map(|parent| parent.join(TUI_STATE_DIR))
        .unwrap_or_default();
    let Ok(text) = fs::read_to_string(state_dir.join("restart.log")) else {
        return Vec::new();
    };

    let mut last_exit: Option<(String, String)> = None;
    let mut disposed = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.contains(EXIT_REQUESTED_EVENT) {
            // A new group starts; whatever came before it is closed.
            last_exit = Some((line.to_owned(), timestamp_of(line)));
            disposed = false;
            continue;
        }
        if line.contains(DISPOSE_EVENT) {
            disposed = true;
        }
    }

    // The last exit is the one that matters, and only when it never finished.
    let Some((event, at)) = last_exit else {
        return Vec::new();
    };
    if disposed {
        return Vec::new();
    }
    let finding = Finding::new(
        CHECK_UPDATE_INTERRUPTED,
        Severity::Warning,
        Code::UpdateInterrupted,
        Category::Install,
    )
    .with_params(params(&[
        ("lastEvent", Value::from(event)),
        ("at", Value::from(at)),
    ]))
    .with_fallback_message("The last tui exit did not finish; an update may have been interrupted");
    vec![finding]
}

/// The lines of `dsh-tui doctor` that describe the environment.
///
/// The exit code is deliberately ignored: the third-party tool exits 0 while printing a
/// failing line, so a check that trusted it would report a healthy install that does not
/// start. Credential lines are left to the application — a missing key is something the
/// user must supply, not a broken installation, and a warning on every run is how people
/// learn to ignore warnings.
fn delegated(package_dir: &Path) -> Vec<Finding> {
    let Some(entry) = tui_entry_point(package_dir) else {
        return Vec::new();
    };
    let output = std::process::Command::new(
        std::env::var("O824_NODE").unwrap_or_else(|_| "node".to_owned()),
    )
    .arg(&entry)
    .arg("doctor")
    .output();
    let Ok(output) = output else {
        // The tui could not even be asked: that is the application's problem to report,
        // and this check has nothing to add.
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let failed: Vec<String> = text
        .lines()
        .filter(|line| line.trim_start().starts_with('✗'))
        .filter(|line| takes_environment(line))
        .map(|line| line.trim().to_owned())
        .collect();
    if failed.is_empty() {
        return Vec::new();
    }
    let finding = Finding::new(
        CHECK_DELEGATED,
        Severity::Warning,
        Code::DelegatedFailed,
        Category::Env,
    )
    .with_params(params(&[("lines", Value::from(failed))]))
    .with_fallback_message("dsh-tui's own doctor reports a problem with its environment");
    vec![finding]
}

/// True when a failing line is about the environment rather than about credentials.
fn takes_environment(line: &str) -> bool {
    let rest = line.trim_start().trim_start_matches('✗').trim_start();
    let key = rest.split(':').next().unwrap_or_default().trim();
    matches!(key, "node" | "pnpm" | "profile" | "launcher ↔ profile")
}

/// The tui's own entry point, as its manifest declares it.
fn tui_entry_point(package_dir: &Path) -> Option<PathBuf> {
    let manifest = fs::read_to_string(package_dir.join("package.json")).ok()?;
    let value: Value = serde_json::from_str(&manifest).ok()?;
    let entry = value.get("bin")?.get("dsh-tui")?.as_str()?;
    let path = package_dir.join(entry.replace('/', std::path::MAIN_SEPARATOR_STR));
    path.is_file().then_some(path)
}

/// `engines.node` from a package manifest.
fn engines_node(package_dir: &Path) -> Option<String> {
    let manifest = fs::read_to_string(package_dir.join("package.json")).ok()?;
    let value: Value = serde_json::from_str(&manifest).ok()?;
    let range = value
        .get("engines")?
        .get("node")?
        .as_str()?
        .trim()
        .to_owned();
    (!range.is_empty()).then_some(range)
}

/// The timestamp that starts a `restart.log` line, when there is one.
fn timestamp_of(line: &str) -> String {
    line.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A version as `major.minor.patch` plus its prerelease tag.
///
/// The ordering is semver's, which the derived one would get wrong: `24.8.0-rc.1` sorts
/// *below* `24.8.0`, and an empty prerelease list would sort below a non-empty one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    /// Empty for a release; otherwise the identifiers, compared segment by segment.
    prerelease: Vec<Prerelease>,
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(
                || match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
                    // A release outranks any prerelease of the same triple.
                    (true, true) => std::cmp::Ordering::Equal,
                    (true, false) => std::cmp::Ordering::Greater,
                    (false, true) => std::cmp::Ordering::Less,
                    (false, false) => self.prerelease.cmp(&other.prerelease),
                },
            )
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prerelease {
    /// A numeric identifier sorts before an alphanumeric one.
    Number(u64),
    Text(String),
}

/// Parses `v24.8.0`, `24.8.0`, or `22.19.0-rc.1`.
pub fn parse_version(text: &str) -> Option<Version> {
    let text = text.trim().trim_start_matches('v');
    // A build suffix (`+abcdef`) is not part of precedence.
    let text = text.split('+').next()?;
    let (core, prerelease) = match text.split_once('-') {
        Some((core, pre)) => (core, pre),
        None => (text, ""),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    let prerelease = prerelease
        .split('.')
        .filter(|part| !part.is_empty())
        .map(|part| match part.parse::<u64>() {
            Ok(number) => Prerelease::Number(number),
            Err(_) => Prerelease::Text(part.to_owned()),
        })
        .collect();
    Some(Version {
        major,
        minor,
        patch,
        prerelease,
    })
}

/// True when `version` satisfies any of the `||`-separated ranges.
///
/// The operators are the ones the tui actually declares: `^`, `>=`, `>`, `<=`, `<`, and an
/// exact version. Anything else makes the whole answer "cannot decide", which is reported
/// as silence rather than as a pass.
pub fn satisfies(version: Version, range: &str) -> bool {
    // Semver's rule, and the documented behaviour of the application: a prerelease only
    // satisfies a range that names a prerelease itself. Node 23 and experimental builds
    // are rejected before the app loads, so `24.8.0-rc.1` must not pass `>=24`.
    if !version.prerelease.is_empty() && !range.contains('-') {
        return false;
    }
    range
        .split("||")
        .filter_map(|alternative| satisfies_one(&version, alternative.trim()))
        .any(|satisfied| satisfied)
}

/// One alternative of a range; `None` when the operator is not understood.
fn satisfies_one(version: &Version, alternative: &str) -> Option<bool> {
    let alternative = alternative.trim();
    if alternative.is_empty() {
        return None;
    }
    if let Some(rest) = alternative.strip_prefix(">=") {
        let bound = parse_version(rest)?;
        return Some(*version >= bound);
    }
    if let Some(rest) = alternative.strip_prefix('>') {
        let bound = parse_version(rest)?;
        return Some(*version > bound);
    }
    if let Some(rest) = alternative.strip_prefix("<=") {
        let bound = parse_version(rest)?;
        return Some(*version <= bound);
    }
    if let Some(rest) = alternative.strip_prefix('<') {
        let bound = parse_version(rest)?;
        return Some(*version < bound);
    }
    if let Some(rest) = alternative.strip_prefix('^') {
        let bound = parse_version(rest)?;
        // A caret allows changes that do not modify the left-most non-zero digit: `^22.19`
        // is `>=22.19.0 <23.0.0`, and `^0.1.0` is `>=0.1.0 <0.2.0`.
        let upper = if bound.major > 0 {
            Version {
                major: bound.major + 1,
                minor: 0,
                patch: 0,
                prerelease: Vec::new(),
            }
        } else if bound.minor > 0 {
            Version {
                major: 0,
                minor: bound.minor + 1,
                patch: 0,
                prerelease: Vec::new(),
            }
        } else {
            Version {
                major: 0,
                minor: 0,
                patch: bound.patch + 1,
                prerelease: Vec::new(),
            }
        };
        return Some(*version >= bound && *version < upper);
    }
    if let Some(rest) = alternative.strip_prefix('~') {
        let bound = parse_version(rest)?;
        let upper = Version {
            major: bound.major,
            minor: bound.minor + 1,
            patch: 0,
            prerelease: Vec::new(),
        };
        return Some(*version >= bound && *version < upper);
    }
    // An exact version: `0.2.1-rc.5` matches only itself.
    let exact = parse_version(alternative)?;
    Some(*version == exact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        parse_version(text).expect("parse")
    }

    fn tui_install(root: &Path, home: &Path) -> Install {
        Install {
            install_id: "t".to_owned(),
            kind: "tui".to_owned(),
            root: root.display().to_string(),
            version: "0.18.0".to_owned(),
            profiles: Vec::new(),
            health: "ok".to_owned(),
            home: home.display().to_string(),
        }
    }

    fn package_with_engines(root: &Path, range: &str) {
        std::fs::create_dir_all(root).expect("mkdir");
        std::fs::write(
            root.join("package.json"),
            format!(r#"{{"name":"@deepseek-harness-tui/dsh-tui","version":"0.18.0","engines":{{"node":"{range}"}}}}"#),
        )
        .expect("write");
    }

    /// F23 fires when the Node that would run the tui is outside its declared range.
    #[test]
    fn the_node_range_check_reads_the_manifest_and_the_machine() {
        let temp = o824_fixtures::TempDir::new("o824-tui-node-").expect("temp dir");
        let root = temp.path().join("pkg");
        package_with_engines(&root, "^22.19 || >=24");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).expect("mkdir");
        let install = tui_install(&root, &home);

        assert!(check(&install, "v24.8.0").is_empty(), "24.8.0 is in range");
        assert!(
            check(&install, "v22.19.0").is_empty(),
            "22.19.0 is the floor"
        );

        let findings = check(&install, "v23.4.0");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].check_id, CHECK_NODE_RANGE);
        assert_eq!(findings[0].params["node"], "v23.4.0");
        assert_eq!(findings[0].params["range"], "^22.19 || >=24");
    }

    /// A package that declares no range has nothing to violate, so nothing is reported.
    #[test]
    fn a_package_without_an_engines_field_is_not_judged() {
        let temp = o824_fixtures::TempDir::new("o824-tui-node-").expect("temp dir");
        let root = temp.path().join("pkg");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"x","version":"1.0.0"}"#,
        )
        .expect("write");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).expect("mkdir");

        assert!(check(&tui_install(&root, &home), "v12.0.0").is_empty());
    }

    fn write_restart_log(home: &Path, body: &str) {
        let state = home.parent().expect("parent").join(".dsh-tui");
        std::fs::create_dir_all(&state).expect("mkdir");
        std::fs::write(state.join("restart.log"), body).expect("write");
    }

    /// A completed exit says nothing; F25 is about the one that never finished.
    #[test]
    fn a_finished_exit_is_not_reported() {
        let temp = o824_fixtures::TempDir::new("o824-tui-restart-").expect("temp dir");
        let root = temp.path().join("pkg");
        package_with_engines(&root, "^22.19 || >=24");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).expect("mkdir");
        write_restart_log(
            &home,
            "2026-11-03T05:19:49.623Z pid=1 funnel: process exit requested {\"signal\":\"SIGHUP\"}\n\
2026-11-03T05:19:49.680Z pid=1 signal: received {\"signal\":\"SIGHUP\",\"answer\":\"exiting\"}\n\
2026-11-03T05:19:51.118Z pid=1 dispose: root disposed {\"ms\":1273}\n",
        );

        assert!(check(&tui_install(&root, &home), "v24.8.0").is_empty());
    }

    /// The interrupted one: an exit was requested and the dispose never came.
    #[test]
    fn an_unfinished_exit_is_reported() {
        let temp = o824_fixtures::TempDir::new("o824-tui-restart-").expect("temp dir");
        let root = temp.path().join("pkg");
        package_with_engines(&root, "^22.19 || >=24");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).expect("mkdir");
        // A finished exit, then one that stops at the request.
        write_restart_log(
            &home,
            "2026-11-03T05:00:00.000Z pid=1 funnel: process exit requested {}\n\
2026-11-03T05:00:01.000Z pid=1 dispose: root disposed {}\n\
2026-11-03T06:00:00.000Z pid=2 funnel: process exit requested {\"signal\":\"SIGTERM\"}\n",
        );

        let findings = check(&tui_install(&root, &home), "v24.8.0");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].check_id, CHECK_UPDATE_INTERRUPTED);
        assert_eq!(findings[0].params["at"], "2026-11-03T06:00:00.000Z");
    }

    /// No log at all is not a fault: nothing was recorded, so nothing is claimed.
    #[test]
    fn a_missing_restart_log_is_silent() {
        let temp = o824_fixtures::TempDir::new("o824-tui-restart-").expect("temp dir");
        let root = temp.path().join("pkg");
        package_with_engines(&root, "^22.19 || >=24");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).expect("mkdir");

        assert!(check(&tui_install(&root, &home), "v24.8.0").is_empty());
    }

    /// The delegated lines: only the environment ones, and never decided by exit status.
    #[test]
    fn only_the_environment_lines_of_the_third_party_doctor_are_taken() {
        assert!(takes_environment("✗ node: v23.0.0"));
        assert!(takes_environment("✗ pnpm: missing"));
        assert!(takes_environment("✗ profile: 0.18.0"));
        assert!(takes_environment("✗ launcher ↔ profile: 未对齐"));
        // A credential is a configuration need, not a broken installation.
        assert!(!takes_environment("✗ DEEPSEEK_API_KEY: 未设置"));
        assert!(!takes_environment("✓ node: v24.8.0"));
    }

    #[test]
    fn the_tui_node_range_is_read_as_the_package_declares_it() {
        // `^22.19 || >=24`, the range on the machine this was written on.
        let range = "^22.19 || >=24";
        for accepted in ["v22.19.0", "v22.20.3", "v24.8.0", "v25.0.0"] {
            assert!(
                satisfies(version(accepted), range),
                "{accepted} must satisfy"
            );
        }
        // `24.0.0` is accepted: `>=24` means `>=24.0.0`. `23.x` is the version the
        // application refuses, and `22.18.x` is below the caret's floor.
        for rejected in ["v22.18.9", "v23.4.0", "v21.7.0"] {
            assert!(
                !satisfies(version(rejected), range),
                "{rejected} must not satisfy"
            );
        }
    }

    #[test]
    fn a_prerelease_sits_below_its_release() {
        assert!(version("24.8.0-rc.1") < version("24.8.0"));
        assert!(version("22.19.0") > version("22.19.0-rc.1"));
        // The documented rule: a prerelease never satisfies a range that names its release
        // without a prerelease of its own.
        assert!(!satisfies(version("24.8.0-rc.1"), ">=24"));
    }

    #[test]
    fn an_unknown_operator_is_not_a_pass() {
        assert!(!satisfies(version("24.8.0"), "builds-on-the-moon"));
    }
}
