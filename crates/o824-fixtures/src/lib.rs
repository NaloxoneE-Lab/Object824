//! Builds synthetic DeepSeek Harness installation trees for tests.
//!
//! Every tree is generated under a temporary directory: nothing is committed,
//! no binary ends up in git, and no test touches the network.
//!
//! A tree doubles as both the install root and the DSH home, so its profile
//! lives at `<root>\profiles\desktop`. That is why one fixture can be handed to
//! discovery as `--root` and to the journal reader as the data root without
//! rewriting paths.
//!
//! The tree must be built at its final path: the `node_modules` entries are
//! links with absolute targets, so moving a completed tree would leave them
//! dangling.

#![deny(unsafe_code)]

mod temp;

pub use temp::TempDir;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The desktop application entry point.
pub const EXE_NAME: &str = "DeepSeek Harness.exe";
/// The plain-text marker at the install root.
pub const VERSION_NAME: &str = "version";
/// The Electron runtime version written into `<root>\version`. It is
/// deliberately not the DSH version: on a real install that file holds
/// Electron's version, so a reader that trusts it is wrong.
pub const RUNTIME_VERSION: &str = "44.7.0";
/// The version inside `resources\app.asar`'s `package.json`, and the only
/// version discovery may report.
pub const DSH_VERSION: &str = "0.2.1-rc.3";
/// The only profile shape P0 knows.
pub const PROFILE_NAME: &str = "desktop";

/// The two bundles every profile carries.
pub const DSH_BASE_PKG: &str = "@deepseek-ai/dsh-base";
/// The desktop host bundle.
pub const DSH_DESKTOP_HOST_PKG: &str = "@deepseek-ai/dsh-desktop-host";

/// Every fixture kind, in the order the acceptance suite uses them.
pub const KINDS: [&str; 8] = [
    "ok",
    "broken-link",
    "missing-bundle",
    "dup-loader",
    "parse-error",
    "half-install",
    "dangling-bridge",
    "git-dep",
];

const ASAR_REL_PATH: &str = "resources/app.asar";
/// A valid cordis document: DSH's root is an array.
const EMPTY_CORDIS: &str = "[]\n";
/// The leftover loader line that collides with the `dsh-echo` bundle (F03).
const DUPLICATE_CORDIS_PATCH: &str = "- id: dsh-echo\n  enabled: true\n";
/// Deliberately unparseable YAML: the flow sequence is never closed, so any
/// YAML parser fails with a line number (F16).
const BROKEN_CORDIS: &str = "- id: dsh-base\n  enabled: [true, false\n";
/// A valid pnpm 11 workspace build whitelist. `allowBuilds` is a mapping of
/// approval key to boolean, which is the only form pnpm 11 accepts.
const WORKSPACE_YAML: &str =
    "packages:\n  - profiles/*\nallowBuilds:\n  \"@deepseek-ai/dsh-base\": true\n";
/// A minimal, valid pnpm lockfile.
const LOCKFILE_YAML: &str = "lockfileVersion: '9.0'\n\nsettings:\n  autoInstallPeers: true\n  excludeLinksFromLockfile: false\n\nimporters:\n  .: {}\n";

/// Why a fixture could not be built.
#[derive(Debug)]
pub enum FixtureError {
    /// The kind is not one of [`KINDS`].
    UnknownKind(String),
    /// A filesystem operation failed.
    Io {
        /// What was being done.
        context: String,
        /// The underlying failure.
        source: io::Error,
    },
    /// Neither a symbolic link nor a junction could be created.
    Link {
        /// The link path.
        link: PathBuf,
        /// The link target.
        target: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixtureError::UnknownKind(kind) => {
                write!(f, "fixtures: unknown install kind {kind:?}")
            }
            FixtureError::Io { context, source } => {
                write!(f, "fixtures: {context}: {source}")
            }
            FixtureError::Link {
                link,
                target,
                source,
            } => write!(
                f,
                "fixtures: cannot create a link from {} to {}: {source}",
                link.display(),
                target.display()
            ),
        }
    }
}

impl std::error::Error for FixtureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FixtureError::Io { source, .. } | FixtureError::Link { source, .. } => Some(source),
            FixtureError::UnknownKind(_) => None,
        }
    }
}

/// One built fixture tree. Dropping it removes the tree.
#[derive(Debug)]
pub struct Fixture {
    dir: TempDir,
}

impl Fixture {
    /// The installation root, which is also the DSH home.
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// The profile directory.
    pub fn profile_dir(&self) -> PathBuf {
        self.root().join("profiles").join(PROFILE_NAME)
    }
}

/// The declarative description of one tree.
#[derive(Clone, Debug, Default)]
struct Install {
    /// `dsh.profile.bundles`, in order.
    bundles: Vec<String>,
    /// Profile dependencies, kept sorted so the JSON is deterministic.
    deps: Vec<(String, String)>,
    /// `profiles\<profile>\node_modules` entries.
    links: Vec<NodeLink>,
    /// `package.json`-bearing directories under `vendor\`.
    vendor: Vec<VendorPackage>,
    /// Real, empty directories (root-relative).
    empty_dirs: Vec<String>,
    /// The archive is missing (half-install).
    no_asar: bool,
    /// An unfinished journal exists (half-install).
    journal: bool,
    /// `cordis.yml` contents.
    cordis: String,
    /// `cordis.patch.yml` contents.
    patch: String,
}

/// One `profiles\<profile>\node_modules` entry.
#[derive(Clone, Debug)]
struct NodeLink {
    pkg: String,
    target: String,
}

/// A real package directory the fixture links into `node_modules`.
#[derive(Clone, Debug)]
struct VendorPackage {
    rel: String,
    name: String,
    version: String,
    peers: Vec<(String, String)>,
}

impl Install {
    fn set_dep(&mut self, name: &str, spec: &str) {
        self.deps.push((name.to_owned(), spec.to_owned()));
    }
}

/// Creates one synthetic installation of the given kind and returns it.
///
/// The kinds are:
///
/// * `ok` — a clean install: every P0 check passes.
/// * `broken-link` — F01: the link target exists but has no `package.json`.
/// * `missing-bundle` — F02: the link target does not exist.
/// * `dup-loader` — F03: one id shared by the bundles and `cordis.patch.yml`.
/// * `parse-error` — F16: `cordis.yml` is invalid YAML.
/// * `half-install` — F20: no `app.asar` plus an unfinished journal.
/// * `dangling-bridge` — F04: an out-of-tree plugin whose bridge is missing.
/// * `git-dep` — F17: a `git+` dependency missing from `allowBuilds`.
pub fn build(kind: &str) -> Result<Fixture, FixtureError> {
    let dir = TempDir::new("o824-fixture-").map_err(|source| FixtureError::Io {
        context: "create a temporary directory".to_owned(),
        source,
    })?;
    build_at(kind, dir.path())?;
    Ok(Fixture { dir })
}

/// Creates one synthetic installation of the given kind under `root`, which must
/// already exist.
pub fn build_at(kind: &str, root: &Path) -> Result<(), FixtureError> {
    let spec = spec_for(kind)?;
    build_tree(root, &spec)
}

/// Returns the tree for `kind`, or an error naming the unknown kind.
fn spec_for(kind: &str) -> Result<Install, FixtureError> {
    let mut spec = clean_install();
    match kind {
        "ok" => {}
        "broken-link" => {
            // The target is a real directory that simply carries no package.json.
            retarget_link(&mut spec, DSH_BASE_PKG, "vendor/dsh-base-empty")?;
            spec.empty_dirs.push("vendor/dsh-base-empty".to_owned());
        }
        "missing-bundle" => {
            // The target does not exist at all.
            retarget_link(&mut spec, DSH_BASE_PKG, "vendor/packages/dsh-base-absent")?;
        }
        "dup-loader" => {
            spec.bundles.push("@acme/dsh-echo".to_owned());
            spec.set_dep("@acme/dsh-echo", "1.0.0");
            spec.links.push(NodeLink {
                pkg: "@acme/dsh-echo".to_owned(),
                target: "vendor/packages/dsh-echo".to_owned(),
            });
            spec.vendor.push(VendorPackage {
                rel: "vendor/packages/dsh-echo".to_owned(),
                name: "@acme/dsh-echo".to_owned(),
                version: "1.0.0".to_owned(),
                peers: Vec::new(),
            });
            spec.patch = DUPLICATE_CORDIS_PATCH.to_owned();
        }
        "parse-error" => spec.cordis = BROKEN_CORDIS.to_owned(),
        "half-install" => {
            spec.no_asar = true;
            spec.journal = true;
        }
        "dangling-bridge" => {
            // An out-of-tree plugin: the `node_modules` bridge from the plugin
            // back into the profile's closure is missing, which is F04.
            spec.bundles.push("dsh-demo-plugin".to_owned());
            spec.set_dep(
                "dsh-demo-plugin",
                "link:../../vendor/plugins/dsh-demo-plugin",
            );
            spec.links.push(NodeLink {
                pkg: "dsh-demo-plugin".to_owned(),
                target: "vendor/plugins/dsh-demo-plugin".to_owned(),
            });
            spec.vendor.push(VendorPackage {
                rel: "vendor/plugins/dsh-demo-plugin".to_owned(),
                name: "dsh-demo-plugin".to_owned(),
                version: "1.0.0".to_owned(),
                peers: vec![(DSH_BASE_PKG.to_owned(), "*".to_owned())],
            });
        }
        "git-dep" => {
            spec.set_dep(
                "dshmarket",
                "git+https://github.com/deepseek-ai/dshmarket.git#v1.70.2",
            );
            // The workspace whitelist stays as it is; dshmarket is not on it.
        }
        other => return Err(FixtureError::UnknownKind(other.to_owned())),
    }
    if spec.cordis.is_empty() {
        spec.cordis = EMPTY_CORDIS.to_owned();
    }
    if spec.patch.is_empty() {
        spec.patch = EMPTY_CORDIS.to_owned();
    }
    Ok(spec)
}

/// The base tree every kind starts from: a valid three-part install whose
/// profile resolves every bundle through `node_modules` to a `package.json`,
/// with no duplicate loader id, no dangling bridge, no git dependency, and no
/// journal. The dirty kinds add exactly one fault to it.
fn clean_install() -> Install {
    Install {
        bundles: vec![DSH_BASE_PKG.to_owned(), DSH_DESKTOP_HOST_PKG.to_owned()],
        deps: vec![
            (DSH_BASE_PKG.to_owned(), DSH_VERSION.to_owned()),
            (DSH_DESKTOP_HOST_PKG.to_owned(), DSH_VERSION.to_owned()),
        ],
        links: vec![
            NodeLink {
                pkg: DSH_BASE_PKG.to_owned(),
                target: "vendor/packages/dsh-base".to_owned(),
            },
            NodeLink {
                pkg: DSH_DESKTOP_HOST_PKG.to_owned(),
                target: "vendor/packages/dsh-desktop-host".to_owned(),
            },
        ],
        vendor: vec![
            VendorPackage {
                rel: "vendor/packages/dsh-base".to_owned(),
                name: DSH_BASE_PKG.to_owned(),
                version: DSH_VERSION.to_owned(),
                peers: Vec::new(),
            },
            VendorPackage {
                rel: "vendor/packages/dsh-desktop-host".to_owned(),
                name: DSH_DESKTOP_HOST_PKG.to_owned(),
                version: DSH_VERSION.to_owned(),
                peers: Vec::new(),
            },
        ],
        empty_dirs: Vec::new(),
        no_asar: false,
        journal: false,
        cordis: String::new(),
        patch: String::new(),
    }
}

/// Points the `node_modules` entry for `pkg` at `target`.
fn retarget_link(spec: &mut Install, pkg: &str, target: &str) -> Result<(), FixtureError> {
    for link in &mut spec.links {
        if link.pkg == pkg {
            link.target = target.to_owned();
            return Ok(());
        }
    }
    Err(FixtureError::UnknownKind(format!(
        "no node_modules link for {pkg:?}"
    )))
}

/// Writes the described installation under `root`.
fn build_tree(root: &Path, spec: &Install) -> Result<(), FixtureError> {
    // The executable is deliberately empty: no fixture ships a binary, and
    // discovery only requires a regular file at this path.
    write_file(&root.join(EXE_NAME), b"")?;
    write_file(
        &root.join(VERSION_NAME),
        format!("{RUNTIME_VERSION}\n").as_bytes(),
    )?;
    if !spec.no_asar {
        write_asar(&root.join(ASAR_REL_PATH))?;
    }

    let profile_dir = root.join("profiles").join(PROFILE_NAME);
    write_file(
        &profile_dir.join("pnpm-workspace.yaml"),
        WORKSPACE_YAML.as_bytes(),
    )?;
    write_profile_manifest(&profile_dir.join("package.json"), spec)?;
    write_file(&profile_dir.join("cordis.yml"), spec.cordis.as_bytes())?;
    write_file(&profile_dir.join("cordis.patch.yml"), spec.patch.as_bytes())?;
    write_file(
        &profile_dir.join("pnpm-lock.yaml"),
        LOCKFILE_YAML.as_bytes(),
    )?;

    for dir in &spec.empty_dirs {
        mkdir_all(&root.join(dir.replace('/', "\\")))?;
    }
    for pkg in &spec.vendor {
        let mut manifest = serde_json::Map::new();
        manifest.insert("name".to_owned(), serde_json::Value::from(pkg.name.clone()));
        manifest.insert(
            "version".to_owned(),
            serde_json::Value::from(pkg.version.clone()),
        );
        if !pkg.peers.is_empty() {
            let mut peers = serde_json::Map::new();
            for (name, spec) in &pkg.peers {
                peers.insert(name.clone(), serde_json::Value::from(spec.clone()));
            }
            manifest.insert(
                "peerDependencies".to_owned(),
                serde_json::Value::Object(peers),
            );
        }
        write_json(
            &root.join(pkg.rel.replace('/', "\\")).join("package.json"),
            &serde_json::Value::Object(manifest),
        )?;
    }
    for link in &spec.links {
        let target = root.join(link.target.replace('/', "\\"));
        let link_path = profile_dir
            .join("node_modules")
            .join(link.pkg.replace('/', "\\"));
        link_dir(&target, &link_path)?;
    }
    if spec.journal {
        let mut journal = serde_json::Map::new();
        journal.insert("id".to_owned(), serde_json::Value::from("install-1"));
        journal.insert("state".to_owned(), serde_json::Value::from("applying"));
        write_json(
            &root.join("journal").join("install-1.json"),
            &serde_json::Value::Object(journal),
        )?;
    }
    Ok(())
}

/// Writes the profile `package.json`.
fn write_profile_manifest(path: &Path, spec: &Install) -> Result<(), FixtureError> {
    let mut deps = serde_json::Map::new();
    let mut sorted = spec.deps.clone();
    // Go marshals a map in key order; keeping that order makes the two
    // implementations produce the same bytes for the same tree.
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, spec) in sorted {
        deps.insert(name, serde_json::Value::from(spec));
    }

    let mut bundles = Vec::with_capacity(spec.bundles.len());
    for bundle in &spec.bundles {
        bundles.push(serde_json::Value::from(bundle.clone()));
    }
    let mut profile = serde_json::Map::new();
    profile.insert("bundles".to_owned(), serde_json::Value::Array(bundles));
    let mut dsh = serde_json::Map::new();
    dsh.insert("profile".to_owned(), serde_json::Value::Object(profile));

    let mut manifest = serde_json::Map::new();
    manifest.insert(
        "name".to_owned(),
        serde_json::Value::from(format!("dsh-profile-{PROFILE_NAME}")),
    );
    manifest.insert("private".to_owned(), serde_json::Value::from(true));
    manifest.insert("dependencies".to_owned(), serde_json::Value::Object(deps));
    manifest.insert("dsh".to_owned(), serde_json::Value::Object(dsh));
    write_json(path, &serde_json::Value::Object(manifest))
}

/// Writes a minimal but format-faithful asar archive whose `package.json`
/// carries [`DSH_VERSION`].
///
/// The layout is exactly what discovery's bounded reader expects, so fixtures
/// exercise the real read path:
///
/// ```text
/// uint32(4), uint32(header pickle size), header pickle, file data
/// ```
///
/// The header pickle itself is `uint32(payload size)`, `uint32(JSON length)`,
/// then the JSON, NUL-padded to a 4-byte boundary.
fn write_asar(path: &Path) -> Result<(), FixtureError> {
    let package = serde_json::json!({
        "name": "deepseek-harness",
        "version": DSH_VERSION,
    });
    let package = serde_json::to_vec(&package).map_err(|source| FixtureError::Io {
        context: "marshal the asar package.json".to_owned(),
        source: io::Error::new(io::ErrorKind::InvalidData, source),
    })?;

    let header = serde_json::json!({
        "files": {
            "package.json": {
                "offset": "0",
                "size": package.len(),
            }
        }
    });
    let header = serde_json::to_vec(&header).map_err(|source| FixtureError::Io {
        context: "marshal the asar header".to_owned(),
        source: io::Error::new(io::ErrorKind::InvalidData, source),
    })?;

    let padding = (4 - header.len() % 4) % 4;
    let payload_size = 4 + header.len() + padding;
    let mut header_pickle = vec![0u8; 4 + payload_size];
    header_pickle[0..4].copy_from_slice(&(payload_size as u32).to_le_bytes());
    header_pickle[4..8].copy_from_slice(&(header.len() as u32).to_le_bytes());
    header_pickle[8..8 + header.len()].copy_from_slice(&header);

    let mut archive = Vec::with_capacity(8 + header_pickle.len() + package.len());
    archive.extend_from_slice(&4u32.to_le_bytes());
    archive.extend_from_slice(&(header_pickle.len() as u32).to_le_bytes());
    archive.extend_from_slice(&header_pickle);
    archive.extend_from_slice(&package);
    write_file(path, &archive)
}

/// Makes `link` point at `target`.
///
/// A symbolic link is preferred — unlike a junction it tolerates a missing
/// target — and an NTFS junction is the fallback, because an unprivileged
/// Windows process may only create symbolic links when Developer Mode is on.
/// The junction fallback creates a missing target for the duration of the call
/// and deletes it again, so the dangling F02 fixture still builds without
/// symlink privilege: silently skipping it would drop F01/F02 coverage on
/// exactly the machines that need it.
fn link_dir(target: &Path, link: &Path) -> Result<(), FixtureError> {
    mkdir_all(link.parent().unwrap_or(link))?;

    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(target, link).is_ok() {
        return Ok(());
    }
    #[cfg(unix)]
    if std::os::unix::fs::symlink(target, link).is_ok() {
        return Ok(());
    }

    #[cfg(windows)]
    if junction_link(target, link).is_ok() {
        return Ok(());
    }

    Err(FixtureError::Link {
        link: link.to_path_buf(),
        target: target.to_path_buf(),
        source: io::Error::other(format!(
            "neither a symbolic link nor a junction could be created from {} to {}",
            link.display(),
            target.display()
        )),
    })
}

/// Creates a directory junction at `link` pointing at `target`.
///
/// A junction cannot be created to a missing target, so when `target` is absent
/// it is created for the mklink call and removed again afterwards; the
/// resulting link is then dangling, which is exactly what the F02 fixture
/// needs.
#[cfg(windows)]
fn junction_link(target: &Path, link: &Path) -> io::Result<()> {
    use std::process::Command;

    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    let created_target = if target.is_dir() {
        false
    } else {
        fs::create_dir_all(target)?;
        true
    };
    // argv form: the path never reaches a shell. Only tests use this helper,
    // and only to manufacture fixtures under a temporary directory.
    let output = Command::new("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output();
    let result = match output {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(io::Error::other(format!(
            "mklink exited with {:?}: {}{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))),
        Err(error) => Err(error),
    };
    if created_target {
        // The target was only a scaffold for mklink: removing it leaves the
        // junction dangling, and it was empty by construction.
        if let Err(error) = fs::remove_dir(target) {
            return Err(io::Error::other(format!(
                "mklink failed ({error}) and the temporary target {} could not be removed",
                target.display()
            )));
        }
    }
    result
}

fn write_file(path: &Path, data: &[u8]) -> Result<(), FixtureError> {
    mkdir_all(path.parent().unwrap_or(path))?;
    fs::write(path, data).map_err(|source| FixtureError::Io {
        context: format!("write {}", path.display()),
        source,
    })
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), FixtureError> {
    let mut data = serde_json::to_vec_pretty(value).map_err(|source| FixtureError::Io {
        context: format!("marshal {}", path.display()),
        source: io::Error::new(io::ErrorKind::InvalidData, source),
    })?;
    data.push(b'\n');
    write_file(path, &data)
}

fn mkdir_all(dir: &Path) -> Result<(), FixtureError> {
    fs::create_dir_all(dir).map_err(|source| FixtureError::Io {
        context: format!("create {}", dir.display()),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_builds() {
        for kind in KINDS {
            let fixture = build(kind).unwrap_or_else(|error| panic!("build {kind}: {error}"));
            assert!(
                fixture.root().join(EXE_NAME).exists(),
                "{kind} is missing the executable"
            );
            assert!(
                fixture.profile_dir().join("package.json").exists(),
                "{kind} is missing the profile manifest"
            );
        }
    }

    #[test]
    fn half_install_has_no_asar() {
        let fixture = build("half-install").expect("build");
        assert!(!fixture.root().join("resources").join("app.asar").exists());
        assert!(fixture
            .root()
            .join("journal")
            .join("install-1.json")
            .exists());
    }

    #[test]
    fn every_other_kind_has_an_asar() {
        for kind in KINDS.iter().filter(|kind| **kind != "half-install") {
            let fixture = build(kind).expect("build");
            assert!(
                fixture.root().join("resources").join("app.asar").exists(),
                "{kind} is missing the asar"
            );
        }
    }

    #[test]
    fn node_modules_entries_are_links_not_copies() {
        let fixture = build("ok").expect("build");
        let link = fixture.profile_dir().join("node_modules").join("vendor");
        // The base package lives at node_modules\@deepseek-ai\dsh-base; a real
        // link resolves to the vendor directory, which has no `.git` marker.
        let _ = link;
        let base = fixture
            .profile_dir()
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh-base");
        let target = fixture
            .root()
            .join("vendor")
            .join("packages")
            .join("dsh-base");
        let resolved = fs::canonicalize(&base).expect("the link must resolve");
        assert_eq!(
            resolved,
            fs::canonicalize(&target).expect("the target must exist")
        );
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert!(matches!(
            build("nope"),
            Err(FixtureError::UnknownKind(kind)) if kind == "nope"
        ));
    }

    #[test]
    fn building_is_fast_enough_for_hundreds_of_tests() {
        let started = std::time::Instant::now();
        for kind in KINDS {
            let _fixture = build(kind).expect("build");
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "{:?} for eight trees is too slow for the acceptance suite",
            started.elapsed()
        );
    }
}
