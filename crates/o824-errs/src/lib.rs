//! Stable, machine-readable error codes and the kernel error type.
//!
//! The string form of a [`Code`] is part of the CLI contract: findings
//! envelopes and exit codes depend on it, so a code is never renamed once it
//! has shipped. The exit-code mapping lives in `o824-cli`, not here, because
//! the kernel itself has no opinion about process status.

#![forbid(unsafe_code)]

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A stable, machine-readable error identifier.
///
/// The `as_str` spelling is the wire form and must not change; see
/// `docs/week1/04-first-crates.md` for the authoritative list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Code {
    /// The command line or its arguments are wrong.
    Usage,
    /// A filesystem operation was refused.
    Permission,
    /// The on-disk state drifted from what the kernel recorded.
    StateDrift,
    /// A network fetch failed.
    Network,
    /// Another process holds the kernel lock.
    Busy,
    /// An unexpected kernel failure.
    Internal,
    /// A path is not a usable DeepSeek Harness installation tree.
    InstallDir,
    /// A profile `node_modules` link does not resolve to a package, although
    /// its target still exists.
    LinkBroken,
    /// A profile `node_modules` link target no longer exists.
    BundleMissing,
    /// A plugin's dependency bridge into the profile closure is missing.
    BridgeDangling,
    /// A loader entry id is named by both the bundle list and the patch file.
    /// Advisory only: on 0.2.1-rc.3 an unknown patch id only warns, so this
    /// must never drive an automatic edit.
    DupLoader,
    /// A profile file is not parseable, or its entries have the wrong shape.
    ProfileParse,
    /// A dependency pnpm fetches from source is not on the build whitelist.
    PnpmBuildBlocked,
    /// A journal records an install that is neither done nor failed.
    HalfInstall,
    /// Two installed forms of DSH are at different versions while sharing one home.
    VersionDrift,
    /// A mutating command could not obtain the confirmation it needs.
    ConfirmationRequired,
    /// A repair that must copy a dependency tree found too little free space.
    DiskFull,
    /// An apply named a plan this session never built.
    PlanUnknown,
    /// An apply named a plan older than the fifteen minutes §19.2 allows.
    PlanExpired,
    /// A stdio frame whose body is not JSON.
    ProtocolParse,
    /// A JSON frame whose envelope is not a JSON-RPC 2.0 request.
    ProtocolInvalid,
    /// A client whose protocol major version differs from the kernel's.
    ProtocolMismatch,
    /// A method the kernel does not implement.
    Unsupported,
    /// The transcript mock failed as instructed by `--fail-at`.
    MockFailure,
}

impl Code {
    /// Every code, in the order the specification lists them.
    pub const ALL: &'static [Code] = &[
        Code::Usage,
        Code::Permission,
        Code::StateDrift,
        Code::Network,
        Code::Busy,
        Code::Internal,
        Code::InstallDir,
        Code::LinkBroken,
        Code::BundleMissing,
        Code::BridgeDangling,
        Code::DupLoader,
        Code::ProfileParse,
        Code::PnpmBuildBlocked,
        Code::HalfInstall,
        Code::VersionDrift,
        Code::ConfirmationRequired,
        Code::DiskFull,
        Code::PlanUnknown,
        Code::PlanExpired,
        Code::ProtocolParse,
        Code::ProtocolInvalid,
        Code::ProtocolMismatch,
        Code::Unsupported,
        Code::MockFailure,
    ];

    /// The wire spelling of the code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Code::Usage => "E_USAGE",
            Code::Permission => "E_PERMISSION",
            Code::StateDrift => "E_STATE_DRIFT",
            Code::Network => "E_NETWORK",
            Code::Busy => "E_BUSY",
            Code::Internal => "E_INTERNAL",
            Code::InstallDir => "E_INSTALL_DIR",
            Code::LinkBroken => "E_LINK_BROKEN",
            Code::BundleMissing => "E_BUNDLE_MISSING",
            Code::BridgeDangling => "E_BRIDGE_DANGLING",
            Code::DupLoader => "E_DUP_LOADER",
            Code::ProfileParse => "E_PROFILE_PARSE",
            Code::PnpmBuildBlocked => "E_PNPM_BUILD_BLOCKED",
            Code::HalfInstall => "E_HALF_INSTALL",
            Code::VersionDrift => "E_VERSION_DRIFT",
            Code::ConfirmationRequired => "E_CONFIRMATION_REQUIRED",
            Code::DiskFull => "E_DISK_FULL",
            Code::PlanUnknown => "E_PLAN_UNKNOWN",
            Code::PlanExpired => "E_PLAN_EXPIRED",
            Code::ProtocolParse => "E_PROTOCOL_PARSE",
            Code::ProtocolInvalid => "E_PROTOCOL_INVALID",
            Code::ProtocolMismatch => "E_PROTOCOL_MISMATCH",
            Code::Unsupported => "E_UNSUPPORTED",
            Code::MockFailure => "E_MOCK_FAILURE",
        }
    }

    /// Parses a wire spelling. Unknown spellings yield `None`: a code the
    /// kernel does not know is a protocol error, not a finding.
    pub fn parse(text: &str) -> Option<Code> {
        Code::ALL.iter().copied().find(|code| code.as_str() == text)
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Code {
    type Err = UnknownCode;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Code::parse(text).ok_or_else(|| UnknownCode(text.to_owned()))
    }
}

/// The error `Code::from_str` reports for a spelling the kernel does not know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownCode(pub String);

impl fmt::Display for UnknownCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown o824 error code {:?}", self.0)
    }
}

impl std::error::Error for UnknownCode {}

// Serialise as the wire spelling, never as the variant name, and reject an
// unknown spelling rather than silently inventing a code.
impl Serialize for Code {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Code {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CodeVisitor;

        impl Visitor<'_> for CodeVisitor {
            type Value = Code;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an o824 error code such as \"E_USAGE\"")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Code, E> {
                Code::parse(value).ok_or_else(|| E::custom(UnknownCode(value.to_owned())))
            }
        }

        deserializer.deserialize_str(CodeVisitor)
    }
}

/// A kernel error: a stable code, a short English fallback message, and the
/// parameters a caller may use to render a localized message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The stable identifier.
    pub code: Code,
    /// Untranslated English text; may be empty.
    pub fallback: String,
    /// Values a UI can use to render a localized message.
    pub params: serde_json::Map<String, serde_json::Value>,
}

impl Error {
    /// Creates an error with no parameters.
    pub fn new(code: Code, fallback: impl Into<String>) -> Error {
        Error {
            code,
            fallback: fallback.into(),
            params: serde_json::Map::new(),
        }
    }

    /// Adds one rendering parameter.
    pub fn with_param(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Error {
        self.params.insert(key.into(), value.into());
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.fallback.is_empty() {
            f.write_str(self.code.as_str())
        } else {
            f.write_str(&self.fallback)
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire spellings are the protocol; a rename here breaks every client.
    #[test]
    fn code_strings_are_exact() {
        let expected = [
            "E_USAGE",
            "E_PERMISSION",
            "E_STATE_DRIFT",
            "E_NETWORK",
            "E_BUSY",
            "E_INTERNAL",
            "E_INSTALL_DIR",
            "E_LINK_BROKEN",
            "E_BUNDLE_MISSING",
            "E_BRIDGE_DANGLING",
            "E_DUP_LOADER",
            "E_PROFILE_PARSE",
            "E_PNPM_BUILD_BLOCKED",
            "E_HALF_INSTALL",
            "E_VERSION_DRIFT",
            "E_CONFIRMATION_REQUIRED",
            "E_DISK_FULL",
            "E_PLAN_UNKNOWN",
            "E_PLAN_EXPIRED",
            "E_PROTOCOL_PARSE",
            "E_PROTOCOL_INVALID",
            "E_PROTOCOL_MISMATCH",
            "E_UNSUPPORTED",
            "E_MOCK_FAILURE",
        ];
        let actual: Vec<&str> = Code::ALL.iter().map(|code| code.as_str()).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn code_round_trips_through_json() {
        for code in Code::ALL {
            let json = serde_json::to_string(code).expect("serialize");
            assert_eq!(json, format!("\"{}\"", code.as_str()));
            let back: Code = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, *code);
        }
    }

    #[test]
    fn unknown_code_is_rejected() {
        assert_eq!(Code::parse("E_NOPE"), None);
        assert!("E_NOPE".parse::<Code>().is_err());
        assert!(serde_json::from_str::<Code>("\"E_NOPE\"").is_err());
    }

    #[test]
    fn display_prefers_fallback_then_code() {
        let with_fallback = Error::new(Code::Usage, "a root is required");
        assert_eq!(with_fallback.to_string(), "a root is required");

        let without_fallback = Error::new(Code::Busy, "");
        assert_eq!(without_fallback.to_string(), "E_BUSY");
    }

    #[test]
    fn params_are_carried_verbatim() {
        let error = Error::new(Code::InstallDir, "not an install")
            .with_param("root", "C:\\x")
            .with_param("missing", serde_json::json!(["DeepSeek Harness.exe"]));
        assert_eq!(error.params["root"], serde_json::json!("C:\\x"));
        assert_eq!(
            error.params["missing"],
            serde_json::json!(["DeepSeek Harness.exe"])
        );
    }
}
