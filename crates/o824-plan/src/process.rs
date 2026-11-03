//! Plans that start or stop a form.
//!
//! §21.5 is explicit that the protocol has no `o824.start`: starting and stopping are
//! `plan` actions, run through the same plan/apply flow — and the same confirmation — as
//! a repair. A separate RPC would mean a second path that drifts, and the kernel would
//! have to trust a caller's say-so instead of a plan it wrote itself.
//!
//! These steps carry no fingerprint: nothing on disk is rewritten, so there is nothing to
//! re-hash before running them. What makes them safe to run later is different — the
//! process record's identity check, which lives in `o824-formproc`.

use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};
use o824_model::{Install, Timestamp};

use crate::{Plan, Step, KIND_RESTORE_SNAPSHOT, PLAN_TTL_SECONDS};

/// Start the form an installation provides.
pub const ACTION_START: &str = "start";
/// Stop the form an installation provides.
pub const ACTION_STOP: &str = "stop";
/// Put the installation's files back the way the last snapshot has them.
pub const ACTION_ROLLBACK: &str = "rollback";

/// Start the web form (`dsh --profile web`).
pub const KIND_START_WEB: &str = "start-web";
/// Stop the web form this kernel started.
pub const KIND_STOP_WEB: &str = "stop-web";
/// Start the desktop app.
pub const KIND_START_DESKTOP: &str = "start-desktop";
/// Stop the desktop app this kernel started.
pub const KIND_STOP_DESKTOP: &str = "stop-desktop";

/// The argument prefix that carries the DSH home a form is started with.
pub const ARG_HOME: &str = "home=";
/// The argument prefix that carries the port a web form listens on.
pub const ARG_PORT: &str = "port=";
/// The argument prefix that carries the installation whose process is stopped.
pub const ARG_INSTALL_ID: &str = "installId=";

/// What a start needs beyond the installation.
#[derive(Clone, Debug, Default)]
pub struct StartOptions {
    /// The DSH home to start the form against.
    pub home: PathBuf,
    /// The port a web form asks for; `None` lets the system choose one.
    pub port: Option<u16>,
    /// The web package directory, when it is already known.
    pub package_dir: Option<PathBuf>,
}

/// Builds the plan for `action` against `install`.
///
/// The caller chooses the step kind by asking for the action; the installation's shape
/// decides whether that action exists. A tui is refused with `E_KIND_UNSUPPORTED`: it is a
/// terminal application, and a kernel that spawned one detached would produce a process
/// nobody can type into.
pub fn build_process(
    install: &Install,
    action: &str,
    now: Timestamp,
    data_root: &Path,
    options: &StartOptions,
) -> Result<Plan, Error> {
    let id = process_plan_id(install, root_of(install).as_path(), now, action);
    let expires = now.plus(std::time::Duration::from_secs(PLAN_TTL_SECONDS));
    let mut plan = Plan {
        id,
        install_id: install.install_id.clone(),
        root: install.root.clone(),
        created_at: now,
        expires_at: expires,
        steps: Vec::new(),
        fingerprint: Default::default(),
    };
    if action == ACTION_ROLLBACK {
        // §21.5 lists rollback among the plan actions, and the machinery exists: a snapshot
        // is taken before every apply, and restoring one is a step `apply` already runs. The
        // only thing missing was a way to ask for it.
        let snapshots = o824_snapshot::list(data_root, &install.install_id)
            .map_err(|error| Error::new(error.code, error.fallback.clone()))?;
        let Some(newest) = snapshots.first() else {
            return Err(Error::new(
                Code::StateDrift,
                format!(
                    "rollback: installation {:?} has no snapshot to go back to",
                    install.install_id
                ),
            )
            .with_param("installId", install.install_id.clone()));
        };
        // A snapshot covers the DSH home, not the application directory: restoring into the
        // application root would write `profiles\` where it does not belong.
        let scope = if install.home.is_empty() {
            install.root.clone()
        } else {
            install.home.clone()
        };
        plan.root = scope.clone();
        let mut step = Step::new(KIND_RESTORE_SNAPSHOT, scope);
        step.from = newest.dir.clone();
        step.to = newest.id.clone();
        plan.steps.push(step);
        return Ok(plan);
    }
    let _ = data_root;

    match (action, install.kind.as_str()) {
        (ACTION_START, "web") => {
            let package_dir = options.package_dir.clone().ok_or_else(|| {
                unsupported("web", "the web form's package directory is not known")
            })?;
            plan.steps.push(Step {
                kind: KIND_START_WEB.to_owned(),
                path: package_dir.display().to_string(),
                from: String::new(),
                to: String::new(),
                args: vec![
                    format!("{ARG_HOME}{}", options.home.display()),
                    format!("{ARG_PORT}{}", options.port.unwrap_or(0)),
                ],
                detail: String::new(),
            });
        }
        (ACTION_START, "desktop") => {
            let exe = desktop_executable(install).ok_or_else(|| {
                unsupported("desktop", "the installation has no desktop executable")
            })?;
            plan.steps.push(Step {
                kind: KIND_START_DESKTOP.to_owned(),
                path: exe.display().to_string(),
                from: String::new(),
                to: String::new(),
                args: vec![format!("{ARG_HOME}{}", options.home.display())],
                detail: String::new(),
            });
        }
        (ACTION_START, "tui") => {
            return Err(unsupported(
                "tui",
                "a tui runs in a terminal; the kernel cannot start one for you",
            ));
        }
        (ACTION_STOP, "web") => plan.steps.push(stop_step(KIND_STOP_WEB, install)),
        (ACTION_STOP, "desktop") => plan.steps.push(stop_step(KIND_STOP_DESKTOP, install)),
        (ACTION_STOP, "tui") => {
            return Err(unsupported(
                "tui",
                "a tui runs in a terminal; there is no process for the kernel to stop",
            ));
        }
        (ACTION_START | ACTION_STOP, other) => {
            return Err(unsupported(
                other,
                "this build starts and stops the web and desktop forms",
            ));
        }
        (other, _) => {
            return Err(Error::new(
                Code::Usage,
                format!("plan: unknown action {other:?}; expected start or stop"),
            ));
        }
    }
    Ok(plan)
}

/// The plan id: the same shape a repair plan uses, with the action mixed into the seed so
/// two plans built in the same second for the same installation do not collide.
fn process_plan_id(install: &Install, root: &Path, now: Timestamp, action: &str) -> String {
    crate::plan_id_for(install, root, now, action)
}

/// The installation root as a path.
fn root_of(install: &Install) -> PathBuf {
    PathBuf::from(&install.root)
}

fn stop_step(kind: &str, install: &Install) -> Step {
    Step {
        kind: kind.to_owned(),
        path: install.root.clone(),
        from: String::new(),
        to: String::new(),
        args: vec![format!("{ARG_INSTALL_ID}{}", install.install_id)],
        detail: String::new(),
    }
}

/// The desktop executable inside an installation root.
fn desktop_executable(install: &Install) -> Option<PathBuf> {
    let root = PathBuf::from(&install.root);
    let exe = root.join(o824_discovery::EXE_NAME);
    if exe.is_file() {
        return Some(exe);
    }
    // A copied installation keeps the app one level down.
    let nested = root.join("app").join(o824_discovery::EXE_NAME);
    nested.is_file().then_some(nested)
}

fn unsupported(kind: &str, reason: &str) -> Error {
    Error::new(
        Code::Unsupported,
        format!("plan: the {kind} form cannot be started or stopped here: {reason}"),
    )
    .with_param("kind", kind.to_owned())
}
