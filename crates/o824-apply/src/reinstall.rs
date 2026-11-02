//! The `reinstall-profile` step: re-materialise a profile's dependency tree with
//! the host's own pnpm, then swap it in.
//!
//! A broken or missing `node_modules` link cannot be rebuilt locally: the package
//! it stands for lives inside the host's `app.asar`, and a junction cannot point
//! into an asar. The only repair is to let the host's own pnpm install the
//! profile's lockfile again, and the safe place to do that is a copy. The same
//! copy is what runs a build script pnpm had blocked: F17's `allow-builds` step
//! wrote the approval into the profile, the copy carries that whitelist, and pnpm
//! re-evaluates it while installing. The plan wires the order — every
//! `allow-builds` before every reinstall — not this step.
//!
//! The order here is the promise:
//!
//! 1. refuse before writing anything when the profile's volume cannot hold the
//!    copy plus the install (`E_DISK_FULL`, nothing created);
//! 2. copy the profile — manifest, lockfile, plugin directories — into
//!    `<profile>.o824-stage`, leaving `node_modules` behind because pnpm rebuilds
//!    it;
//! 3. run the host's `pnpm install` in that copy;
//! 4. require that the copy now holds a non-empty `node_modules` tree;
//! 5. swap it in with two renames, restoring the old tree if either fails.
//!
//! Until step 5 the profile's `node_modules` is not touched a byte, so a failed
//! install, a failed fetch, or a broken pnpm leaves a profile that repairs exactly
//! as well as it did before. Failures after the new tree is committed (deleting
//! the old tree or the leftover copy) are reported rather than papered over, but
//! they do not undo a verified swap: re-breaking the profile to tidy up would be
//! the wrong trade.

use std::io;
use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};
use o824_plan::Step;

/// The host application's entry point.
const HOST_EXE_NAME: &str = "DeepSeek Harness.exe";
/// The pnpm the host ships, relative to the application root.
const HOST_PNPM_REL_PATH: &str = "resources/runtime/primary-runtime/dependencies/pnpm/bin/pnpm.mjs";
/// The variable that makes the host executable behave as Node.
const ELECTRON_RUN_AS_NODE: &str = "ELECTRON_RUN_AS_NODE";
/// How long one staged install may take.
const REINSTALL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// How much of pnpm's output the journal keeps.
const INSTALL_STDERR_LIMIT: usize = 2 * 1024;
/// The staging directory's suffix, carrying this kernel's marker.
const STAGE_SUFFIX: &str = ".o824-stage";
/// The suffix of the dependency tree being replaced.
const OLD_TREE_SUFFIX: &str = ".o824-old";
/// The profile's lockfile.
const LOCKFILE_NAME: &str = "pnpm-lock.yaml";
/// The dependency tree inside a profile.
const NODE_MODULES_DIR_NAME: &str = "node_modules";
/// The staged install writes a second tree beside the one being repaired, so the
/// volume must hold both.
const REINSTALL_COPY_FACTOR: u64 = 2;
/// Slack on top of the copy factor.
const REINSTALL_HEADROOM: u64 = 256 << 20;

/// Re-materialises a profile's dependency tree and swaps it in.
pub fn reinstall_profile(step: &mut Step) -> Result<(), Error> {
    let profile_dir = o824_model::path::clean(Path::new(&step.path));
    if step.to.trim().is_empty() {
        return Err(internal(format!(
            "apply: reinstall-profile {:?} has no application root to run pnpm from",
            step.path
        )));
    }
    let app_dir = o824_model::path::clean(Path::new(&step.to));
    let node_modules = profile_dir.join(NODE_MODULES_DIR_NAME);
    let stage = PathBuf::from(format!("{}{STAGE_SUFFIX}", profile_dir.display()));

    // 1. The space precheck: the only point where nothing has been created yet.
    let existing = dir_bytes(&node_modules);
    let required = REINSTALL_COPY_FACTOR.saturating_mul(existing) + REINSTALL_HEADROOM;
    let free = free_disk_bytes(&profile_dir).map_err(|error| {
        file_error(
            "measure the profile volume's free space",
            &profile_dir,
            &error,
        )
    })?;
    if free < required {
        return Err(Error::new(
            Code::DiskFull,
            format!(
                "apply: {} needs {} MiB free to reinstall its dependencies ({} MiB of node_modules \
                 x {REINSTALL_COPY_FACTOR} + {} MiB headroom) and the volume has {} MiB; nothing was written",
                profile_dir.display(),
                required >> 20,
                existing >> 20,
                REINSTALL_HEADROOM >> 20,
                free >> 20
            ),
        )
        .with_param("freeBytes", free)
        .with_param("nodeModulesBytes", existing)
        .with_param("path", profile_dir.display().to_string())
        .with_param("requiredBytes", required));
    }

    // 2. Stage. A leftover from an interrupted run is not a user's directory: the
    // name carries this kernel's marker.
    if let Err(error) = std::fs::remove_dir_all(&stage) {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(file_error(
                "remove the leftover staging copy",
                &stage,
                &error,
            ));
        }
    }
    std::fs::create_dir_all(&stage)
        .map_err(|error| file_error("create the staging copy", &stage, &error))?;
    if let Err(error) = copy_profile(&profile_dir, &stage) {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(error);
    }

    // 3. Install in the copy, with the host's own pnpm. Nothing under the real
    // profile is reachable from here, so no failure can damage it.
    if let Err(error) = install_profile_dependencies(&app_dir, &stage) {
        let _ = std::fs::remove_dir_all(&stage);
        step.detail = stderr_tail_of(&error);
        return Err(error);
    }

    // 4. pnpm's exit code is not proof: the tree it claims to have installed has
    // to be there before it replaces anything real.
    let entries = std::fs::read_dir(stage.join(NODE_MODULES_DIR_NAME))
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    if entries == 0 {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(internal(format!(
            "apply: pnpm install in {:?} produced no {NODE_MODULES_DIR_NAME} tree; the profile is unchanged",
            stage.display().to_string()
        )));
    }

    // 5. Swap, then clean up. The two renames are the commit point.
    if let Err(error) = swap_dependency_tree(&node_modules, &stage.join(NODE_MODULES_DIR_NAME)) {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(error);
    }
    if let Err(error) = std::fs::remove_dir_all(&stage) {
        return Err(file_error("remove the staging copy", &stage, &error));
    }

    // 6. One line for the journal, so a user reading it later knows what the swap
    // actually delivered.
    let noun = if entries == 1 { "package" } else { "packages" };
    step.detail = format!("reinstalled {entries} top-level {noun} from {LOCKFILE_NAME}");
    Ok(())
}

/// Replaces `node_modules` with `staged` by renaming each, and puts the old tree
/// back when either rename fails.
///
/// `node_modules` is a real directory here, not a link (a link is F01/F02, which is
/// why the tree is being replaced), and a rename within one volume is the only
/// directory operation atomic enough to leave neither tree half-moved.
fn swap_dependency_tree(node_modules: &Path, staged: &Path) -> Result<(), Error> {
    let old = PathBuf::from(format!("{}{OLD_TREE_SUFFIX}", node_modules.display()));

    // A leftover old tree from an interrupted run would make the first rename fail,
    // and it is this kernel's own marker, never a user's directory.
    if let Err(error) = std::fs::remove_dir_all(&old) {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(file_error(
                "remove the previous dependency tree's leftover",
                &old,
                &error,
            ));
        }
    }

    let had_old = match std::fs::symlink_metadata(node_modules) {
        Ok(_) => {
            if let Err(error) = std::fs::rename(node_modules, &old) {
                return Err(file_error(
                    "move the previous dependency tree aside",
                    node_modules,
                    &error,
                ));
            }
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(file_error(
                "inspect the previous dependency tree",
                node_modules,
                &error,
            ));
        }
    };

    // From here on a failure must not leave the profile without a tree.
    let restore = |cause: Error| -> Error {
        if !had_old {
            return cause;
        }
        match std::fs::rename(&old, node_modules) {
            Ok(()) => cause,
            Err(error) => internal(format!(
                "{cause}; and the previous dependency tree could not be put back: {error}"
            )),
        }
    };

    if let Err(error) = std::fs::rename(staged, node_modules) {
        return Err(restore(file_error(
            "move the new dependency tree into place",
            node_modules,
            &error,
        )));
    }
    if let Err(error) = std::fs::remove_dir_all(&old) {
        // The verified tree is in place; only the tree it replaced is left.
        // Restoring that one would re-break the profile, so the leftover is
        // reported instead of swapped back.
        return Err(file_error(
            "remove the previous dependency tree",
            &old,
            &error,
        ));
    }
    Ok(())
}

/// Copies every top-level entry of the profile into the staging copy, except
/// `node_modules` — pnpm rebuilds it — and dot-directories, which are the host's
/// own bookkeeping and are neither read by pnpm nor worth the copy.
///
/// Everything else is copied, because pnpm needs it: the manifest and the lockfile
/// to know what to install, and the directories a `link:` dependency points at,
/// since the copy does not inherit the profile's siblings.
fn copy_profile(source: &Path, destination: &Path) -> Result<(), Error> {
    let entries = std::fs::read_dir(source)
        .map_err(|error| file_error("list the profile", source, &error))?;
    let mut entries: Vec<std::fs::DirEntry> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case(NODE_MODULES_DIR_NAME) {
            continue;
        }
        if name.starts_with('.') {
            // The host's dot-directories (.plugin-manager, .dsh-market) are
            // skipped. `metadata` rather than the directory entry, because a
            // junction to a directory does not report itself as a directory
            // through `symlink_metadata` on Windows.
            if std::fs::metadata(entry.path())
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
        }
        copy_entry(&entry.path(), &destination.join(&name))?;
    }
    Ok(())
}

/// Copies one file, directory or link, never following a link.
fn copy_entry(source: &Path, destination: &Path) -> Result<(), Error> {
    if let Ok(target) = std::fs::read_link(source) {
        // A link to a directory is a junction in practice, so it is recreated as
        // one; a link to anything else falls back to a symbolic link.
        let links_a_directory = std::fs::metadata(source)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        if links_a_directory && super::junction::set_junction(destination, &target).is_ok() {
            return Ok(());
        }
        return std::os::windows::fs::symlink_file(&target, destination).map_err(|error| {
            file_error(
                "recreate the profile link in the staging copy",
                destination,
                &error,
            )
        });
    }

    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| file_error("inspect the profile entry", source, &error))?;
    if metadata.is_dir() {
        std::fs::create_dir_all(destination)
            .map_err(|error| file_error("create the staging directory", destination, &error))?;
        let children = std::fs::read_dir(source)
            .map_err(|error| file_error("list the profile directory", source, &error))?;
        let mut children: Vec<std::fs::DirEntry> = children.flatten().collect();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            copy_entry(&child.path(), &destination.join(child.file_name()))?;
        }
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| file_error("create the staging directory", parent, &error))?;
    }
    std::fs::copy(source, destination)
        .map(|_| ())
        .map_err(|error| file_error("copy the profile file", destination, &error))
}

/// The program, argv and environment that re-materialise a profile's
/// dependencies: the host's argv form, never a command string.
pub type InstallCommand = (PathBuf, Vec<String>, Vec<(String, String)>);

/// The argv that re-materialises a profile's dependencies: the host's executable
/// running the pnpm it ships, as Node, with the lockfile frozen, in the staged
/// copy.
///
/// `--frozen-lockfile` is what makes the repair reproducible: the profile's
/// `pnpm-lock.yaml` is the record of what should be on disk, so installing from it
/// either produces that tree or fails, instead of quietly resolving something else
/// into a profile the kernel is trying to restore. A profile without a lockfile is
/// legitimate, and pnpm refuses `--frozen-lockfile` when the file is absent, so the
/// flag is passed only when the copy actually holds one.
///
/// `--config.confirmModulesPurge=false` is needed because pnpm asks before it
/// deletes an existing `node_modules` and aborts without a TTY. The staging
/// directory starts empty, but a retried run can leave one behind.
pub fn profile_install_command(
    app_dir: &Path,
    directory: &Path,
    has_lockfile: bool,
) -> Result<InstallCommand, Error> {
    if app_dir.display().to_string().trim().is_empty() {
        return Err(internal(
            "apply: a reinstall-profile step has no application root",
        ));
    }
    if directory.display().to_string().trim().is_empty() {
        return Err(internal(
            "apply: a reinstall-profile step has no staging directory",
        ));
    }
    let program = app_dir.join(HOST_EXE_NAME);
    let mut args = vec![
        "--expose-internals".to_owned(),
        app_dir
            .join(HOST_PNPM_REL_PATH.replace('/', "\\"))
            .display()
            .to_string(),
        "install".to_owned(),
    ];
    if has_lockfile {
        args.push("--frozen-lockfile".to_owned());
    }
    args.push("--config.confirmModulesPurge=false".to_owned());
    Ok((program, args, with_electron_run_as_node()))
}

/// The current environment with exactly one `ELECTRON_RUN_AS_NODE=1` entry.
///
/// An inherited value is replaced rather than duplicated, because Windows resolves
/// a duplicated variable unpredictably and the host executable must run as Node,
/// not as a GUI.
fn with_electron_run_as_node() -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = std::env::vars()
        .filter(|(key, _)| !key.eq_ignore_ascii_case(ELECTRON_RUN_AS_NODE))
        .collect();
    environment.push((ELECTRON_RUN_AS_NODE.to_owned(), "1".to_owned()));
    environment
}

/// Runs the host's `pnpm install` in `directory`.
fn install_profile_dependencies(app_dir: &Path, directory: &Path) -> Result<(), Error> {
    let has_lockfile = directory.join(LOCKFILE_NAME).is_file();
    let (program, args, environment) = profile_install_command(app_dir, directory, has_lockfile)?;
    let output = o824_execx::run_with_deadline(
        &program.display().to_string(),
        &args,
        Some(&environment),
        Some(directory),
        REINSTALL_DEADLINE,
    );
    if let Some(error) = output.error {
        return Err(error);
    }
    if output.code != 0 {
        return Err(reinstall_failed(
            directory,
            output.code,
            &output.stderr,
            &output.stdout,
        ));
    }
    Ok(())
}

/// Turns pnpm's non-zero exit into the step failure the journal records, keeping
/// the tail of its output so the user can see why the install failed — and saying
/// that the profile is unchanged, which is the promise the staging copy keeps.
fn reinstall_failed(stage: &Path, code: i32, stderr: &str, stdout: &str) -> Error {
    // pnpm reports most failures on stdout; only fall back to stderr when it said
    // nothing there, so the user always sees the reason.
    let mut tail = tail_string(stderr, INSTALL_STDERR_LIMIT);
    if tail.trim().is_empty() {
        tail = tail_string(stdout, INSTALL_STDERR_LIMIT);
    }
    let mut message = format!(
        "apply: pnpm install in {:?} exited with code {code}; the profile's node_modules is unchanged",
        stage.display().to_string()
    );
    let trimmed = tail.trim();
    if !trimmed.is_empty() {
        message.push_str(": ");
        message.push_str(trimmed);
    }
    Error::new(Code::Internal, message)
        .with_param("exit", code)
        .with_param("path", stage.display().to_string())
        .with_param("stderr", tail)
}

/// The stderr tail a kernel error recorded, so the step's journal note keeps why
/// pnpm failed.
fn stderr_tail_of(error: &Error) -> String {
    error
        .params
        .get("stderr")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The last `limit` bytes of `text`, marked as truncated when it is not the whole
/// text. The journal must never pretend an excerpt is complete.
pub fn tail_string(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut start = text.len() - limit;
    // Never cut a multi-byte character in half.
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    format!("...{}", &text[start..])
}

/// The total size of the regular files under `dir`, never following a link.
///
/// An unreadable entry is skipped rather than failing the repair: the precheck is a
/// guard rail, not an audit, and a slightly optimistic estimate is better than
/// refusing to repair a profile because one file cannot be measured.
fn dir_bytes(dir: &Path) -> u64 {
    let mut total: u64 = 0;
    o824_verify::link::walk_no_follow(dir, &mut |path, is_link| {
        if is_link {
            return;
        }
        if let Ok(metadata) = std::fs::symlink_metadata(path) {
            if metadata.is_file() {
                total += metadata.len();
            }
        }
    });
    total
}

/// Free bytes on the volume holding `path`.
#[cfg(windows)]
fn free_disk_bytes(path: &Path) -> io::Result<u64> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = path
        .display()
        .to_string()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut free = 0u64;
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call, and
    // `free` is a valid out-parameter.
    unsafe {
        GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None)
            .map_err(|error| io::Error::other(error.to_string()))?;
    }
    Ok(free)
}

/// A build without the Windows API cannot measure free space; the precheck is
/// skipped rather than guessed.
#[cfg(not(windows))]
fn free_disk_bytes(_path: &Path) -> io::Result<u64> {
    Ok(u64::MAX)
}

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
    use o824_fixtures::TempDir;

    #[test]
    fn the_install_argv_is_the_hosts_own_pnpm_as_node() {
        let app = Path::new(r"D:\Harness");
        let stage = Path::new(r"D:\dsh\profiles\.desktop.o824-stage");
        let (program, args, environment) = profile_install_command(app, stage, true).expect("argv");

        assert_eq!(program, Path::new(r"D:\Harness\DeepSeek Harness.exe"));
        assert_eq!(args[0], "--expose-internals");
        assert!(
            args[1].ends_with(r"resources\runtime\primary-runtime\dependencies\pnpm\bin\pnpm.mjs")
        );
        assert_eq!(args[2], "install");
        assert_eq!(args[3], "--frozen-lockfile");
        assert_eq!(args[4], "--config.confirmModulesPurge=false");
        assert_eq!(
            environment
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(ELECTRON_RUN_AS_NODE))
                .count(),
            1,
            "exactly one ELECTRON_RUN_AS_NODE entry"
        );
    }

    #[test]
    fn a_profile_without_a_lockfile_is_not_frozen() {
        let (_, args, _) =
            profile_install_command(Path::new(r"D:\Harness"), Path::new(r"D:\stage"), false)
                .expect("argv");
        assert!(!args.iter().any(|arg| arg == "--frozen-lockfile"));
    }

    #[test]
    fn the_output_tail_is_marked_when_truncated() {
        assert_eq!(tail_string("short", 100), "short");
        let long = "x".repeat(50);
        let tail = tail_string(&long, 10);
        assert!(tail.starts_with("..."), "{tail}");
        assert_eq!(tail.len(), 13);
    }

    #[test]
    fn the_copy_skips_node_modules_and_dot_directories() {
        let source = TempDir::new("o824-reinstall-src-").expect("source");
        std::fs::write(source.path().join("package.json"), b"{}").expect("write");
        std::fs::create_dir(source.path().join("node_modules")).expect("mkdir");
        std::fs::write(source.path().join("node_modules").join("x"), b"x").expect("write");
        std::fs::create_dir(source.path().join(".plugin-manager")).expect("mkdir");
        std::fs::write(source.path().join(".plugin-manager").join("y"), b"y").expect("write");
        std::fs::create_dir(source.path().join("vendor")).expect("mkdir");
        std::fs::write(source.path().join("vendor").join("z"), b"z").expect("write");

        let destination = TempDir::new("o824-reinstall-dst-").expect("destination");
        copy_profile(source.path(), destination.path()).expect("copy");

        assert!(destination.path().join("package.json").is_file());
        assert!(destination.path().join("vendor").join("z").is_file());
        assert!(!destination.path().join("node_modules").exists());
        assert!(!destination.path().join(".plugin-manager").exists());
    }

    #[test]
    fn the_swap_replaces_the_tree_and_keeps_no_leftover() {
        let root = TempDir::new("o824-reinstall-swap-").expect("root");
        let node_modules = root.path().join("node_modules");
        std::fs::create_dir(&node_modules).expect("old tree");
        std::fs::write(node_modules.join("old.txt"), b"old").expect("write");

        let staged = root.path().join("staged");
        std::fs::create_dir(&staged).expect("new tree");
        std::fs::write(staged.join("new.txt"), b"new").expect("write");

        swap_dependency_tree(&node_modules, &staged).expect("swap");

        assert!(node_modules.join("new.txt").is_file());
        assert!(!node_modules.join("old.txt").exists());
        assert!(!root.path().join("node_modules.o824-old").exists());
        assert!(!staged.exists());
    }

    #[test]
    fn a_swap_whose_new_tree_is_missing_puts_the_old_one_back() {
        let root = TempDir::new("o824-reinstall-swap-").expect("root");
        let node_modules = root.path().join("node_modules");
        std::fs::create_dir(&node_modules).expect("old tree");
        std::fs::write(node_modules.join("old.txt"), b"old").expect("write");

        let missing = root.path().join("not-there");
        let error = swap_dependency_tree(&node_modules, &missing).expect_err("must fail");
        assert_eq!(error.code, Code::Internal);
        // The profile still holds its tree.
        assert!(node_modules.join("old.txt").is_file());
        assert!(!root.path().join("node_modules.o824-old").exists());
    }

    #[test]
    fn a_profile_without_node_modules_still_refuses_a_missing_app_root() {
        let mut step = Step::new(
            o824_plan::KIND_REINSTALL_PROFILE,
            r"D:\dsh\profiles\desktop",
        );
        let error = reinstall_profile(&mut step).expect_err("must fail");
        assert_eq!(error.code, Code::Internal);
        assert!(
            error.fallback.contains("application root"),
            "{}",
            error.fallback
        );
    }
}
