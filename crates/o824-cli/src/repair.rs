//! The `repair` subcommand.
//!
//! `repair` is the only command that mutates an installation, so it is deliberate
//! about it. At this milestone the plan side is complete and the execution side
//! is not: `--dry-run` prints exactly the plan `apply` will run, and a run without
//! `--dry-run` refuses with `E_UNSUPPORTED` rather than pretending to have
//! repaired anything. Milestone M3 fills that in.

use std::io::Write;
use std::path::PathBuf;

use o824_errs::{Code, Error};
use o824_model::{Envelope, Install};

use crate::flags::{FlagSet, FlagSpec};
use crate::{exit_code_of, EXIT_FINDINGS, EXIT_OK};

/// The help text for the repair subcommand.
pub const USAGE: &str = "\
Usage:
  o824 repair --install <id> [--profile <name>] [--yes] [--dry-run] [--json]
  o824 repair --root <path> [--home <path>] [--profile <name>] [--yes] [--dry-run] [--json]

Diagnoses one DeepSeek Harness installation, builds a repair plan, and applies
it. Every run takes the machine lock, the plan is bound to the state it was built
against, and the first checkpoint is a verified snapshot.

Flags:
  --install <id>    repair one discovered installation, selected by id
  --root <path>     repair this directory instead of scanning the machine
  --home <path>     DSH home whose profiles are repaired (--root implies --home;
                    this flag overrides that)
  --profile <name>  repair only this profile, not the installation's list
  --yes             apply the plan without asking for confirmation
  --dry-run         print the plan and exit 0 without changing anything
  --json            print the plan (with --dry-run) or the result as JSON
  --with-install    also re-materialise dependencies with the host's pnpm

Exit code 0 means the plan was applied and a fresh diagnosis is clean, 1 means
findings remain, 2 means the command line was unusable, 3 means confirmation was
required, 5 means the state drifted, and 7 means another O824 process holds the
lock.
";

/// Runs `o824 repair`. Returns the process exit code.
pub fn run(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    run_with(args, stdout, stderr, None)
}

/// Runs `o824 repair` with an explicit data root, for tests.
pub fn run_with(
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    data_root: Option<&std::path::Path>,
) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("yes"),
        FlagSpec::boolean("dry-run"),
        FlagSpec::boolean("json"),
        FlagSpec::boolean("with-install"),
        FlagSpec::valued("root"),
        FlagSpec::valued("home"),
        FlagSpec::valued("profile"),
        FlagSpec::valued("install"),
    ]);
    if let Err(error) = flags.parse("repair", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    if let Some(extra) = flags.positional().first() {
        let error = Error::new(
            Code::Usage,
            format!("o824 repair: unexpected argument {extra:?}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let install = match crate::install::select(
        "repair",
        flags.value("install"),
        flags.value("root"),
        flags.value("home"),
    ) {
        Ok(install) => install,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };
    // --profile narrows the iteration to exactly the named profile, like doctor.
    let install = match flags.value("profile").filter(|profile| !profile.is_empty()) {
        Some(profile) => Install {
            profiles: vec![profile.to_owned()],
            ..install
        },
        None => install,
    };

    let data_root: PathBuf = match data_root {
        Some(root) => root.to_path_buf(),
        None => o824_state::data_root(None),
    };

    // The journal lives under the data root, which O824_DATA_ROOT overrides
    // inside the state rule; repair has no data-root flag of its own.
    let envelope = o824_verify::doctor(&install, &data_root);
    let plan = match o824_plan::build_with(
        &install,
        &envelope,
        o824_model::Timestamp::now(),
        &data_root,
        o824_plan::Options {
            install_packages: flags.flag("with-install"),
        },
    ) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    if flags.flag("dry-run") {
        if flags.flag("json") {
            return write_json(stdout, stderr, &plan);
        }
        let _ = stdout.write_all(write_plan(&plan).as_bytes());
        return EXIT_OK;
    }

    if plan.steps.is_empty() {
        return nothing_to_do(stdout, stderr, &envelope, &plan, flags.flag("json"));
    }

    // Execution lands with milestone M3. Saying so is better than printing a
    // success line for a repair that did not run.
    let error = Error::new(
        Code::Unsupported,
        "o824 repair: applying a plan arrives with milestone M3; run with --dry-run to \
         see the plan this build would execute",
    );
    let _ = writeln!(stderr, "{error}");
    exit_code_of(Some(&error))
}

/// Reports a plan with no steps.
///
/// A clean installation exits 0; findings that have no automatic repair exit 1, so
/// a script never mistakes "nothing I can do" for "fixed".
fn nothing_to_do(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    envelope: &Envelope,
    plan: &o824_plan::Plan,
    json: bool,
) -> i32 {
    if json {
        let document = serde_json::json!({
            "planId": plan.id,
            "installId": plan.install_id,
            "root": plan.root,
            "applied": false,
            "steps": [],
            "findings": envelope.findings,
        });
        return write_document(stdout, stderr, &document);
    }
    if envelope.findings.is_empty() {
        let _ = writeln!(stdout, "nothing to repair: no findings");
        return EXIT_OK;
    }
    let _ = writeln!(
        stdout,
        "no automatic repair for {} finding(s):",
        envelope.findings.len()
    );
    for finding in &envelope.findings {
        let _ = writeln!(stdout, "  {}", crate::render::format_finding(finding));
        let hint = repair_hint(finding);
        if !hint.is_empty() {
            let _ = writeln!(stdout, "    {hint}");
        }
    }
    EXIT_FINDINGS
}

/// Explains what the user can do about a finding the kernel will not repair
/// automatically.
fn repair_hint(finding: &o824_model::Finding) -> &'static str {
    match finding.code {
        Code::LinkBroken => "the link points at a directory that is not a plugin bundle; reinstall the plugin, or run repair --with-install to reinstall the profile's dependencies",
        Code::DupLoader => "this patch row names an id the loader does not know; DSH only warns about it - check whether it is a leftover before deleting it",
        Code::BundleMissing => "the bundle this link points at is gone; reinstall it (P1 will rebuild the plugin closure automatically), or run repair --with-install now",
        Code::ProfileParse => "restore this profile file from a snapshot first (o824 snapshot create --install <id>), then repair",
        Code::BridgeDangling => "the bridge cannot be rebuilt from the profile manifest; reinstall the plugin",
        Code::HalfInstall => "an install was interrupted; roll it back or reinstall it",
        _ => "",
    }
}

/// Renders the plan a `--dry-run` would apply.
pub fn write_plan(plan: &o824_plan::Plan) -> String {
    let mut text = String::new();
    text.push_str(&format!("plan {}  install {}\n", plan.id, plan.install_id));
    text.push_str(&format!("root: {}\n", plan.root));
    text.push_str(&format!(
        "expires: {}\n",
        plan.expires_at.to_rfc3339_seconds()
    ));
    if plan.steps.is_empty() {
        text.push_str("steps: none\n");
        return text;
    }
    text.push_str(&format!("steps: {}\n", plan.steps.len()));
    for step in &plan.steps {
        text.push_str(&format!(
            "  {}  {}{}\n",
            step.kind,
            step.path,
            step_detail(step)
        ));
    }
    text
}

/// Renders a step's optional inputs for the human plan.
fn step_detail(step: &o824_plan::Step) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !step.from.is_empty() {
        parts.push(format!("from={}", step.from));
    }
    // An install runs the host's own pnpm from the application root, which is the
    // one input a user cannot infer from the step's path.
    if step.kind == o824_plan::KIND_REINSTALL_PROFILE && !step.to.is_empty() {
        parts.push(format!("to={}", step.to));
    }
    if !step.args.is_empty() {
        parts.push(format!("args={}", step.args.join(",")));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  {}", parts.join(" "))
    }
}

fn write_json(stdout: &mut dyn Write, stderr: &mut dyn Write, value: &o824_plan::Plan) -> i32 {
    match serde_json::to_value(value) {
        Ok(document) => write_document(stdout, stderr, &document),
        Err(error) => {
            let error = Error::new(
                Code::Internal,
                format!("o824 repair: cannot encode the plan: {error}"),
            );
            let _ = writeln!(stderr, "{error}");
            exit_code_of(Some(&error))
        }
    }
}

/// Prints one JSON document and nothing else.
fn write_document(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    value: &serde_json::Value,
) -> i32 {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned());
    if let Err(error) = writeln!(stdout, "{text}") {
        let error = Error::new(
            Code::Internal,
            format!("o824 repair: cannot write the result: {error}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    EXIT_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_fixtures::TempDir;

    fn run_capture(args: &[&str], data_root: &std::path::Path) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_with(&args, &mut stdout, &mut stderr, Some(data_root));
        (
            code,
            String::from_utf8(stdout).expect("utf8"),
            String::from_utf8(stderr).expect("utf8"),
        )
    }

    #[test]
    fn a_dry_run_on_a_clean_tree_prints_a_plan_with_no_steps() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = TempDir::new("o824-repair-").expect("store");
        let root = fixture.root().display().to_string();
        let (code, stdout, stderr) = run_capture(
            &["--root", &root, "--home", &root, "--dry-run"],
            store.path(),
        );
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("steps: none"), "{stdout}");
        assert!(stdout.starts_with("plan plan-"), "{stdout}");
    }

    #[test]
    fn a_dry_run_on_the_git_dep_tree_prints_the_allow_builds_step() {
        let fixture = o824_fixtures::build("git-dep").expect("fixture");
        let store = TempDir::new("o824-repair-").expect("store");
        let root = fixture.root().display().to_string();
        let (code, stdout, stderr) = run_capture(
            &["--root", &root, "--home", &root, "--dry-run", "--json"],
            store.path(),
        );
        assert_eq!(code, 0, "{stderr}");
        let plan: serde_json::Value = serde_json::from_str(&stdout).expect("json");
        assert_eq!(plan["steps"][0]["kind"], "allow-builds");
        assert_eq!(
            plan["steps"][0]["args"][0],
            "dshmarket@git+https://github.com/deepseek-ai/dshmarket.git"
        );
    }

    #[test]
    fn applying_a_plan_is_not_reachable_before_m3() {
        let fixture = o824_fixtures::build("git-dep").expect("fixture");
        let store = TempDir::new("o824-repair-").expect("store");
        let root = fixture.root().display().to_string();
        let (code, _, stderr) = run_capture(&["--root", &root, "--home", &root], store.path());
        assert_eq!(code, crate::EXIT_INTERNAL);
        assert!(stderr.contains("milestone M3"), "{stderr}");
    }

    #[test]
    fn a_tree_with_no_automatic_repair_exits_one() {
        let fixture = o824_fixtures::build("half-install").expect("fixture");
        let root = fixture.root().display().to_string();
        // half-install's journal is under the fixture root, which is also what
        // this test passes as the data root.
        let (code, stdout, _) = run_capture(&["--root", &root, "--home", &root], fixture.root());
        assert_eq!(code, EXIT_FINDINGS);
        assert!(stdout.contains("no automatic repair"), "{stdout}");
    }
}
