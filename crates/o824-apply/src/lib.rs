//! Executes a repair plan against one installation.
//!
//! The order is not negotiable, because each step is what makes the next one
//! safe:
//!
//! 1. take the machine lock — every writer is serialised;
//! 2. refuse an expired plan — it is bound to the state it was built against;
//! 3. re-verify the fingerprint — a file that changed since the plan was built
//!    means the plan no longer describes the tree;
//! 4. take a verified snapshot — the first rollback checkpoint;
//! 5. write the journal — an interrupted apply is then diagnosable;
//! 6. run the steps in order;
//! 7. on any failure, restore the snapshot, delete what the repair created, put
//!    replaced links back, and record all of it in the journal;
//! 8. on success, record `done`.
//!
//! A nil return means every step ran and the journal records `done`. A failure
//! before the first mutation leaves the installation untouched; a failure during
//! the steps restores the checkpoint and returns the original error. A rollback
//! that itself fails is recorded in the journal but never replaces the error that
//! caused it: the caller must not be told "the rollback failed" when the repair
//! failed.
//!
//! This crate must never depend on `o824-cli`: the kernel does not know that
//! human-facing output exists.

#![deny(unsafe_code)]

pub mod allow_builds;
pub mod journal;
// The free-space precheck calls the Windows API, which has no standard-library
// equivalent; the module keeps its raw pointers to itself.
pub mod process;
#[allow(unsafe_code)]
pub mod reinstall;
// Installing a reparse point is the one thing the standard library cannot do;
// the module keeps its raw pointers to itself and everything below it is safe.
#[allow(unsafe_code)]
pub mod junction;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};
use o824_model::Timestamp;
use o824_plan::{Plan, Step};

use crate::journal::{Journal, State};

/// The profile directory name inside a DSH home.
const PROFILES_DIR_NAME: &str = "profiles";
/// The profile diagnosed when the installation lists none.
const DEFAULT_PROFILE: &str = "desktop";
/// The pnpm whitelist file name inside a profile.
const WORKSPACE_FILE_NAME: &str = "pnpm-workspace.yaml";

/// Executes `plan` against the installation at `root`, using `data_root` for the
/// snapshot store and the journal.
pub fn run(data_root: &Path, plan: &mut Plan, root: &Path) -> Result<(), Error> {
    // 1. The machine lock serialises every writer. It is released when the guard
    // drops, including on every failure path.
    let mut handle = o824_lock::acquire(data_root)?;

    // 2. A stale plan is never applied: its fingerprint no longer describes the
    // installation.
    let now = Timestamp::now();
    if plan.expires_at < now {
        let _ = handle.release();
        return Err(drift(format!(
            "apply: plan {} expired at {}; re-run the plan",
            plan.id,
            plan.expires_at.to_rfc3339_seconds()
        )));
    }

    // 3. Re-verify the fingerprint before anything is written (spec §15.4).
    if let Err(error) = verify_fingerprint(root, &plan.fingerprint) {
        let _ = handle.release();
        return Err(error);
    }

    // 4. Only a plan that rewrites files needs a checkpoint. A start rewrites nothing
    // and a stop rewrites nothing, and the record is the whole subject of those steps —
    // stopping a process the kernel started is exactly the case where the installation
    // may be gone. Snapshotting it would fail on a machine that has no installation,
    // which says nothing about whether the process can be stopped.
    if !plan.steps.iter().any(|step| !is_process_step(&step.kind)) {
        for step in plan.steps.iter_mut() {
            if let Err(error) = execute(data_root, &plan.install_id, root, step) {
                let _ = handle.release();
                return Err(error);
            }
        }
        let _ = handle.release();
        return Ok(());
    }

    // 5. The first rollback checkpoint. A snapshot that did not reach disk aborts the
    // run before any mutation.
    let snapshot = match o824_snapshot::create(data_root, &plan.install_id, root) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let _ = handle.release();
            return Err(error);
        }
    };

    // The snapshot can only restore files that existed when it was taken, so the
    // pre-apply existence of every file a step may write is recorded here. A
    // rollback then also removes a file the repair created.
    let tracked = tracked_paths(data_root, plan, root);
    let existed: BTreeMap<PathBuf, bool> = tracked
        .iter()
        .map(|path| {
            (
                path.clone(),
                path.exists() || std::fs::symlink_metadata(path).is_ok(),
            )
        })
        .collect();

    // 5. The journal is written before the first mutation, so an interrupted apply
    // is diagnosable (spec §15.3).
    let mut entry = Journal::new(&plan.id, &plan.install_id, root, now, plan.steps.clone());
    if let Err(error) = journal::write(data_root, &plan.id, &entry) {
        let _ = handle.release();
        return Err(error);
    }

    // 6. Steps run in order; the first failure stops the plan.
    //
    // A link step replaces a reparse point that the snapshot does not cover
    // (`node_modules` is deliberately out of scope), so its prior state is
    // recorded as the step is reached and replayed on rollback.
    let mut replaced: Vec<LinkState> = Vec::new();

    for index in 0..plan.steps.len() {
        let step = plan.steps[index].clone();
        if replaces_link(&step.kind) {
            let state = match capture_link_state(Path::new(&step.path)) {
                Ok(state) => state,
                Err(error) => {
                    return fail(
                        &mut handle,
                        data_root,
                        plan,
                        root,
                        &snapshot,
                        &tracked,
                        &existed,
                        &replaced,
                        &mut entry,
                        error,
                    )
                }
            };
            if state.kind == LinkStateKind::Other {
                // Never clobber something the repair did not create.
                let error = drift(format!(
                    "apply: {:?} exists and is not a link; refusing to replace something this repair did not create",
                    step.path
                ));
                return fail(
                    &mut handle,
                    data_root,
                    plan,
                    root,
                    &snapshot,
                    &tracked,
                    &existed,
                    &replaced,
                    &mut entry,
                    error,
                );
            }
            replaced.push(state);
        }
        if let Err(error) = execute(data_root, &plan.install_id, root, &mut plan.steps[index]) {
            // 8. Roll back to the checkpoint and report the original error.
            return fail(
                &mut handle,
                data_root,
                plan,
                root,
                &snapshot,
                &tracked,
                &existed,
                &replaced,
                &mut entry,
                error,
            );
        }
    }

    // 7. Success.
    entry.state = State::Done;
    entry.finished_at = Some(Timestamp::now());
    entry.steps = plan.steps.clone();
    let result = journal::write(data_root, &plan.id, &entry);
    let _ = handle.release();
    result
}

/// Rolls the installation back to the checkpoint and reports `cause`.
///
/// A journal with state `failed` and an empty `rollbackError` must mean the tree
/// is back where it started, so every restore failure is recorded.
#[allow(clippy::too_many_arguments)]
fn fail(
    handle: &mut o824_lock::Handle,
    data_root: &Path,
    plan: &Plan,
    root: &Path,
    snapshot: &o824_snapshot::Snapshot,
    tracked: &[PathBuf],
    existed: &BTreeMap<PathBuf, bool>,
    replaced: &[LinkState],
    entry: &mut Journal,
    cause: Error,
) -> Result<(), Error> {
    let mut rollback: Vec<String> = Vec::new();
    if let Err(error) = o824_snapshot::restore(data_root, snapshot, root) {
        rollback.push(error.to_string());
    }
    if let Err(error) = remove_created(tracked, existed) {
        rollback.push(error);
    }
    if let Err(error) = restore_links(replaced) {
        rollback.push(error);
    }

    entry.state = State::Failed;
    entry.finished_at = Some(Timestamp::now());
    entry.error = cause.to_string();
    if !rollback.is_empty() {
        entry.rollback_error = rollback.join("; ");
    }
    entry.steps = plan.steps.clone();
    // The journal write on the failure path must not mask the failure being
    // reported; the error itself is the contract.
    let _ = journal::write(data_root, &plan.id, entry);
    let _ = handle.release();
    Err(cause)
}

/// Runs one step.
fn execute(data_root: &Path, install_id: &str, root: &Path, step: &mut Step) -> Result<(), Error> {
    if step.kind.trim().is_empty() {
        return Err(internal("apply: a step has no kind"));
    }
    if step.path.trim().is_empty() {
        return Err(internal(format!(
            "apply: step {:?} has no target path",
            step.kind
        )));
    }
    let path = PathBuf::from(&step.path);
    if !is_process_step(&step.kind) && !path.is_absolute() {
        return Err(internal(format!(
            "apply: step {:?} path {:?} is not absolute",
            step.kind, step.path
        )));
    }
    if !is_process_step(&step.kind) && !under_root(root, &path) {
        return Err(drift(format!(
            "apply: step {:?} path {:?} is outside the installation root {:?}; re-run the plan",
            step.kind,
            step.path,
            root.display().to_string()
        )));
    }

    // A process step starts or stops a form; it rewrites no file and its path is an
    // executable outside the installation root, so the root rules below do not apply.
    if is_process_step(&step.kind) {
        return crate::process::run(data_root, install_id, step);
    }

    match step.kind.as_str() {
        o824_plan::KIND_DEDUPE_LOADER => dedupe_loader(step),
        o824_plan::KIND_REBUILD_LINK => rebuild_link(step),
        o824_plan::KIND_REBUILD_BRIDGE => rebuild_bridge(step),
        o824_plan::KIND_ALLOW_BUILDS => allow_builds(step),
        o824_plan::KIND_RESTORE_SNAPSHOT => restore_snapshot(data_root, install_id, root, step),
        o824_plan::KIND_REPLACE_FILE => replace_file(step),
        o824_plan::KIND_REINSTALL_PROFILE => reinstall::reinstall_profile(step),
        other => Err(internal(format!("apply: unknown step kind {other:?}"))),
    }
}

/// True when a step starts or stops a form rather than editing a file.
pub fn is_process_step(kind: &str) -> bool {
    matches!(
        kind,
        o824_plan::KIND_START_WEB
            | o824_plan::KIND_STOP_WEB
            | o824_plan::KIND_START_DESKTOP
            | o824_plan::KIND_STOP_DESKTOP
    )
}

/// True when a step replaces a reparse point.
fn replaces_link(kind: &str) -> bool {
    kind == o824_plan::KIND_REBUILD_LINK || kind == o824_plan::KIND_REBUILD_BRIDGE
}

/// Re-hashes every file the plan recorded, refusing to continue when one changed,
/// disappeared, or cannot be read.
fn verify_fingerprint(root: &Path, fingerprint: &BTreeMap<String, String>) -> Result<(), Error> {
    for (rel, expected) in fingerprint {
        let path = root.join(rel.replace('/', "\\"));
        let actual = match hash_file(&path) {
            Ok(actual) => actual,
            Err(error) => {
                return Err(drift(format!(
                    "apply: cannot re-read {rel:?} before repairing it: {error}; re-run the plan"
                )));
            }
        };
        if &actual != expected {
            return Err(drift(format!(
                "apply: {rel:?} changed since the plan was built (sha256 {actual}, plan expected {expected}); re-run the plan"
            )));
        }
    }
    Ok(())
}

/// Rewrites `cordis.patch.yml` so the rows whose id is in the step's args are
/// dropped.
///
/// No P0 finding maps to this step any more: F03 is advisory and never drives an
/// automatic edit. The kernel therefore refuses the step rather than shipping a
/// writer nothing exercises — a repair path that no diagnosis can reach is a
/// liability, not a feature. `apply` keeps the kind so a hand-built plan gets a
/// clear answer instead of "unknown step".
fn dedupe_loader(_step: &Step) -> Result<(), Error> {
    Err(Error::new(
        Code::Unsupported,
        "apply: dedupe-loader is not reachable from any P0 finding (F03 is advisory), so this          build does not carry its writer; the step is refused rather than half-implemented",
    ))
}

/// Recreates a profile `node_modules` link as a directory junction.
fn rebuild_link(step: &Step) -> Result<(), Error> {
    if step.from.trim().is_empty() {
        return Err(internal(format!(
            "apply: rebuild-link {:?} has no target",
            step.path
        )));
    }
    create_junction(Path::new(&step.path), Path::new(&step.from))
}

/// Recreates an out-of-tree plugin's dependency bridge.
fn rebuild_bridge(step: &Step) -> Result<(), Error> {
    if step.from.trim().is_empty() {
        return Err(internal(format!(
            "apply: rebuild-bridge {:?} has no target",
            step.path
        )));
    }
    create_junction(Path::new(&step.path), Path::new(&step.from))
}

/// Replaces `path` with a directory junction to `target`, refusing a target that
/// cannot be a profile bundle.
///
/// A junction to a directory that does not exist would leave the link just as
/// broken as it was while claiming success, so the forward path insists that the
/// target exists and is a directory. A rollback does not: it must be able to put
/// back a dangling link exactly as it was.
fn create_junction(path: &Path, target: &Path) -> Result<(), Error> {
    match fs::metadata(target) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(internal(format!(
                "apply: the link target {:?} is not a directory",
                target.display().to_string()
            )));
        }
        Err(error) => {
            return Err(drift(format!(
                "apply: the link target {:?} does not exist ({error}); reinstall the bundle and re-run the plan",
                target.display().to_string()
            )));
        }
    }
    install_junction(path, target)
}

/// Replaces `path` with a directory junction to `target` without judging the
/// target.
///
/// The new junction is built beside the old entry first and moved into place only
/// after the native reparse-point call succeeded, so a caller that cannot create
/// the link leaves the existing link exactly as it was. A real directory is never
/// deleted.
fn install_junction(path: &Path, target: &Path) -> Result<(), Error> {
    let absolute = o824_model::path::clean(
        &std::path::absolute(target)
            .map_err(|error| file_error("resolve the link target", target, &error))?,
    );

    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)
            .map_err(|error| file_error("create the link directory", directory, &error))?;
    }

    let staging = junction_staging_path(path)?;
    // A stale staging entry from a crashed run is never a user's file: the name
    // carries this kernel's marker.
    remove_link(&staging)?;
    // FSCTL_SET_REPARSE_POINT replaces an empty directory's contents with the
    // reparse point, so the staging path is created as exactly that.
    fs::create_dir(&staging)
        .map_err(|error| file_error("create the junction staging directory", &staging, &error))?;
    if let Err(error) = junction::set_junction(&staging, &absolute) {
        let _ = fs::remove_dir(&staging);
        return Err(internal(format!(
            "apply: cannot create the junction {:?} -> {:?}: {error}",
            path.display().to_string(),
            absolute.display().to_string()
        )));
    }
    if let Err(error) = remove_link(path) {
        let _ = fs::remove_dir(&staging);
        return Err(error);
    }
    if let Err(error) = fs::rename(&staging, path) {
        let _ = fs::remove_dir(&staging);
        return Err(file_error("install the rebuilt link", path, &error));
    }
    Ok(())
}

/// What a link path held before a step replaced it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkStateKind {
    /// The step created the entry.
    Absent,
    /// The entry was a reparse point with a recorded target.
    Link,
    /// The entry was a real file or directory.
    Other,
}

/// The pre-step state of one link path. The snapshot cannot restore it —
/// `node_modules` is out of its scope — so a rollback replays these records.
#[derive(Clone, Debug)]
struct LinkState {
    path: PathBuf,
    kind: LinkStateKind,
    target: PathBuf,
}

/// Records what `path` holds right now.
fn capture_link_state(path: &Path) -> Result<LinkState, Error> {
    if fs::symlink_metadata(path).is_err() && !path.exists() {
        return Ok(LinkState {
            path: path.to_path_buf(),
            kind: LinkStateKind::Absent,
            target: PathBuf::new(),
        });
    }
    match fs::read_link(path) {
        Ok(target) => Ok(LinkState {
            path: path.to_path_buf(),
            kind: LinkStateKind::Link,
            target,
        }),
        // Not a reparse point: a real file or directory lives here.
        Err(_) => Ok(LinkState {
            path: path.to_path_buf(),
            kind: LinkStateKind::Other,
            target: PathBuf::new(),
        }),
    }
}

/// Replays the recorded link states in reverse order, so the tree ends where it
/// started. Returns every failure, joined, so the journal's `rollbackError` tells
/// the truth.
fn restore_links(states: &[LinkState]) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();
    for state in states.iter().rev() {
        match state.kind {
            LinkStateKind::Absent => {
                if let Err(error) = fs::remove_dir(&state.path) {
                    if error.kind() != io::ErrorKind::NotFound {
                        failures.push(format!("{}: {error}", state.path.display()));
                    }
                }
            }
            LinkStateKind::Link => {
                if link_target_matches(state) {
                    continue;
                }
                if let Err(error) = install_junction(&state.path, &state.target) {
                    failures.push(format!("{}: {error}", state.path.display()));
                }
            }
            LinkStateKind::Other => {}
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// True when `path` is still a link to the recorded target.
fn link_target_matches(state: &LinkState) -> bool {
    match fs::read_link(&state.path) {
        Ok(current) => normalize_link_target(&current)
            .display()
            .to_string()
            .eq_ignore_ascii_case(&normalize_link_target(&state.target).display().to_string()),
        Err(_) => false,
    }
}

/// Strips the prefixes Windows puts on a reparse point's target so two spellings
/// of the same directory compare equal.
fn normalize_link_target(target: &Path) -> PathBuf {
    o824_model::path::clean(&o824_model::path::strip_nt_namespace(target))
}

/// Returns an unused sibling path for a junction under construction.
fn junction_staging_path(path: &Path) -> Result<PathBuf, Error> {
    let directory = path.parent().unwrap_or(path);
    let base = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    for attempt in 0..8 {
        let candidate = directory.join(format!("{base}.o824-new-{attempt}"));
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    Err(internal(format!(
        "apply: cannot allocate a staging path beside {:?}",
        path.display().to_string()
    )))
}

/// Removes `path` when it is a link, and does nothing when it is already absent.
///
/// Removing a junction or symbolic link removes the link itself, and fails on a
/// real non-empty directory — which is exactly the behaviour a repair wants.
fn remove_link(path: &Path) -> Result<(), Error> {
    if fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(file_error("remove the stale link", path, &error)),
    }
}

/// The `allow-builds` step.
fn allow_builds(step: &mut Step) -> Result<(), Error> {
    if step.args.is_empty() {
        return Ok(());
    }
    let outcome = allow_builds::set_approvals(Path::new(&step.path), &step.args)?;
    if outcome.migrated > 0 {
        step.detail = format!(
            "converted the allowBuilds sequence to the pnpm 11 map form ({} entries)",
            outcome.migrated
        );
    }
    Ok(())
}

/// The `replace-file` step: copies `from` over `path`.
fn replace_file(step: &Step) -> Result<(), Error> {
    if step.from.trim().is_empty() {
        return Err(internal(format!(
            "apply: replace-file {:?} has no source",
            step.path
        )));
    }
    // The source is read before the target's directory is touched, so a step that
    // fails leaves no empty directory behind.
    let data = fs::read(&step.from)
        .map_err(|error| file_error("read the replacement file", Path::new(&step.from), &error))?;
    write_file_atomically(Path::new(&step.path), &data)
}

/// The `restore-snapshot` step.
fn restore_snapshot(
    data_root: &Path,
    install_id: &str,
    root: &Path,
    step: &Step,
) -> Result<(), Error> {
    if install_id.trim().is_empty() {
        return Err(internal(
            "apply: a restore-snapshot step needs the installation id",
        ));
    }
    let snapshots = o824_snapshot::list(data_root, install_id)?;
    let snapshot = snapshots
        .into_iter()
        .find(|snapshot| {
            snapshot.id == step.to
                || (step.to.is_empty() && snapshot.dir.eq_ignore_ascii_case(&step.from))
        })
        .ok_or_else(|| {
            drift(format!(
                "apply: the snapshot {:?} named by the plan is gone; re-run the plan",
                step.to
            ))
        })?;

    o824_snapshot::restore(data_root, &snapshot, root)?;
    verify_restored_profiles(root, step, &snapshot)
}

/// Re-reads the profiles a restore touched and fails when one of the files the
/// restore was supposed to heal still does not parse.
fn verify_restored_profiles(
    root: &Path,
    step: &Step,
    snapshot: &o824_snapshot::Snapshot,
) -> Result<(), Error> {
    let mut targets: Vec<String> = Vec::new();
    if !step.args.is_empty() {
        for rel in &step.args {
            let rel = rel.trim();
            if !rel.is_empty() {
                targets.push(rel.replace('\\', "/"));
            }
        }
    } else {
        for entry in &snapshot.files {
            if profile_of(&entry.rel).is_some() || entry.rel == WORKSPACE_FILE_NAME {
                targets.push(entry.rel.clone());
            }
        }
    }
    if targets.is_empty() {
        return Ok(());
    }

    for profile in profiles_under(root, &targets) {
        for finding in o824_verify::profile::profile_parse(root, &profile) {
            if finding.code != Code::ProfileParse {
                continue;
            }
            let file = finding
                .params
                .get("file")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .replace('\\', "/");
            if targets.contains(&file) {
                return Err(drift(format!(
                    "apply: {file:?} still does not parse after restoring snapshot {:?}; the snapshot cannot repair this profile",
                    snapshot.id
                )));
            }
        }
    }
    Ok(())
}

/// The profile name a slash-separated profile file belongs to, or `None` for a
/// file outside `profiles\<name>\`.
fn profile_of(rel: &str) -> Option<String> {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() >= 3 && parts[0].eq_ignore_ascii_case(PROFILES_DIR_NAME) {
        Some(parts[1].to_owned())
    } else {
        None
    }
}

/// The profiles whose files must be re-checked after a restore.
fn profiles_under(root: &Path, targets: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut every_profile = false;
    for rel in targets {
        match profile_of(rel) {
            Some(name) => {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            None => every_profile = true,
        }
    }
    if every_profile {
        if let Ok(entries) = fs::read_dir(root.join(PROFILES_DIR_NAME)) {
            let mut found: Vec<String> = entries
                .flatten()
                .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            found.sort();
            names.extend(found);
        }
        if names.is_empty() {
            names.push(DEFAULT_PROFILE.to_owned());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Every file the plan may create or rewrite, sorted.
fn tracked_paths(data_root: &Path, plan: &Plan, root: &Path) -> Vec<PathBuf> {
    let mut seen: Vec<PathBuf> = Vec::new();
    for step in &plan.steps {
        for path in written_paths(step) {
            if !seen.contains(&path) {
                seen.push(path);
            }
        }
        if step.kind == o824_plan::KIND_RESTORE_SNAPSHOT && !plan.install_id.trim().is_empty() {
            if let Ok(snapshots) = o824_snapshot::list(data_root, &plan.install_id) {
                if let Some(snapshot) = snapshots.iter().find(|snapshot| {
                    snapshot.id == step.to
                        || (step.to.is_empty() && snapshot.dir.eq_ignore_ascii_case(&step.from))
                }) {
                    for entry in &snapshot.files {
                        let path = root.join(entry.rel.replace('/', "\\"));
                        if !seen.contains(&path) {
                            seen.push(path);
                        }
                    }
                }
            }
        }
    }
    // The fingerprint names every file a step will rewrite, so it is a second,
    // independent source for the same set.
    for rel in plan.fingerprint.keys() {
        let path = root.join(rel.replace('/', "\\"));
        if !seen.contains(&path) {
            seen.push(path);
        }
    }
    seen.sort();
    seen
}

/// The files one step may create or rewrite.
fn written_paths(step: &Step) -> Vec<PathBuf> {
    match step.kind.as_str() {
        o824_plan::KIND_DEDUPE_LOADER
        | o824_plan::KIND_ALLOW_BUILDS
        | o824_plan::KIND_REPLACE_FILE
        | o824_plan::KIND_REBUILD_LINK
        | o824_plan::KIND_REBUILD_BRIDGE => {
            if step.path.trim().is_empty() {
                Vec::new()
            } else {
                vec![PathBuf::from(&step.path)]
            }
        }
        _ => Vec::new(),
    }
}

/// Deletes every tracked path that did not exist before the repair and that a
/// step therefore created.
fn remove_created(tracked: &[PathBuf], existed: &BTreeMap<PathBuf, bool>) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();
    for path in tracked {
        if existed.get(path).copied().unwrap_or(false) {
            continue;
        }
        if let Err(error) = fs::remove_file(path) {
            if error.kind() != io::ErrorKind::NotFound {
                if let Err(directory_error) = fs::remove_dir(path) {
                    if directory_error.kind() != io::ErrorKind::NotFound {
                        failures.push(format!("{}: {directory_error}", path.display()));
                    }
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// True when `path` is `root` itself or lives inside it.
fn under_root(root: &Path, path: &Path) -> bool {
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

/// The lowercase hex sha256 of a file.
fn hash_file(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};

    let data = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

/// Writes `data` over `path` atomically.
fn write_file_atomically(path: &Path, data: &[u8]) -> Result<(), Error> {
    let Some(directory) = path.parent() else {
        return Err(file_error(
            "derive the file directory",
            path,
            &io::Error::other("the path has no parent directory"),
        ));
    };
    fs::create_dir_all(directory)
        .map_err(|error| file_error("create the file directory", directory, &error))?;
    let temporary = directory.join(format!(
        "{}.o824-new-{}",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_owned()),
        std::process::id()
    ));
    let written = (|| -> io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(file_error("write the file", path, &error));
    }
    Ok(())
}

/// A plan that cannot be applied because the world moved; the caller must
/// re-plan, not retry.
fn drift(message: String) -> Error {
    Error::new(Code::StateDrift, message)
}

/// A kernel-internal failure: the step itself was malformed.
fn internal(message: impl Into<String>) -> Error {
    Error::new(Code::Internal, message)
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
    use o824_fixtures::{Fixture, TempDir};
    use o824_model::{Install, Timestamp};

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

    fn plan_for(fixture: &Fixture, store: &TempDir) -> (Install, Plan) {
        let install = install_of(fixture.root());
        let envelope = o824_verify::doctor(&install, store.path());
        let plan = o824_plan::build(&install, &envelope, Timestamp::now(), store.path())
            .expect("build the plan");
        (install, plan)
    }

    #[test]
    fn an_allow_builds_repair_clears_the_finding() {
        let fixture = o824_fixtures::build("git-dep").expect("fixture");
        let store = TempDir::new("o824-apply-store-").expect("store");
        let (install, mut plan) = plan_for(&fixture, &store);
        assert_eq!(plan.steps.len(), 1);

        run(store.path(), &mut plan, fixture.root()).expect("apply");

        // The repaired tree diagnoses clean.
        let after = o824_verify::doctor(&install, store.path());
        assert!(after.findings.is_empty(), "{:?}", after.findings);

        // The whitelist kept its other keys and gained the approval key.
        let text = std::fs::read_to_string(fixture.profile_dir().join("pnpm-workspace.yaml"))
            .expect("read the whitelist");
        assert!(text.contains("packages:"), "{text}");
        assert!(text.contains("\"@deepseek-ai/dsh-base\": true"), "{text}");
        assert!(
            text.contains("\"dshmarket@git+https://github.com/deepseek-ai/dshmarket.git\": true"),
            "{text}"
        );
    }

    #[test]
    fn a_rebuild_bridge_repair_installs_a_junction_and_clears_the_finding() {
        let fixture = o824_fixtures::build("dangling-bridge").expect("fixture");
        let store = TempDir::new("o824-apply-store-").expect("store");
        let (install, mut plan) = plan_for(&fixture, &store);
        assert_eq!(plan.steps.len(), 1, "{:?}", plan.steps);
        assert_eq!(plan.steps[0].kind, o824_plan::KIND_REBUILD_BRIDGE);

        run(store.path(), &mut plan, fixture.root()).expect("apply");

        let after = o824_verify::doctor(&install, store.path());
        assert!(after.findings.is_empty(), "{:?}", after.findings);

        // The bridge resolves to the profile's own bundle.
        let bridge = PathBuf::from(&plan.steps[0].path);
        assert!(
            bridge.join("package.json").is_file(),
            "the bridge must resolve through to a package"
        );
    }

    #[test]
    fn a_failed_step_rolls_the_tree_back_and_records_it() {
        let fixture = o824_fixtures::build("git-dep").expect("fixture");
        let store = TempDir::new("o824-apply-store-").expect("store");
        let (install, mut plan) = plan_for(&fixture, &store);
        let whitelist = fixture.profile_dir().join("pnpm-workspace.yaml");
        let before = std::fs::read_to_string(&whitelist).expect("read before");

        // A second step that cannot run: its target does not exist.
        let mut broken = Step::new(o824_plan::KIND_REBUILD_LINK, plan.steps[0].path.clone());
        broken.from = fixture.root().join("no-such-target").display().to_string();
        plan.steps.push(broken);

        let error = run(store.path(), &mut plan, fixture.root()).expect_err("must fail");
        assert_eq!(error.code, Code::StateDrift, "{error}");

        // The first step's write was rolled back.
        assert_eq!(
            std::fs::read_to_string(&whitelist).expect("read after"),
            before,
            "the allow-builds write must be undone"
        );

        // The journal tells the truth: failed, and the rollback itself worked.
        let path = journal::journal_path(store.path(), &plan.id).expect("journal path");
        let text = std::fs::read_to_string(path).expect("journal");
        assert!(text.contains("\"state\": \"failed\""), "{text}");
        assert!(
            !text.contains("rollbackError"),
            "the rollback succeeded: {text}"
        );
        assert!(text.contains("\"finishedAt\""), "{text}");

        // And the tree still diagnoses the same fault.
        let after = o824_verify::doctor(&install, store.path());
        assert_eq!(after.findings.len(), 1, "{:?}", after.findings);
    }

    #[test]
    fn an_expired_plan_is_refused_before_anything_is_written() {
        let fixture = o824_fixtures::build("git-dep").expect("fixture");
        let store = TempDir::new("o824-apply-store-").expect("store");
        let (_install, mut plan) = plan_for(&fixture, &store);
        plan.expires_at = Timestamp::now();

        let error = run(store.path(), &mut plan, fixture.root()).expect_err("must fail");
        assert_eq!(error.code, Code::StateDrift);
        assert!(error.fallback.contains("expired"), "{}", error.fallback);
    }

    #[test]
    fn a_changed_file_is_state_drift_before_anything_is_written() {
        let fixture = o824_fixtures::build("git-dep").expect("fixture");
        let store = TempDir::new("o824-apply-store-").expect("store");
        let (_install, mut plan) = plan_for(&fixture, &store);

        // The plan fingerprinted the whitelist; change it.
        let whitelist = fixture.profile_dir().join("pnpm-workspace.yaml");
        std::fs::write(&whitelist, "packages: []\n").expect("change");

        let error = run(store.path(), &mut plan, fixture.root()).expect_err("must fail");
        assert_eq!(error.code, Code::StateDrift);
        assert!(
            error.fallback.contains("changed since the plan was built"),
            "{}",
            error.fallback
        );
    }

    #[test]
    fn a_step_outside_the_root_is_refused() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = TempDir::new("o824-apply-store-").expect("store");
        let (_install, mut plan) = plan_for(&fixture, &store);
        plan.steps.push(Step::new(
            o824_plan::KIND_REPLACE_FILE,
            std::env::temp_dir()
                .join("o824-outside.txt")
                .display()
                .to_string(),
        ));

        let error = run(store.path(), &mut plan, fixture.root()).expect_err("must fail");
        assert_eq!(error.code, Code::StateDrift);
        assert!(
            error.fallback.contains("outside the installation root"),
            "{}",
            error.fallback
        );
    }

    #[test]
    fn a_successful_run_records_a_done_journal() {
        let fixture = o824_fixtures::build("git-dep").expect("fixture");
        let store = TempDir::new("o824-apply-store-").expect("store");
        let (_install, mut plan) = plan_for(&fixture, &store);

        run(store.path(), &mut plan, fixture.root()).expect("apply");

        let path = journal::journal_path(store.path(), &plan.id).expect("journal path");
        let text = std::fs::read_to_string(path).expect("journal");
        assert!(text.contains("\"state\": \"done\""), "{text}");
        assert!(text.contains("\"finishedAt\""), "{text}");
        assert!(!text.contains("\"error\""), "{text}");
    }
}
