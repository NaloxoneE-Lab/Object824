//! A real golden file must survive a round trip through the Rust model:
//! deserialise it into `Envelope`, serialise it again, and get the same bytes.
//!
//! This is the test that catches a wrong field name, a wrong field order, or a
//! lost `preserve_order` — all three are invisible until a golden goes red.

use o824_conformance::{golden_file, load, KINDS};
use o824_model::Envelope;

#[test]
fn every_doctor_golden_round_trips_byte_for_byte() {
    for kind in KINDS {
        let path = golden_file(kind, "doctor");
        let golden = load(&path);

        let envelope: Envelope = serde_json::from_str(&golden)
            .unwrap_or_else(|error| panic!("{} does not deserialise: {error}", path.display()));

        let rendered = format!(
            "{}\n",
            serde_json::to_string_pretty(&envelope).expect("serialize")
        );
        assert_eq!(
            rendered, golden,
            "{kind}.doctor.json changed after a round trip"
        );
    }
}

#[test]
fn the_envelope_identifier_is_the_documented_one() {
    let envelope: Envelope =
        serde_json::from_str(&load(&golden_file("ok", "doctor"))).expect("parse ok doctor");
    assert_eq!(envelope.envelope, o824_model::ENVELOPE_ID);
    assert!(envelope.findings.is_empty(), "the ok tree is clean");
}

#[test]
fn a_finding_carries_its_optional_fields_in_contract_order() {
    let envelope: Envelope =
        serde_json::from_str(&load(&golden_file("git-dep", "doctor"))).expect("parse");
    let finding = envelope.findings.first().expect("one finding");

    let value = serde_json::to_value(finding).expect("serialize");
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "checkId",
            "severity",
            "code",
            "category",
            "params",
            "fallbackMessage",
            "repair"
        ],
        "detail is empty in this finding and must be omitted"
    );
}
