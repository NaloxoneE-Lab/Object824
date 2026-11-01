//! The shared data model: the findings envelope, individual findings, and the
//! install record.
//!
//! The JSON field names are a contract with third-party UIs and with the golden
//! files in `spec/golden`, so they must not change. Two rules matter more than
//! the rest:
//!
//! * Field order follows the reference implementation, which is why the whole
//!   workspace enables `serde_json`'s `preserve_order`.
//! * A collection is never `null`. Go's nil slice marshals to `null`, and the
//!   golden files are a cross-language contract; the Rust side normalises at
//!   construction time so the zero-value habits of either language cannot leak
//!   into the wire format.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use o824_errs::{Code, Error};

pub mod time;

pub use time::Timestamp;

/// The versioned envelope identifier returned by diagnosis.
pub const ENVELOPE_ID: &str = "dsh-doctor/v1";

/// Lexical path helpers shared by every crate that reports a path.
///
/// They live in the shared model crate because every layer needs them and the
/// model is the only crate that sits below all of them; they are pure and pull
/// in no dependency.
pub mod path {
    use std::ffi::OsString;
    use std::path::{Component, Path, PathBuf};

    /// Normalises a path the way the reference implementation's `filepath.Clean`
    /// does for the shapes this kernel produces: `.` components are dropped,
    /// `..` pops one component, redundant separators disappear, and an empty
    /// result is `.`.
    ///
    /// This is lexical, not filesystem-aware: `..` pops even when the component
    /// it removes is a link target. That matches `filepath.Clean`, which is the
    /// contract for every path a finding reports.
    pub fn clean(path: &Path) -> PathBuf {
        let mut cleaned = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    if !cleaned.pop() {
                        cleaned.push(OsString::from(".."));
                    }
                }
                other => cleaned.push(other.as_os_str()),
            }
        }
        if cleaned.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            cleaned
        }
    }

    /// Turns a path Windows reports in the object namespace back into the Win32
    /// spelling, so it can be compared with an installation root.
    ///
    /// `\??\Global\` is how a secondary volume's drive letter appears inside the
    /// object namespace; the junction still resolves exactly like `\??\D:\`, but
    /// leaving the prefix in place makes a reported path opaque.
    pub fn strip_nt_namespace(path: &Path) -> PathBuf {
        let mut value = path.display().to_string();
        for prefix in ["\\\\?\\", "\\??\\"] {
            if let Some(rest) = value.strip_prefix(prefix) {
                value = rest.to_owned();
            }
        }
        if let Some(rest) = value.strip_prefix("Global\\") {
            value = rest.to_owned();
        }
        PathBuf::from(value)
    }
}

/// A finding's severity level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The installation is broken and needs repair.
    Error,
    /// Something looks wrong but DSH still runs.
    Warning,
    /// Informational only.
    Info,
}

impl Severity {
    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// Groups findings by the part of the installation they concern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    /// The installation tree itself.
    Install,
    /// A profile or loader configuration file.
    Config,
    /// A plugin or bundle.
    Plugin,
    /// Runtime data.
    Data,
    /// The process environment.
    Env,
    /// A security-relevant condition.
    Security,
    /// Network reachability.
    Network,
    /// Filesystem permissions.
    Permission,
    /// An internal kernel inconsistency.
    Internal,
}

impl Category {
    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Category::Install => "install",
            Category::Config => "config",
            Category::Plugin => "plugin",
            Category::Data => "data",
            Category::Env => "env",
            Category::Security => "security",
            Category::Network => "network",
            Category::Permission => "permission",
            Category::Internal => "internal",
        }
    }
}

/// How a finding can be repaired. `kind` names the repair operation; `plan_hint`
/// is human-readable guidance for the plan step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repair {
    /// The repair operation name.
    pub kind: String,
    /// The plan step a caller should build.
    pub plan_hint: String,
}

/// One diagnostic result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// "F01" … "F20".
    pub check_id: String,
    /// How bad it is.
    pub severity: Severity,
    /// The stable error code.
    pub code: Code,
    /// Which part of the installation it concerns.
    pub category: Category,
    /// Values a UI renders into a localized message; omitted when empty.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
    /// Untranslated English text; omitted when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fallback_message: String,
    /// Extra machine-readable context; omitted when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// The repair, when one exists; omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<Repair>,
}

impl Finding {
    /// Creates a finding with no optional fields.
    pub fn new(
        check_id: impl Into<String>,
        severity: Severity,
        code: Code,
        category: Category,
    ) -> Finding {
        Finding {
            check_id: check_id.into(),
            severity,
            code,
            category,
            params: Map::new(),
            fallback_message: String::new(),
            detail: String::new(),
            repair: None,
        }
    }

    /// Sets the rendering parameters.
    pub fn with_params(mut self, params: Map<String, Value>) -> Finding {
        self.params = params;
        self
    }

    /// Sets the English fallback message.
    pub fn with_fallback_message(mut self, message: impl Into<String>) -> Finding {
        self.fallback_message = message.into();
        self
    }

    /// Sets the detail text.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Finding {
        self.detail = detail.into();
        self
    }

    /// Sets the repair.
    pub fn with_repair(mut self, repair: Repair) -> Finding {
        self.repair = Some(repair);
        self
    }
}

/// The versioned top-level JSON document returned by diagnosis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// Always [`ENVELOPE_ID`].
    pub envelope: String,
    /// The findings; never `null`, empty renders as `[]`.
    #[serde(default)]
    pub findings: Vec<Finding>,
}

impl Envelope {
    /// Wraps findings in a versioned envelope.
    ///
    /// The reference implementation normalises a nil slice to an empty one so
    /// JSON renders `[]`; in Rust the empty vector already renders `[]`, and
    /// this constructor is the only sanctioned way to build an envelope, so the
    /// rule holds by construction.
    pub fn new(findings: Vec<Finding>) -> Envelope {
        Envelope {
            envelope: ENVELOPE_ID.to_owned(),
            findings,
        }
    }
}

impl Default for Envelope {
    fn default() -> Envelope {
        Envelope::new(Vec::new())
    }
}

/// One discovered DeepSeek Harness installation.
///
/// `health` is one of `"ok"`, `"degraded"`, or `"damaged"`. `home` is the
/// resolved DSH home that owns the profile tree and is omitted when unknown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Install {
    /// The installation identifier.
    pub install_id: String,
    /// The installation shape; `"desktop"` is the only one P0 knows.
    pub kind: String,
    /// The installation root.
    pub root: String,
    /// The DSH version read from the installation.
    pub version: String,
    /// The profiles under the DSH home; always an array.
    #[serde(default)]
    pub profiles: Vec<String>,
    /// `"ok"`, `"degraded"`, or `"damaged"`.
    pub health: String,
    /// The DSH home; omitted when unknown.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub home: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_finding() -> Finding {
        Finding::new(
            "F17",
            Severity::Error,
            Code::PnpmBuildBlocked,
            Category::Plugin,
        )
    }

    #[test]
    fn envelope_shape() {
        let envelope = Envelope::new(vec![sample_finding()]);
        let value = serde_json::to_value(&envelope).expect("serialize");
        assert_eq!(value["envelope"], json!("dsh-doctor/v1"));
        assert!(value["findings"].is_array());
        // Key order is part of the contract: serde_json must preserve it.
        let object = value.as_object().expect("object");
        let keys: Vec<&str> = object.keys().map(String::as_str).collect();
        assert_eq!(keys, ["envelope", "findings"]);
    }

    #[test]
    fn new_envelope_normalises_nil() {
        let value = serde_json::to_value(Envelope::new(Vec::new())).expect("serialize");
        assert_eq!(value["findings"], json!([]));
        assert!(!value.to_string().contains("null"));

        // A missing `findings` key deserialises to an empty vector and comes
        // back out as `[]`, never as an absent or null field.
        let restored: Envelope =
            serde_json::from_str(r#"{"envelope":"dsh-doctor/v1"}"#).expect("deserialize");
        assert_eq!(restored.findings, Vec::<Finding>::new());
        assert_eq!(
            serde_json::to_value(&restored).expect("serialize")["findings"],
            json!([])
        );
    }

    #[test]
    fn finding_omits_empty_detail() {
        let value = serde_json::to_value(sample_finding()).expect("serialize");
        let object = value.as_object().expect("object");
        assert!(!object.contains_key("params"));
        assert!(!object.contains_key("fallbackMessage"));
        assert!(!object.contains_key("detail"));
        assert!(!object.contains_key("repair"));
    }

    #[test]
    fn finding_json_contract() {
        let finding = sample_finding()
            .with_params(Map::from_iter([("package".to_owned(), json!("dshmarket"))]))
            .with_fallback_message("pnpm build scripts are not allowed for this dependency")
            .with_detail("detail")
            .with_repair(Repair {
                kind: "auto".to_owned(),
                plan_hint: "allow-builds".to_owned(),
            });

        let value = serde_json::to_value(&finding).expect("serialize");
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
                "detail",
                "repair"
            ]
        );
        assert_eq!(value["checkId"], json!("F17"));
        assert_eq!(value["severity"], json!("error"));
        assert_eq!(value["code"], json!("E_PNPM_BUILD_BLOCKED"));
        assert_eq!(value["category"], json!("plugin"));
        assert_eq!(value["repair"]["kind"], json!("auto"));
        assert_eq!(value["repair"]["planHint"], json!("allow-builds"));
    }

    #[test]
    fn category_and_severity_cover_nine_and_three_values() {
        let categories = [
            Category::Install,
            Category::Config,
            Category::Plugin,
            Category::Data,
            Category::Env,
            Category::Security,
            Category::Network,
            Category::Permission,
            Category::Internal,
        ];
        let spelled: Vec<&str> = categories.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            spelled,
            [
                "install",
                "config",
                "plugin",
                "data",
                "env",
                "security",
                "network",
                "permission",
                "internal"
            ]
        );

        let severities = [Severity::Error, Severity::Warning, Severity::Info];
        let spelled: Vec<&str> = severities.iter().map(|s| s.as_str()).collect();
        assert_eq!(spelled, ["error", "warning", "info"]);
    }

    #[test]
    fn install_json_contract() {
        let install = Install {
            install_id: "i1".to_owned(),
            kind: "desktop".to_owned(),
            root: "C:\\harness".to_owned(),
            version: "0.2.1-rc.3".to_owned(),
            profiles: vec!["desktop".to_owned()],
            health: "ok".to_owned(),
            home: "C:\\home".to_owned(),
        };
        let value = serde_json::to_value(&install).expect("serialize");
        let keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "installId",
                "kind",
                "root",
                "version",
                "profiles",
                "health",
                "home"
            ]
        );
        assert_eq!(value["installId"], json!("i1"));
        assert!(value["profiles"].is_array());

        // `home` is omitted when unknown, and `profiles` is still an array.
        let anonymous = Install {
            home: String::new(),
            profiles: Vec::new(),
            ..install
        };
        let value = serde_json::to_value(&anonymous).expect("serialize");
        assert!(!value.as_object().expect("object").contains_key("home"));
        assert_eq!(value["profiles"], json!([]));
    }
}
