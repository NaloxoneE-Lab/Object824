//! The build identity of the `o824` binary.
//!
//! A release build injects the real values with `O824_VERSION`, `O824_COMMIT`,
//! and `O824_BUILD_TIME`; an ordinary `cargo build` keeps the development
//! defaults below. Nothing here reads the environment at run time or consults
//! the clock, so the reported identity is exactly what was compiled in.

#![forbid(unsafe_code)]

/// The release version, or the development marker.
pub const VERSION: &str = match option_env!("O824_VERSION") {
    Some(version) => version,
    None => "0.1.0-dev",
};

/// The source revision the binary was built from; empty unless injected.
pub const COMMIT: &str = match option_env!("O824_COMMIT") {
    Some(commit) => commit,
    None => "",
};

/// When the binary was built, ideally RFC 3339; empty unless injected.
pub const BUILD_TIME: &str = match option_env!("O824_BUILD_TIME") {
    Some(built) => built,
    None => "",
};

/// Renders the one-line build identity:
///
/// ```text
/// o824 <version>[ (commit <commit>, built <time>)] <os>/<arch>
/// ```
///
/// The parenthetical is included only for the values that were injected, so a
/// plain `cargo build` prints `o824 0.1.0-dev windows/amd64` rather than empty
/// parentheses. The platform suffix is always present: it is the first thing a
/// bug report needs and costs nothing.
pub fn string() -> String {
    let mut rendered = String::from("o824 ");
    rendered.push_str(version_text());

    let mut parts: Vec<String> = Vec::with_capacity(2);
    let commit = trim(COMMIT);
    if !commit.is_empty() {
        parts.push(format!("commit {commit}"));
    }
    let built = trim(BUILD_TIME);
    if !built.is_empty() {
        parts.push(format!("built {built}"));
    }
    if !parts.is_empty() {
        rendered.push_str(" (");
        rendered.push_str(&parts.join(", "));
        rendered.push(')');
    }

    rendered.push(' ');
    rendered.push_str(std::env::consts::OS);
    rendered.push('/');
    rendered.push_str(arch());

    rendered
}

/// Trims the whitespace an injected value may carry. An injected but empty
/// string means "not set", which is what an injection of an empty value does.
fn trim(value: &str) -> &str {
    value.trim()
}

/// Returns the version to print, falling back to the development marker when an
/// injection left it empty: `o824  windows/amd64` would read as a formatting bug
/// rather than as a development build.
fn version_text() -> &'static str {
    let version = trim(VERSION);
    if version.is_empty() {
        "0.1.0-dev"
    } else {
        version
    }
}

/// The architecture spelling used in the platform suffix.
///
/// Rust says `x86_64` where the reference implementation says `amd64`, and
/// `aarch64` where it says `arm64`. The suffix is operator-facing output copied
/// into bug reports, so it keeps the reference spelling; the mapping only
/// covers the two Windows targets the kernel ships for.
fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_has_the_version_and_platform() {
        let rendered = string();
        assert!(rendered.starts_with("o824 "), "{rendered}");
        assert!(
            rendered.contains(version_text()),
            "missing version in {rendered}"
        );
        assert!(
            rendered.ends_with(&format!("{}/{}", std::env::consts::OS, arch())),
            "missing platform suffix in {rendered}"
        );
    }

    #[test]
    fn string_has_no_empty_parenthetical() {
        // In a default build neither commit nor build time is injected, so the
        // output must not contain a dangling "()".
        if trim(COMMIT).is_empty() && trim(BUILD_TIME).is_empty() {
            assert!(!string().contains('('), "{}", string());
        }
    }

    #[test]
    fn version_text_falls_back_to_the_development_marker() {
        assert_eq!(version_text(), "0.1.0-dev");
    }
}
