//! Human-facing rendering of findings.
//!
//! One finding is one line: `<checkId>  <severity>  <code>  <params>`, with the
//! parameters sorted by key so map iteration never leaks into the output. A
//! clean installation prints `no findings` instead.

use std::io::{self, Write};

use o824_model::Finding;
use serde_json::Value;

/// Separates the fixed fields of a human finding line, and separates them from
/// the params group. Two spaces keep the columns readable while staying
/// trivially parseable.
pub const FINDING_FIELD_SEPARATOR: &str = "  ";

/// Writes one line per finding, or the clean line when there is nothing to
/// report.
pub fn write_findings(out: &mut dyn Write, findings: &[Finding]) -> io::Result<()> {
    if findings.is_empty() {
        return writeln!(out, "no findings");
    }
    for finding in findings {
        writeln!(out, "{}", format_finding(finding))?;
    }
    Ok(())
}

/// Renders one finding as `<checkId>  <severity>  <code>  <params>`.
pub fn format_finding(finding: &Finding) -> String {
    let mut fields = vec![
        finding.check_id.clone(),
        finding.severity.as_str().to_owned(),
        finding.code.as_str().to_owned(),
    ];
    let params = format_params(finding);
    if !params.is_empty() {
        fields.push(params);
    }
    fields.join(FINDING_FIELD_SEPARATOR)
}

/// Renders finding parameters as space-separated `key=value` pairs in ascending
/// key order.
pub fn format_params(finding: &Finding) -> String {
    let mut keys: Vec<&String> = finding.params.keys().collect();
    keys.sort();
    keys.into_iter()
        .map(|key| format!("{key}={}", format_value(&finding.params[key])))
        .collect::<Vec<String>>()
        .join(" ")
}

/// Renders one parameter value the way the reference implementation's `%v` does
/// for the shapes findings carry.
fn format_value(value: &Value) -> String {
    match value {
        Value::Null => "<nil>".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(format_value).collect();
            format!("[{}]", rendered.join(" "))
        }
        Value::Object(_) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_model::{Category, Code, Finding, Repair, Severity};
    use serde_json::{json, Map, Value};

    fn params(pairs: [(&str, Value); 2]) -> Map<String, Value> {
        let mut sorted = pairs.to_vec();
        sorted.sort_by(|left, right| left.0.cmp(right.0));
        sorted
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect()
    }

    fn finding() -> Finding {
        Finding::new("F03", Severity::Warning, Code::DupLoader, Category::Config).with_params(
            params([("ids", json!(["dsh-echo"])), ("duplicates", json!(1))]),
        )
    }

    #[test]
    fn a_finding_renders_in_contract_order() {
        assert_eq!(
            format_finding(&finding()),
            "F03  warning  E_DUP_LOADER  duplicates=1 ids=[dsh-echo]"
        );
    }

    #[test]
    fn a_clean_diagnosis_says_so() {
        let mut out: Vec<u8> = Vec::new();
        write_findings(&mut out, &[]).expect("write");
        assert_eq!(String::from_utf8(out).expect("utf8"), "no findings\n");
    }

    #[test]
    fn repair_is_not_part_of_the_human_line() {
        let finding = finding().with_repair(Repair {
            kind: "auto".to_owned(),
            plan_hint: "allow-builds".to_owned(),
        });
        assert!(!format_finding(&finding).contains("allow-builds"));
    }
}
