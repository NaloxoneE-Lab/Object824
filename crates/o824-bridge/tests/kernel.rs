//! The bridge against the real kernel binary: one process, frames, jobs, and a close that
//! is honest.
//!
//! These tests spawn `o824 kernel --stdio` the way the GUI does. A stub would let the
//! bridge agree with itself; the point is agreeing with the thing it will talk to.

use std::path::PathBuf;

use o824_bridge::{Bridge, BridgeError, Options};

/// The kernel binary this run built.
///
/// `CARGO_BIN_EXE_*` only exists for the crate that defines the binary, so the path is
/// derived from the test executable: `<target>/debug/deps/<test>.exe` → `<target>/debug/o824.exe`.
fn binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("current exe");
    path.pop(); // the test binary's name
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(format!("o824{}", std::env::consts::EXE_SUFFIX));
    path
}

fn options_for_fixture(fixture: &o824_fixtures::Fixture) -> Options {
    Options {
        program: Some(binary()),
        root: Some(fixture.root().to_path_buf()),
        home: Some(fixture.root().to_path_buf()),
        data_root: Some(fixture.root().join("data")),
    }
}

#[test]
fn the_handshake_names_the_protocol_and_its_capabilities() {
    let fixture = o824_fixtures::build("ok").expect("fixture");
    let mut bridge = Bridge::start(&options_for_fixture(&fixture)).expect("start");
    let info = bridge.info().expect("info");
    assert!(
        info.get("protocolVersion")
            .and_then(|v| v.as_str())
            .is_some(),
        "{info}"
    );
    let capabilities = info
        .get("capabilities")
        .and_then(|v| v.as_array())
        .expect("capabilities");
    assert!(capabilities.iter().any(|c| c == "doctor"), "{info}");
    assert!(bridge.close(), "the kernel exits on EOF");
}

#[test]
fn discover_returns_the_fixture_installation() {
    let fixture = o824_fixtures::build("ok").expect("fixture");
    let mut bridge = Bridge::start(&options_for_fixture(&fixture)).expect("start");
    let installs = bridge.discover().expect("discover");
    assert_eq!(installs.len(), 1, "{installs:?}");
    assert_eq!(installs[0].kind, "desktop");
    assert!(bridge.close());
}

/// A doctor is a job: its answer arrives as events, and a client that read only the reply
/// would hang. The bridge waits for the terminal state and returns the envelope.
#[test]
fn a_doctor_job_returns_its_envelope() {
    let fixture = o824_fixtures::build("broken-link").expect("fixture");
    let mut bridge = Bridge::start(&options_for_fixture(&fixture)).expect("start");
    let envelope = match bridge.doctor("i1") {
        Ok(envelope) => envelope,
        Err(error) => {
            panic!(
                "doctor failed: {error}; kernel stderr: {}",
                bridge.stderr_text()
            );
        }
    };
    let findings = envelope
        .get("findings")
        .and_then(|v| v.as_array())
        .expect("findings");
    assert!(!findings.is_empty(), "{envelope}");
    assert!(bridge.close());
}

/// §21.5: `start` is a plan action, and the bridge carries the plan back whole.
#[test]
fn a_start_plan_arrives_with_its_step() {
    let fixture = o824_fixtures::build("ok").expect("fixture");
    let mut bridge = Bridge::start(&options_for_fixture(&fixture)).expect("start");
    let plan = bridge.plan("i1", "start").expect("a start plan");
    let steps = plan["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1, "{plan}");
    assert_eq!(steps[0]["kind"], "start-desktop", "{plan}");
    assert!(bridge.close());
}

/// An action the kernel does not know comes back as an error with a code, not silence.
#[test]
fn an_unknown_action_is_an_error_with_a_code() {
    let fixture = o824_fixtures::build("ok").expect("fixture");
    let mut bridge = Bridge::start(&options_for_fixture(&fixture)).expect("start");
    let error = bridge.plan("i1", "levitate").expect_err("must fail");
    match error {
        BridgeError::Kernel { code, .. } => assert!(code.starts_with("E_"), "{code}"),
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(bridge.close());
}

/// A kernel that is asked to do something it does not know answers with an error, and the
/// bridge turns that into a value with a code rather than a panic.
#[test]
fn an_unknown_install_id_is_an_error_with_a_code() {
    let fixture = o824_fixtures::build("ok").expect("fixture");
    let mut bridge = Bridge::start(&options_for_fixture(&fixture)).expect("start");
    let error = bridge.doctor("i-does-not-exist").expect_err("must fail");
    assert_eq!(o824_bridge::exit_code(&error), 8);
    match error {
        BridgeError::Kernel { code, .. } => assert!(code.starts_with("E_"), "{code}"),
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(bridge.close());
}

/// The kernel must not outlive the window: closing the pipe has to end the process, and
/// `close` reports whether it actually did.
#[test]
fn closing_the_pipe_ends_the_kernel() {
    let fixture = o824_fixtures::build("ok").expect("fixture");
    let bridge = Bridge::start(&options_for_fixture(&fixture)).expect("start");
    assert!(bridge.close(), "the kernel exits when its stdin closes");
}

/// Talking to a program that is not the kernel fails at spawn time, with a message that
/// says which program.
#[test]
fn a_missing_kernel_is_reported_as_such() {
    let options = Options {
        program: Some(PathBuf::from("this-kernel-does-not-exist")),
        ..Options::default()
    };
    let error = Bridge::start(&options).expect_err("must fail");
    assert!(
        error.to_string().contains("cannot start the kernel"),
        "{error}"
    );
}
