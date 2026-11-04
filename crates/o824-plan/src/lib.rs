//! Turns a diagnosis into the ordered, fingerprinted list of steps that `apply`
//! executes.
//!
//! [`build`] is a pure function: the same installation and the same findings
//! always produce the same steps, so a plan can be shown to a user, printed as
//! JSON, and executed later. The only I/O it performs is reading the files it
//! must fingerprint (spec §15.4) and, for a restore, listing the snapshot store
//! it must name.
//!
//! A plan never emits a step it cannot run. A repair that declines to guess — a
//! bridge the profile manifest does not name, a link whose target is gone, a
//! profile restore with no snapshot to restore from — gets no step at all: the
//! finding stays, `repair` lists it and exits 1, and `E_STATE_DRIFT` is reserved
//! for a world that changed under the plan.
//!
//! The mapping is fixed by the P0 catalogue's fixability column:
//!
//! | finding | step |
//! |---|---|
//! | F01 `E_LINK_BROKEN` | `rebuild-link`, when the target still has a `package.json` |
//! | F02 `E_BUNDLE_MISSING` | none — the target is gone by definition |
//! | F03 `E_DUP_LOADER` | none — advisory, its criterion is unproven |
//! | F04 `E_BRIDGE_DANGLING` | `rebuild-bridge`, when the profile manifest names the package |
//! | F16 `E_PROFILE_PARSE` | `restore-snapshot`, when a snapshot exists |
//! | F17 `E_PNPM_BUILD_BLOCKED` | `allow-builds`, one per profile with keys |
//! | F20 `E_HALF_INSTALL` | none — an interrupted install is fixability B |
//!
//! This crate must never depend on `o824-apply`: a plan does not know that
//! execution exists.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};
use o824_model::{Envelope, Finding, Install, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Step kinds. The strings are part of the plan document a UI may print, so they
/// are stable.
pub const KIND_REBUILD_LINK: &str = "rebuild-link";
/// Recreates an out-of-tree plugin's dependency bridge (F04).
pub const KIND_REBUILD_BRIDGE: &str = "rebuild-bridge";
/// Adds approval keys to a profile's pnpm build whitelist (F17).
pub const KIND_ALLOW_BUILDS: &str = "allow-builds";
/// Restores the installation's files from the newest snapshot (F16).
pub const KIND_RESTORE_SNAPSHOT: &str = "restore-snapshot";
/// Re-materialises a profile's dependency tree with pnpm (opt-in).
pub const KIND_REINSTALL_PROFILE: &str = "reinstall-profile";
/// Rewrites a profile's `cordis.patch.yml`, dropping duplicated ids.
///
/// No P0 finding maps to it any more: F03 is advisory and never drives an
/// automatic edit. `apply` still implements the step, so a hand-built plan may
/// carry it.
pub const KIND_DEDUPE_LOADER: &str = "dedupe-loader";
/// Copies `from` over `path`. No P0 finding maps to it; it is the generic file
/// step the rollback tests use.
pub const KIND_REPLACE_FILE: &str = "replace-file";
/// Puts back the link entries the last apply replaced, from the journal's own record.
///
/// This is what makes a rollback truthful for a repair that touched `node_modules`: the
/// snapshot does not cover that tree, but the journal does, because `apply` recorded each
/// entry before it replaced it. `from` carries the journal id.
pub const KIND_UNDO_APPLY: &str = "undo-apply";

/// How long a plan stays applicable: fifteen minutes, per §15.4.
pub const PLAN_TTL_SECONDS: u64 = 15 * 60;

/// The DSH home directory holding one directory per profile.
const PROFILES_DIR_NAME: &str = "profiles";
/// The pnpm dependency tree inside a profile.
const NODE_MODULES_DIR_NAME: &str = "node_modules";
/// Diagnosed when the installation lists no profiles, matching `verify`.
const DEFAULT_PROFILE: &str = "desktop";
/// The profile's `package.json`.
const MANIFEST_FILE_NAME: &str = "package.json";
/// The pnpm build whitelist F17 rewrites.
const WORKSPACE_FILE_NAME: &str = "pnpm-workspace.yaml";
/// Marks a pnpm file-link dependency spec.
const LINK_PROTOCOL_PREFIX: &str = "link:";

const PARAM_PATH: &str = "path";
const PARAM_TARGET: &str = "target";
const PARAM_PACKAGE: &str = "package";
const PARAM_FILE: &str = "file";
const PARAM_APPROVAL_KEY: &str = "key";

/// One mutation `apply` performs, in order.
///
/// `path` is always absolute. `from`/`to`/`args` carry the step-specific inputs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// The step kind.
    pub kind: String,
    /// The absolute target.
    pub path: String,
    /// The source of a rebuild or restore.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from: String,
    /// The application root a staged install runs from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub to: String,
    /// The step's arguments: approval keys, or the files a restore must heal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// A note apply records about the step as it runs it — today, that a
    /// pre-v11 `allowBuilds` sequence was migrated to the pnpm 11 map form. A
    /// plan as built leaves it empty; the journal carries it after the run.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl Step {
    /// Creates a step of one kind with no optional inputs.
    pub fn new(kind: &str, path: impl Into<String>) -> Step {
        Step {
            kind: kind.to_owned(),
            path: path.into(),
            ..Step::default()
        }
    }
}

/// One fingerprinted repair plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// The plan id.
    pub id: String,
    /// The installation the plan repairs.
    pub install_id: String,
    /// The absolute scope root.
    pub root: String,
    /// When the plan was built.
    pub created_at: Timestamp,
    /// When it stops being applicable.
    pub expires_at: Timestamp,
    /// The steps, in execution order; always an array.
    pub steps: Vec<Step>,
    /// `rel -> sha256` for every file a step will rewrite.
    pub fingerprint: BTreeMap<String, String>,
}

/// Selects the steps [`build_with`] may emit beyond the always-safe ones.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Additionally emit, per profile, a `reinstall-profile` step.
    ///
    /// It carries both opt-in repairs because they are the same installation: an
    /// F17 finding needs it because writing the whitelist alone removes the
    /// finding while leaving the package unbuilt, and an F01/F02 link finding
    /// needs it because the package the link stands for lives inside the host's
    /// `app.asar`, which a junction cannot point into. It is off by default on
    /// purpose: a successful install runs the package's own scripts.
    pub install_packages: bool,
}

/// Maps the findings of one diagnosis to the steps that repair them.
pub fn build(
    install: &Install,
    envelope: &Envelope,
    now: Timestamp,
    data_root: &Path,
) -> Result<Plan, Error> {
    build_with(install, envelope, now, data_root, Options::default())
}

/// [`build`] with the opt-in steps [`Options`] selects.
pub fn build_with(
    install: &Install,
    envelope: &Envelope,
    now: Timestamp,
    data_root: &Path,
    options: Options,
) -> Result<Plan, Error> {
    let root = if install.home.trim().is_empty() {
        install.root.clone()
    } else {
        install.home.clone()
    };
    if root.trim().is_empty() {
        return Err(Error::new(
            Code::Usage,
            "plan: the installation root is empty",
        ));
    }
    // A plan's steps carry absolute paths, so a relative root is resolved once,
    // here, instead of being re-interpreted by apply.
    let root = absolute(Path::new(&root))
        .map_err(|error| file_error("resolve the installation root", Path::new(&root), &error))?;

    // A reinstall-profile step runs the host application's own executable and the
    // pnpm it ships, which live in the installation root, not in the DSH home the
    // profiles live under.
    let app_root = if install.root.trim().is_empty() {
        None
    } else {
        Some(absolute(Path::new(&install.root)).map_err(|error| {
            file_error(
                "resolve the application root",
                Path::new(&install.root),
                &error,
            )
        })?)
    };

    // apply's first checkpoint is a snapshot keyed by the installation id, so a
    // plan without one could never be applied.
    if install.install_id.trim().is_empty() {
        return Err(Error::new(Code::Usage, "plan: the installation has no id"));
    }

    let mut plan = Plan {
        id: plan_id(install, &root, now),
        install_id: install.install_id.clone(),
        root: root.display().to_string(),
        created_at: now,
        expires_at: now.plus(std::time::Duration::from_secs(PLAN_TTL_SECONDS)),
        steps: Vec::new(),
        fingerprint: BTreeMap::new(),
    };

    let mut steps: Vec<Step> = Vec::new();
    let mut restore: Option<Step> = None;
    let mut allowed = false;
    // Reinstalls are kept apart so every one of them runs after every
    // allow-builds write: the staged copy a reinstall installs from must contain
    // the whitelist the write just produced.
    let mut reinstalls: Vec<Step> = Vec::new();
    let mut reinstalled: Vec<PathBuf> = Vec::new();
    let profiles = profiles_of(install);

    for finding in &envelope.findings {
        match finding.code {
            Code::LinkBroken | Code::BundleMissing => {
                if options.install_packages {
                    if let Some(profile_dir) =
                        reinstall_target(&root, &app_root, &profiles, finding)
                    {
                        if !reinstalled.contains(&profile_dir) {
                            reinstalled.push(profile_dir.clone());
                            let mut step = Step::new(
                                KIND_REINSTALL_PROFILE,
                                profile_dir.display().to_string(),
                            );
                            if let Some(app_root) = &app_root {
                                step.to = app_root.display().to_string();
                            }
                            reinstalls.push(step);
                        }
                        // No rebuild-link either way: the reinstall replaces the
                        // whole tree the link lives in.
                        continue;
                    }
                }
                let Some(step) = rebuild_link_step(&root, finding)? else {
                    // F02's target is gone by definition, and P0 refuses to
                    // guess where the bundle should come from.
                    continue;
                };
                if !within_root(&root, Path::new(&step.path)) {
                    continue;
                }
                fingerprint_file(&root, Path::new(&step.path), &mut plan.fingerprint)?;
                steps.push(step);
            }

            Code::BridgeDangling => {
                let Some(step) = rebuild_bridge_step(&root, &profiles, finding)? else {
                    // The manifest does not name where the bridge should point.
                    continue;
                };
                if !within_root(&root, Path::new(&step.path)) {
                    continue;
                }
                fingerprint_file(&root, Path::new(&step.path), &mut plan.fingerprint)?;
                steps.push(step);
            }

            Code::ProfileParse => {
                if restore.is_none() {
                    let Some(mut step) =
                        restore_step(install, &root, data_root, &mut plan.fingerprint)?
                    else {
                        continue;
                    };
                    // The first finding fixes the step; later ones only add the
                    // files the restore must heal.
                    if let Some(file) = param_string(finding, PARAM_FILE) {
                        let file = file.replace('\\', "/");
                        if !step.args.contains(&file) {
                            step.args.push(file);
                        }
                    }
                    restore = Some(step);
                    continue;
                }
                if let Some(detail) = restore.as_mut() {
                    if let Some(file) = param_string(finding, PARAM_FILE) {
                        let file = file.replace('\\', "/");
                        if !detail.args.contains(&file) {
                            detail.args.push(file);
                        }
                    }
                }
            }

            Code::PnpmBuildBlocked => {
                // The whitelist is per profile, so one pass re-derives every
                // profile's missing packages instead of trusting the finding.
                if allowed {
                    continue;
                }
                allowed = true;
                let allow = allow_builds_steps(&root, &profiles, &mut plan.fingerprint)?;
                if options.install_packages {
                    for step in &allow {
                        if let Some(parent) = Path::new(&step.path).parent() {
                            let profile_dir = parent.to_path_buf();
                            if !reinstalled.contains(&profile_dir) {
                                reinstalled.push(profile_dir.clone());
                                let mut reinstall = Step::new(
                                    KIND_REINSTALL_PROFILE,
                                    profile_dir.display().to_string(),
                                );
                                if let Some(app_root) = &app_root {
                                    reinstall.to = app_root.display().to_string();
                                }
                                reinstalls.push(reinstall);
                            }
                        }
                    }
                }
                steps.extend(allow);
            }

            // F03 is advisory, F20 is fixability B, and every other code has no
            // ruled repair.
            _ => {}
        }
    }

    if let Some(restore) = restore {
        // A restore rewrites every captured file, so it runs before the targeted
        // rewrites; otherwise it would undo them.
        steps.insert(0, restore);
    }
    steps.extend(reinstalls);
    // A plan with nothing to do still lists zero steps. Go's nil slice would
    // marshal to null, and the golden files are the cross-implementation
    // contract, so the array is always non-null here.
    plan.steps = steps;
    Ok(plan)
}

/// The profiles the installation lists, defaulting to desktop exactly like
/// `verify`, sorted and deduplicated so the plan is stable.
fn profiles_of(install: &Install) -> Vec<String> {
    let mut profiles = install.profiles.clone();
    if profiles.is_empty() {
        profiles.push(DEFAULT_PROFILE.to_owned());
    }
    profiles.sort();
    profiles.dedup();
    profiles
}

/// One allow-builds step per profile whose source dependencies are missing from
/// that profile's pnpm build whitelist.
///
/// The whitelist lives beside the profile's manifest, at
/// `profiles\<profile>\pnpm-workspace.yaml` — the file pnpm actually reads. The
/// keys are re-derived through `verify::pnpm_build_gate` rather than copied out
/// of the finding, so the repair and the diagnosis cannot disagree.
fn allow_builds_steps(
    root: &Path,
    profiles: &[String],
    fingerprint: &mut BTreeMap<String, String>,
) -> Result<Vec<Step>, Error> {
    let mut steps = Vec::new();
    for profile in profiles {
        let mut keys: Vec<String> = Vec::new();
        for finding in o824_verify::pnpm::pnpm_build_gate(root, profile) {
            if finding.code != Code::PnpmBuildBlocked {
                continue;
            }
            let key = param_string(&finding, PARAM_APPROVAL_KEY)
                .or_else(|| param_string(&finding, PARAM_PACKAGE))
                .unwrap_or_default();
            if !key.is_empty() && !keys.contains(&key) {
                keys.push(key);
            }
        }
        if keys.is_empty() {
            continue;
        }
        // Sorted so a plan is deterministic regardless of the order the findings
        // arrived in.
        keys.sort();
        let path = root
            .join(PROFILES_DIR_NAME)
            .join(profile)
            .join(WORKSPACE_FILE_NAME);
        fingerprint_file(root, &path, fingerprint)?;
        let mut step = Step::new(KIND_ALLOW_BUILDS, path.display().to_string());
        step.args = keys;
        steps.push(step);
    }
    Ok(steps)
}

/// The profile directory an F01/F02 finding belongs to, when the opt-in reinstall
/// can be aimed at it.
fn reinstall_target(
    root: &Path,
    app_root: &Option<PathBuf>,
    profiles: &[String],
    finding: &Finding,
) -> Option<PathBuf> {
    if app_root.is_none() {
        return None;
    }
    let value = param_string(finding, PARAM_PATH)?;
    let path = resolve(root, &value);
    for profile in profiles {
        let profile_dir = root.join(PROFILES_DIR_NAME).join(profile);
        if within_root(&profile_dir, &path) {
            return Some(profile_dir);
        }
    }
    None
}

/// Maps an F01/F02 finding to a rebuild-link step, or `None` when the link cannot
/// be aimed.
///
/// The step is emitted only when the finding recorded a target and that target is
/// an existing directory carrying a `package.json`. A junction to a path that
/// does not exist would leave the link exactly as broken as it was while claiming
/// success, and a target that is not a package is not a profile bundle.
fn rebuild_link_step(root: &Path, finding: &Finding) -> Result<Option<Step>, Error> {
    let Some(path) = abs_param(root, finding, PARAM_PATH) else {
        return Ok(None);
    };
    let Some(target) = param_string(finding, PARAM_TARGET) else {
        return Ok(None);
    };
    if target.trim().is_empty() {
        return Ok(None);
    }
    let resolved = resolve(root, target.trim());
    match std::fs::metadata(&resolved) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(file_error("inspect the link target", &resolved, &error)),
    }
    // A junction can only be rebuilt toward a directory that is a bundle. When
    // the target has no package.json, the link is aimed at the wrong place, and
    // rebuilding it toward that same directory reproduces the fault.
    match std::fs::metadata(resolved.join(MANIFEST_FILE_NAME)) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(file_error(
                "inspect the link target's manifest",
                &resolved,
                &error,
            ));
        }
    }
    let mut step = Step::new(KIND_REBUILD_LINK, path.display().to_string());
    step.from = resolved.display().to_string();
    Ok(Some(step))
}

/// Maps an F04 finding to a rebuild-bridge step, or `None` when the bridge cannot
/// be aimed.
///
/// The target is never guessed. The bridge belongs to an out-of-tree plugin, and
/// the only manifest that names where the package lives is the profile's: the
/// profile declares the plugin as a `link:` dependency and it declares the
/// bridged package. The bridge must therefore resolve through the profile's own
/// `node_modules` entry for that package.
fn rebuild_bridge_step(
    root: &Path,
    profiles: &[String],
    finding: &Finding,
) -> Result<Option<Step>, Error> {
    let package = param_string(finding, PARAM_PACKAGE).unwrap_or_default();
    let Some(bridge_path) = abs_param(root, finding, PARAM_PATH) else {
        return Ok(None);
    };
    if package.is_empty() {
        return Ok(None);
    }
    let Some(plugin_dir) = plugin_dir_of(&bridge_path, &package) else {
        return Ok(None);
    };

    for profile in profiles {
        let profile_dir = root.join(PROFILES_DIR_NAME).join(profile);
        let Ok(manifest) = read_manifest(&profile_dir) else {
            continue;
        };
        if !manifest.links_plugin(&profile_dir, &plugin_dir) {
            continue;
        }
        if !manifest.names_package(&package) {
            return Ok(None);
        }
        let target = profile_dir
            .join(NODE_MODULES_DIR_NAME)
            .join(package.replace('/', "\\"));
        match std::fs::metadata(target.join(MANIFEST_FILE_NAME)) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(file_error("inspect the profile's bundle", &target, &error)),
        }
        let mut step = Step::new(KIND_REBUILD_BRIDGE, bridge_path.display().to_string());
        step.from = target.display().to_string();
        return Ok(Some(step));
    }
    Ok(None)
}

/// Maps an F16 finding to a restore-snapshot step backed by the installation's
/// newest snapshot, and fingerprints every file that snapshot will overwrite.
///
/// `None` means there is no snapshot: without one the step cannot run, and a
/// finding the kernel cannot repair is reported by `repair` (exit 1) exactly like
/// an F04 the manifest cannot aim. It is not state drift, because nothing changed
/// under the plan.
fn restore_step(
    install: &Install,
    root: &Path,
    data_root: &Path,
    fingerprint: &mut BTreeMap<String, String>,
) -> Result<Option<Step>, Error> {
    if install.install_id.trim().is_empty() {
        return Err(Error::new(
            Code::StateDrift,
            "plan: the installation has no id, so its snapshots cannot be found",
        ));
    }
    let snapshots = o824_snapshot::list(data_root, &install.install_id)?;
    let Some(newest) = snapshots.first() else {
        return Ok(None);
    };
    for entry in &newest.files {
        let absolute = root.join(entry.rel.replace('/', "\\"));
        fingerprint_file(root, &absolute, fingerprint)?;
    }
    let mut step = Step::new(KIND_RESTORE_SNAPSHOT, root.display().to_string());
    step.from = newest.dir.clone();
    step.to = newest.id.clone();
    Ok(Some(step))
}

/// The subset of a profile `package.json` the plan reads.
struct ProfileManifest {
    dependencies: BTreeMap<String, String>,
}

impl ProfileManifest {
    /// True when the manifest declares `plugin_dir` as a `link:` dependency.
    fn links_plugin(&self, profile_dir: &Path, plugin_dir: &Path) -> bool {
        for spec in self.dependencies.values() {
            let Some(spec_path) = spec.trim().strip_prefix(LINK_PROTOCOL_PREFIX) else {
                continue;
            };
            let spec_path = spec_path.trim();
            if spec_path.is_empty() {
                continue;
            }
            let candidate =
                o824_model::path::clean(&profile_dir.join(spec_path.replace('/', "\\")));
            if same_path(&candidate, plugin_dir) {
                return true;
            }
        }
        false
    }

    /// True when the manifest declares `package` as a dependency.
    fn names_package(&self, package: &str) -> bool {
        self.dependencies.contains_key(package)
            || self
                .dependencies
                .keys()
                .any(|name| name.eq_ignore_ascii_case(package))
    }
}

fn read_manifest(profile_dir: &Path) -> std::io::Result<ProfileManifest> {
    let data = std::fs::read(profile_dir.join(MANIFEST_FILE_NAME))?;
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        dependencies: BTreeMap<String, String>,
    }
    let raw: Raw = serde_json::from_slice(&data).map_err(std::io::Error::other)?;
    Ok(ProfileManifest {
        dependencies: raw.dependencies,
    })
}

/// Strips the `node_modules\<pkg>` suffix from a bridge path and returns the
/// plugin directory it belongs to.
///
/// The comparison is textual and case-insensitive, exactly like the reference
/// implementation: the bridge path may be absolute while the suffix is relative,
/// so a path-component strip would not match.
fn plugin_dir_of(bridge_path: &Path, package: &str) -> Option<PathBuf> {
    let bridge = bridge_path.display().to_string();
    let marker = format!(
        "{sep}{NODE_MODULES_DIR_NAME}{sep}{}",
        package.replace('/', std::path::MAIN_SEPARATOR_STR),
        sep = std::path::MAIN_SEPARATOR
    );
    if bridge.len() <= marker.len() {
        return None;
    }
    if !bridge.to_lowercase().ends_with(&marker.to_lowercase()) {
        return None;
    }
    let cut = bridge.len() - marker.len();
    if !bridge.is_char_boundary(cut) {
        return None;
    }
    Some(PathBuf::from(&bridge[..cut]))
}

/// Resolves one root-relative finding parameter to an absolute path.
///
/// `verify` records a root-relative path when it can and the absolute one when the
/// target lives on another volume, so both spellings are accepted.
fn abs_param(root: &Path, finding: &Finding, key: &str) -> Option<PathBuf> {
    let value = param_string(finding, key)?;
    if value.trim().is_empty() {
        return None;
    }
    Some(resolve(root, value.trim()))
}

/// True when `path` is `root` itself or lives inside it.
///
/// A plan never emits a step apply would refuse, so a finding whose path is
/// outside the installation stays a finding.
fn within_root(root: &Path, path: &Path) -> bool {
    let root = o824_model::path::clean(root);
    let path = o824_model::path::clean(path);
    if path == root {
        return true;
    }
    match path.strip_prefix(&root) {
        Ok(rest) => !rest.as_os_str().is_empty() && !rest.starts_with(".."),
        Err(_) => false,
    }
}

/// Turns a finding path into an absolute path: an absolute spelling is kept, a
/// root-relative one is joined to `root`.
fn resolve(root: &Path, value: &str) -> PathBuf {
    let candidate = Path::new(value);
    if candidate.is_absolute() {
        return o824_model::path::clean(candidate);
    }
    o824_model::path::clean(&root.join(value.replace('/', "\\")))
}

/// Records `rel(path) -> sha256(path)` for every file a step will rewrite.
///
/// A path that does not exist is skipped (there is nothing to bind), and a
/// directory is skipped too: only a regular file carries a fingerprint.
fn fingerprint_file(
    root: &Path,
    path: &Path,
    fingerprint: &mut BTreeMap<String, String>,
) -> Result<(), Error> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(file_error("fingerprint", path, &error)),
    };
    if !metadata.is_file() {
        return Ok(());
    }
    let data = std::fs::read(path).map_err(|error| file_error("fingerprint", path, &error))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    let relative = relative_slash_path(root, path);
    if let Some(relative) = relative {
        fingerprint.insert(relative, hex);
    }
    Ok(())
}

/// Renders `path` relative to `root` with forward slashes, or `None` when it is
/// not inside `root`.
fn relative_slash_path(root: &Path, path: &Path) -> Option<String> {
    let root = o824_model::path::clean(root);
    let path = o824_model::path::clean(path);
    let rest = path.strip_prefix(&root).ok()?;
    let text = rest.display().to_string().replace('\\', "/");
    if text.is_empty() || text == "." {
        return None;
    }
    Some(text)
}

/// Builds a stable, path-safe identifier: the creation time plus a digest of the
/// installation, so two plans built for the same installation at the same instant
/// are the same plan, and a journal file name can never escape its directory.
fn plan_id(install: &Install, root: &Path, now: Timestamp) -> String {
    plan_id_for(install, root, now, "repair")
}

/// The plan id with the action in the seed, so a start and a repair built in the same
/// second for the same installation are different plans.
pub(crate) fn plan_id_for(install: &Install, root: &Path, now: Timestamp, action: &str) -> String {
    // A repair keeps the exact seed the reference implementation uses, so a plan id for
    // the same installation and second is the same on both sides. An action plan mixes
    // the action in, so a start and a repair in that second are different plans.
    let seed = if action == "repair" {
        format!(
            "{}\u{0}{}\u{0}{}",
            install.install_id,
            root.display(),
            now.to_rfc3339()
        )
    } else {
        format!(
            "{}\u{0}{}\u{0}{}\u{0}{}",
            install.install_id,
            root.display(),
            now.to_rfc3339(),
            action
        )
    };
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(16);
    for byte in &digest[..4] {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!(
        "plan-{:04}{:02}{:02}T{:02}{:02}{:02}Z-{}",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        hex
    )
}

fn param_string(finding: &Finding, key: &str) -> Option<String> {
    finding
        .params
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn absolute(path: &Path) -> std::io::Result<PathBuf> {
    Ok(o824_model::path::clean(&std::path::absolute(path)?))
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = left.display().to_string();
    let right = right.display().to_string();
    left.eq_ignore_ascii_case(&right)
}

/// Wraps a filesystem failure as a kernel error, keeping permission denials
/// distinct.
fn file_error(operation: &str, path: &Path, cause: &std::io::Error) -> Error {
    let code = if cause.kind() == std::io::ErrorKind::PermissionDenied {
        Code::Permission
    } else {
        Code::Internal
    };
    Error::new(
        code,
        format!(
            "plan: cannot {operation} {:?}: {cause}",
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

    fn install_of(root: &Path) -> Install {
        Install {
            install_id: "local-test".to_owned(),
            kind: "desktop".to_owned(),
            root: root.display().to_string(),
            version: o824_fixtures::DSH_VERSION.to_owned(),
            profiles: vec!["desktop".to_owned()],
            health: "ok".to_owned(),
            home: root.display().to_string(),
        }
    }

    fn plan_for(kind: &str) -> (Plan, TempDir, o824_fixtures::Fixture) {
        let fixture = o824_fixtures::build(kind).expect("fixture");
        let store = TempDir::new("o824-plan-").expect("store");
        let install = install_of(fixture.root());
        let envelope = o824_verify::doctor(&install, store.path());
        let now = Timestamp::parse("2026-11-01T12:10:41.5123456Z").expect("timestamp");
        let plan = build(&install, &envelope, now, store.path()).expect("build");
        (plan, store, fixture)
    }

    #[test]
    fn a_clean_tree_has_no_steps() {
        let (plan, _store, _fixture) = plan_for("ok");
        assert!(plan.steps.is_empty());
        assert!(plan.fingerprint.is_empty());
        assert_eq!(plan.created_at.to_rfc3339(), "2026-11-01T12:10:41.5123456Z");
        assert_eq!(plan.expires_at.to_rfc3339(), "2026-11-01T12:25:41.5123456Z");
        assert!(plan.id.starts_with("plan-20261101T121041Z-"), "{}", plan.id);
        assert_eq!(plan.id.len(), "plan-20261101T121041Z-".len() + 8);
    }

    /// F01's target exists but has no `package.json`, so rebuilding the link
    /// would reproduce the fault. The finding stays and no step is emitted.
    #[test]
    fn a_link_to_a_non_package_gets_no_step() {
        let (plan, _store, _fixture) = plan_for("broken-link");
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    }

    /// F02's target is gone by definition.
    #[test]
    fn a_missing_bundle_gets_no_step() {
        let (plan, _store, _fixture) = plan_for("missing-bundle");
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    }

    /// F03 is advisory and never drives an automatic edit.
    #[test]
    fn a_duplicate_loader_entry_gets_no_step() {
        let (plan, _store, _fixture) = plan_for("dup-loader");
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    }

    /// F16 has no snapshot to restore from, so it stays a finding.
    #[test]
    fn a_parse_error_without_a_snapshot_gets_no_step() {
        let (plan, _store, _fixture) = plan_for("parse-error");
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    }

    /// F20 is fixability B.
    #[test]
    fn an_interrupted_install_gets_no_step() {
        let (plan, _store, _fixture) = plan_for("half-install");
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    }

    #[test]
    fn a_dangling_bridge_gets_one_rebuild_bridge_step() {
        let (plan, _store, _fixture) = plan_for("dangling-bridge");
        assert_eq!(plan.steps.len(), 1, "{:?}", plan.steps);
        let step = &plan.steps[0];
        assert_eq!(step.kind, KIND_REBUILD_BRIDGE);
        let sep = std::path::MAIN_SEPARATOR;
        let root = plan.root.trim_end_matches(sep);
        assert_eq!(
            step.path,
            format!(
                "{root}{sep}vendor{sep}plugins{sep}dsh-demo-plugin{sep}node_modules{sep}@deepseek-ai{sep}dsh-base"
            )
        );
        assert_eq!(
            step.from,
            format!(
                "{root}{sep}profiles{sep}desktop{sep}node_modules{sep}@deepseek-ai{sep}dsh-base"
            )
        );
        // The bridge does not exist yet, so there is nothing to fingerprint.
        assert!(plan.fingerprint.is_empty());
    }

    #[test]
    fn a_blocked_build_gets_one_allow_builds_step_and_a_fingerprint() {
        let (plan, _store, _fixture) = plan_for("git-dep");
        assert_eq!(plan.steps.len(), 1, "{:?}", plan.steps);
        let step = &plan.steps[0];
        assert_eq!(step.kind, KIND_ALLOW_BUILDS);
        let sep = std::path::MAIN_SEPARATOR;
        assert!(
            step.path
                .ends_with(&format!("profiles{sep}desktop{sep}pnpm-workspace.yaml")),
            "{}",
            step.path
        );
        assert_eq!(
            step.args,
            ["dshmarket@git+https://github.com/deepseek-ai/dshmarket.git"]
        );
        assert_eq!(
            plan.fingerprint.get("profiles/desktop/pnpm-workspace.yaml"),
            Some(&"e7987c113d261412264ca571fc344b95b17c0c09ab399a62fc7e47b05362a2af".to_owned()),
            "the fingerprint is the sha256 of the file the step rewrites"
        );
    }

    #[test]
    fn the_same_tree_at_the_same_instant_builds_the_same_plan() {
        let fixture = o824_fixtures::build("git-dep").expect("fixture");
        let store = TempDir::new("o824-plan-").expect("store");
        let install = install_of(fixture.root());
        let envelope = o824_verify::doctor(&install, store.path());
        let now = Timestamp::parse("2026-11-01T12:10:41Z").expect("timestamp");
        let first = build(&install, &envelope, now, store.path()).expect("first");
        let second = build(&install, &envelope, now, store.path()).expect("second");
        assert_eq!(first, second);
    }

    #[test]
    fn a_different_instant_produces_a_different_id() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = TempDir::new("o824-plan-").expect("store");
        let install = install_of(fixture.root());
        let envelope = o824_verify::doctor(&install, store.path());
        let first = build(
            &install,
            &envelope,
            Timestamp::parse("2026-11-01T12:10:41Z").expect("timestamp"),
            store.path(),
        )
        .expect("first");
        let second = build(
            &install,
            &envelope,
            Timestamp::parse("2026-11-01T12:10:42Z").expect("timestamp"),
            store.path(),
        )
        .expect("second");
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn an_installation_without_an_id_is_refused() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = TempDir::new("o824-plan-").expect("store");
        let mut install = install_of(fixture.root());
        install.install_id = String::new();
        let envelope = o824_verify::doctor(&install, store.path());
        let error =
            build(&install, &envelope, Timestamp::now(), store.path()).expect_err("must fail");
        assert_eq!(error.code, Code::Usage);
        assert!(error.fallback.contains("no id"), "{}", error.fallback);
    }

    #[test]
    fn a_step_outside_the_root_is_never_planned() {
        let root = Path::new(r"C:\install");
        assert!(within_root(root, Path::new(r"C:\install\profiles\desktop")));
        assert!(within_root(root, root));
        assert!(!within_root(root, Path::new(r"C:\installer\profiles")));
        assert!(!within_root(root, Path::new(r"C:\elsewhere")));
    }
}

pub mod process;

pub use process::{
    StartOptions, ACTION_START, ACTION_STOP, ARG_HOME, ARG_INSTALL_ID, ARG_PORT,
    KIND_START_DESKTOP, KIND_START_WEB, KIND_STOP_DESKTOP, KIND_STOP_WEB,
};
