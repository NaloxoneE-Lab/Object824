//! Builds the support bundle described by spec §19.7: one zip a user can hand to
//! a maintainer, containing the build identity, the system summary, the doctor
//! findings, the profile's settings files, and the newest install journals.
//!
//! The bundle is deliberately small and **allow-listed**. Nothing is discovered by
//! walking the installation: exactly the five profile files named in
//! [`PROFILE_FILE_NAMES`] and the journal files under the data root are read, so
//! credentials, session transcripts, and tokens have no path into the archive.
//!
//! Every text entry is redacted by default: the installation root, the DSH home,
//! and `%USERPROFILE%` are replaced with `<ROOT>`, `<HOME>`, and `<USERPROFILE>`.
//! The replacements are applied to the raw bytes and to the JSON-escaped spelling
//! of each path, because a path inside a JSON document has doubled backslashes.
//! [`Options::include_paths`] turns the replacement off for a maintainer who needs
//! the real layout, and records that choice in the manifest's `redacted` field.
//!
//! The archive is written atomically: a temporary file in the destination
//! directory is filled, flushed, closed, and renamed over the target, so a reader
//! never observes a half-written zip and a failed run leaves no file at all.

#![deny(unsafe_code)]

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};
use o824_model::{Install, Timestamp};
use serde::{Deserialize, Serialize};

/// The entry that describes the bundle itself.
const MANIFEST_NAME: &str = "manifest.json";
/// The entry with the build identity.
const VERSION_NAME: &str = "version.json";
/// The entry with the OS summary.
const SYSTEM_NAME: &str = "system.json";
/// The entry with the complete findings envelope.
const DOCTOR_NAME: &str = "doctor.json";
/// The prefix the copied profile files get in the zip.
const PROFILE_DIR_NAME: &str = "profile";
/// The prefix the copied journals get in the zip.
const JOURNAL_DIR_NAME: &str = "journal";
/// How many of the newest journals are included.
const MAX_JOURNAL_ENTRIES: usize = 10;
/// The DSH home directory holding one directory per profile.
const PROFILES_DIR_NAME: &str = "profiles";

/// The redaction markers. They are part of the bundle format: a maintainer reads
/// them as "the real path was removed".
pub const PLACEHOLDER_ROOT: &str = "<ROOT>";
/// The DSH home placeholder.
pub const PLACEHOLDER_HOME: &str = "<HOME>";
/// The user-profile placeholder.
pub const PLACEHOLDER_USER_PROFILE: &str = "<USERPROFILE>";

/// The settings files §19.7 allows into a bundle. A missing file is simply absent
/// from the archive: the bundle reports what exists.
pub const PROFILE_FILE_NAMES: [&str; 5] = [
    "package.json",
    "cordis.yml",
    "cordis.patch.yml",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
];

/// Identifies the binary that produced the bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    /// The release version.
    pub version: String,
    /// The source revision.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit: String,
}

/// The installation fingerprint: what was diagnosed and where it lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallInfo {
    /// The installation id.
    pub id: String,
    /// The installation shape.
    pub kind: String,
    /// The DSH version.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// `"ok"`, `"degraded"`, or `"damaged"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub health: String,
    /// The installation root; redacted unless `include_paths` was set.
    pub root: String,
}

/// `manifest.json`: the bundle's table of contents and provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// When the bundle was built.
    pub created_at: Timestamp,
    /// The binary that built it.
    pub tool: Tool,
    /// The installation it describes.
    pub install: InstallInfo,
    /// True when paths were replaced with placeholders.
    pub redacted: bool,
    /// The archive entries, sorted.
    pub entries: Vec<String>,
}

/// `version.json`: the build identity, including the toolchain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    /// The release version.
    pub version: String,
    /// The source revision.
    #[serde(default)]
    pub commit: String,
    /// When the binary was built.
    #[serde(default)]
    pub build_time: String,
    /// The toolchain that compiled it.
    pub toolchain: String,
    /// The target platform.
    pub platform: String,
}

/// `system.json`. A field that cannot be read on this platform is omitted; the
/// bundle never guesses an OS version or a free-space value.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    /// The Windows product name and build, when it can be read.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub os_version: String,
    /// The target architecture.
    pub arch: String,
    /// The data root, when it is known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data_root: String,
    /// Free bytes on the data root's volume, when it can be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_disk_bytes: Option<i64>,
}

/// Controls one bundle.
#[derive(Clone, Debug)]
pub struct Options {
    /// The installation the bundle describes, already resolved by the caller.
    pub install: Install,
    /// The directory holding `journal\`. Empty means resolve the default.
    pub data_root: PathBuf,
    /// The zip path. It must name a file, not a directory.
    pub out: PathBuf,
    /// Keep absolute paths instead of replacing them with placeholders.
    pub include_paths: bool,
    /// The manifest timestamp; it is a field so tests can be deterministic.
    pub now: Timestamp,
}

/// One file destined for the archive, in memory.
struct Entry {
    name: String,
    data: Vec<u8>,
}

/// Builds one diagnostic bundle and returns the manifest written into it.
///
/// The return value matters for `--json`: it is the same document (with the same
/// redaction) that `manifest.json` holds, so a caller never has to reopen the zip
/// to learn what the bundle contains.
pub fn write(options: Options) -> Result<Manifest, Error> {
    let out = options.out.clone();
    if out.display().to_string().trim().is_empty() {
        return Err(usage_error("diagnose: --out needs a file path"));
    }
    let out = o824_model::path::clean(&out);
    // A directory cannot be replaced by a zip, and truncating its path would be
    // silent data damage. Refuse before creating any temporary file.
    if out.is_dir() {
        return Err(Error::new(
            Code::Usage,
            format!(
                "diagnose: {:?} is a directory, not a zip file",
                out.display().to_string()
            ),
        )
        .with_param("out", out.display().to_string()));
    }

    let data_root = if options.data_root.display().to_string().trim().is_empty() {
        o824_state::data_root(None)
    } else {
        options.data_root.clone()
    };

    let mut payloads = collect_payloads(&options.install, &data_root)?;
    payloads.push(Entry {
        name: VERSION_NAME.to_owned(),
        data: marshal(VERSION_NAME, &version_info())?,
    });
    payloads.push(Entry {
        name: SYSTEM_NAME.to_owned(),
        data: marshal(SYSTEM_NAME, &system_info(&data_root))?,
    });
    payloads.push(Entry {
        name: DOCTOR_NAME.to_owned(),
        data: marshal(
            DOCTOR_NAME,
            &o824_verify::doctor(&options.install, &data_root),
        )?,
    });

    // The entry list is decided before the manifest is marshalled, so the manifest
    // can name itself without a size that would depend on its own content.
    let mut names: Vec<String> = payloads.iter().map(|entry| entry.name.clone()).collect();
    names.push(MANIFEST_NAME.to_owned());
    names.sort();
    names.dedup();

    let mut manifest = Manifest {
        created_at: options.now,
        tool: Tool {
            version: o824_version::VERSION.to_owned(),
            commit: o824_version::COMMIT.to_owned(),
        },
        install: InstallInfo {
            id: options.install.install_id.clone(),
            kind: options.install.kind.clone(),
            version: options.install.version.clone(),
            health: options.install.health.clone(),
            root: options.install.root.clone(),
        },
        redacted: !options.include_paths,
        entries: names,
    };

    if !options.include_paths {
        let redactor = Redactor::new(
            &options.install.root,
            &options.install.home,
            &std::env::var("USERPROFILE").unwrap_or_default(),
        );
        // The manifest is both an archive entry and the command's `--json` output,
        // so its own path field is redacted before marshalling: redacting only the
        // written bytes would leak the real root through `--json`.
        manifest.install.root = redactor.redact(&manifest.install.root);
        for payload in &mut payloads {
            payload.data = redactor.redact_bytes(&payload.data);
        }
    }

    payloads.push(Entry {
        name: MANIFEST_NAME.to_owned(),
        data: marshal(MANIFEST_NAME, &manifest)?,
    });
    // Deterministic archive order: the manifest's entry list is sorted, and the
    // zip is written in the same order for reproducible bundles.
    payloads.sort_by(|left, right| left.name.cmp(&right.name));

    write_zip(&out, &payloads)?;
    Ok(manifest)
}

/// The build identity.
fn version_info() -> VersionInfo {
    VersionInfo {
        version: o824_version::VERSION.to_owned(),
        commit: o824_version::COMMIT.to_owned(),
        build_time: o824_version::BUILD_TIME.to_owned(),
        toolchain: format!(
            "rustc {}",
            option_env!("O824_RUSTC_VERSION").unwrap_or("unknown")
        ),
        platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
    }
}

/// The OS summary, read best-effort.
fn system_info(data_root: &Path) -> SystemInfo {
    SystemInfo {
        os_version: platform::os_version(),
        arch: std::env::consts::ARCH.to_owned(),
        data_root: data_root.display().to_string(),
        free_disk_bytes: platform::free_disk_bytes(data_root),
    }
}

/// Reads the allow-listed profile and journal files.
fn collect_payloads(install: &Install, data_root: &Path) -> Result<Vec<Entry>, Error> {
    let mut entries = collect_profiles(install)?;
    entries.extend(collect_journals(data_root)?);
    Ok(entries)
}

/// Copies the five settings files of every profile the installation lists.
fn collect_profiles(install: &Install) -> Result<Vec<Entry>, Error> {
    let home = if install.home.trim().is_empty() {
        install.root.trim().to_owned()
    } else {
        install.home.trim().to_owned()
    };
    if home.is_empty() {
        return Ok(Vec::new());
    }
    let home = o824_model::path::clean(Path::new(&home));

    let profiles = if install.profiles.is_empty() {
        o824_discovery_profile_names(&home)
    } else {
        install.profiles.clone()
    };

    let mut entries = Vec::with_capacity(profiles.len() * PROFILE_FILE_NAMES.len());
    for profile in profiles {
        // A profile name becomes a path component in the archive, so it must be
        // exactly one.
        if !is_single_path_element(&profile) {
            continue;
        }
        let directory = home.join(PROFILES_DIR_NAME).join(&profile);
        for name in PROFILE_FILE_NAMES {
            let path = directory.join(name);
            match std::fs::read(&path) {
                Ok(data) => entries.push(Entry {
                    name: format!("{PROFILE_DIR_NAME}/{profile}/{name}"),
                    data,
                }),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(file_error("read a profile file", &path, &error)),
            }
        }
    }
    Ok(entries)
}

/// Lists the profile directory names under `<home>\profiles`, sorted.
fn o824_discovery_profile_names(home: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(home.join(PROFILES_DIR_NAME)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Copies the newest journals from `<dataRoot>\journal`.
///
/// A missing journal directory is not an error: it means no operation was ever
/// recorded, not that the bundle is broken.
fn collect_journals(data_root: &Path) -> Result<Vec<Entry>, Error> {
    let directory = data_root.join(JOURNAL_DIR_NAME);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(file_error("read the journal directory", &directory, &error)),
    };

    struct JournalFile {
        name: String,
        modified: std::time::SystemTime,
    }
    let mut files: Vec<JournalFile> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
            || !name.to_lowercase().ends_with(".json")
        {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            // A file that vanished between the listing and the stat is not a
            // reason to fail the whole bundle.
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        files.push(JournalFile {
            name,
            modified: metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
        });
    }

    // Newest first; the name breaks ties so two journals written in the same clock
    // tick still produce a stable selection.
    files.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.name.cmp(&right.name))
    });
    files.truncate(MAX_JOURNAL_ENTRIES);
    // The archive order is by name so the zip is reproducible.
    files.sort_by(|left, right| left.name.cmp(&right.name));

    let mut payloads = Vec::with_capacity(files.len());
    for file in files {
        let path = directory.join(&file.name);
        match std::fs::read(&path) {
            Ok(data) => payloads.push(Entry {
                name: format!("{JOURNAL_DIR_NAME}/{}", file.name),
                data,
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(file_error("read a journal file", &path, &error)),
        }
    }
    Ok(payloads)
}

/// Encodes `value` as indented JSON with a trailing newline.
fn marshal<T: Serialize>(name: &str, value: &T) -> Result<Vec<u8>, Error> {
    let mut data = serde_json::to_vec_pretty(value).map_err(|error| {
        Error::new(
            Code::Internal,
            format!("diagnose: cannot encode {name}: {error}"),
        )
        .with_param("entry", name)
        .with_param("error", error.to_string())
    })?;
    data.push(b'\n');
    Ok(data)
}

/// Writes the entries to `out` atomically.
fn write_zip(out: &Path, entries: &[Entry]) -> Result<(), Error> {
    let Some(directory) = out.parent() else {
        return Err(file_error(
            "derive the bundle directory",
            out,
            &io::Error::other("the path has no parent directory"),
        ));
    };
    std::fs::create_dir_all(directory)
        .map_err(|error| file_error("create the bundle directory", directory, &error))?;
    let temporary = directory.join(format!(
        "{}.tmp-{}",
        out.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "diagnose.zip".to_owned()),
        std::process::id()
    ));

    let written = (|| -> Result<(), Error> {
        let file = std::fs::File::create(&temporary)
            .map_err(|error| file_error("create the diagnostic bundle", &temporary, &error))?;
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for entry in entries {
            archive.start_file(&entry.name, options).map_err(|error| {
                Error::new(
                    Code::Internal,
                    format!(
                        "diagnose: cannot add {:?} to the bundle: {error}",
                        entry.name
                    ),
                )
            })?;
            archive.write_all(&entry.data).map_err(|error| {
                Error::new(
                    Code::Internal,
                    format!(
                        "diagnose: cannot write {:?} into the bundle: {error}",
                        entry.name
                    ),
                )
            })?;
        }
        let file = archive.finish().map_err(|error| {
            Error::new(
                Code::Internal,
                format!("diagnose: cannot finish the diagnostic bundle: {error}"),
            )
            .with_param("path", temporary.display().to_string())
        })?;
        // The bundle is evidence a user will send away, so it is flushed before the
        // rename rather than left in the page cache.
        file.sync_all()
            .map_err(|error| file_error("flush the diagnostic bundle", &temporary, &error))?;
        Ok(())
    })();

    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temporary, out) {
        let _ = std::fs::remove_file(&temporary);
        return Err(file_error("publish the diagnostic bundle", out, &error));
    }
    Ok(())
}

/// Replaces absolute paths with placeholders in both raw and JSON-escaped
/// spellings.
///
/// Replacements are sorted longest-first so a shorter path that is a prefix of a
/// longer one (the user profile contains the install root, which contains the DSH
/// home) cannot partially rewrite the longer one.
struct Redactor {
    replacements: Vec<(String, &'static str)>,
}

impl Redactor {
    fn new(install_root: &str, home: &str, user_profile: &str) -> Redactor {
        let candidates = [
            (install_root, PLACEHOLDER_ROOT),
            (home, PLACEHOLDER_HOME),
            (user_profile, PLACEHOLDER_USER_PROFILE),
        ];

        let mut seen_paths: BTreeSet<String> = BTreeSet::new();
        let mut seen_variants: BTreeSet<String> = BTreeSet::new();
        let mut replacements: Vec<(String, &'static str)> = Vec::new();
        for (path, placeholder) in candidates {
            let clean = path.trim();
            if clean.is_empty() {
                continue;
            }
            let clean = o824_model::path::clean(Path::new(clean))
                .display()
                .to_string();
            // Windows paths are case-insensitive, so duplicate detection is too.
            if !seen_paths.insert(clean.to_lowercase()) {
                continue;
            }
            for spelling in path_spellings(&clean) {
                for variant in path_variants(&spelling) {
                    if seen_variants.insert(variant.clone()) {
                        replacements.push((variant, placeholder));
                    }
                }
            }
        }

        replacements.sort_by_key(|pair| std::cmp::Reverse(pair.0.len()));
        Redactor { replacements }
    }

    fn redact(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for (old, new) in &self.replacements {
            if text.contains(old.as_str()) {
                text = text.replace(old.as_str(), new);
            }
        }
        text
    }

    fn redact_bytes(&self, data: &[u8]) -> Vec<u8> {
        let mut data = data.to_vec();
        for (old, new) in &self.replacements {
            let needle = old.as_bytes();
            if data.windows(needle.len()).any(|window| window == needle) {
                data = replace_bytes(&data, needle, new.as_bytes());
            }
        }
        data
    }
}

/// Every way the same directory can be spelled on this machine: as given, as an
/// 8.3 short name, and as its long name.
///
/// The short form is not academic. GitHub's Windows runners run with
/// `%USERPROFILE%` set to the long spelling while `%TEMP%` is the short one
/// (`RUNNER~1`), so a redaction table that knew only the long spelling would leave
/// the short one in the bundle. The same is true of any tree reached through a
/// short name.
fn path_spellings(clean: &str) -> Vec<String> {
    let mut spellings = vec![clean.to_owned()];
    for alternative in [platform::short_path(clean), platform::long_path(clean)]
        .into_iter()
        .flatten()
    {
        if !spellings
            .iter()
            .any(|spelling| spelling.eq_ignore_ascii_case(&alternative))
        {
            spellings.push(alternative);
        }
    }
    spellings
}

/// The spellings a path can take in the archive: the native form, the
/// forward-slash form, and the JSON-escaped native form.
fn path_variants(clean: &str) -> Vec<String> {
    let mut variants = vec![clean.to_owned(), clean.replace('\\', "/")];
    let escaped = clean.replace('\\', "\\\\");
    if escaped != clean {
        variants.push(escaped);
    }
    variants
}

/// Replaces every occurrence of `needle` in `haystack`.
fn replace_bytes(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut index = 0;
    while index < haystack.len() {
        if haystack[index..].starts_with(needle) {
            out.extend_from_slice(replacement);
            index += needle.len();
        } else {
            out.push(haystack[index]);
            index += 1;
        }
    }
    out
}

/// True when `value` can be exactly one path component.
fn is_single_path_element(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\', ':', '\0'])
}

fn usage_error(message: &str) -> Error {
    Error::new(Code::Usage, message)
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
            "diagnose: cannot {operation} {:?}: {cause}",
            path.display().to_string()
        ),
    )
    .with_param("error", cause.to_string())
    .with_param("path", path.display().to_string())
}

/// The platform-specific parts of the system summary.
///
/// Reading the OS build and the free space needs the Windows API, which has no
/// standard-library equivalent. The two calls live behind `#[allow(unsafe_code)]`
/// so the rest of the crate stays `deny(unsafe_code)`.
#[allow(unsafe_code)]
mod platform {
    /// The Windows product name and build, when the registry can be read.
    #[cfg(windows)]
    pub fn os_version() -> String {
        let product = registry_string(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "ProductName",
        );
        let build = registry_string(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuild",
        );
        match (product, build) {
            (Some(product), Some(build)) => Some(format!("{product} (build {build})")),
            (Some(product), None) => Some(product),
            _ => None,
        }
        .unwrap_or_default()
    }

    #[cfg(not(windows))]
    pub fn os_version() -> String {
        String::new()
    }

    /// Reads one `REG_SZ` value out of `HKLM`.
    #[cfg(windows)]
    fn registry_string(path: &str, value: &str) -> Option<String> {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

        unsafe {
            let path = HSTRING::from(path);
            let value = HSTRING::from(value);
            let mut size = 0u32;
            let status = RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &path,
                &value,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            );
            if status != ERROR_SUCCESS || size == 0 {
                return None;
            }
            let mut buffer = vec![0u8; size as usize];
            let status = RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &path,
                &value,
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr() as *mut _),
                Some(&mut size),
            );
            if status != ERROR_SUCCESS {
                return None;
            }
            let wide: Vec<u16> = buffer
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .take_while(|unit| *unit != 0)
                .collect();
            Some(String::from_utf16_lossy(&wide))
        }
    }

    /// The 8.3 short spelling of a path, when Windows has one.
    #[cfg(windows)]
    pub fn short_path(path: &str) -> Option<String> {
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;

        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buffer = vec![0u16; 512];
        // SAFETY: `wide` is NUL-terminated and outlives the call, and `buffer` is a
        // valid writable slice of the length passed.
        let length =
            unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(buffer.as_mut_slice())) };
        if length == 0 || length as usize >= buffer.len() {
            return None;
        }
        let text = String::from_utf16_lossy(&buffer[..length as usize]);
        if text.eq_ignore_ascii_case(path) {
            None
        } else {
            Some(text)
        }
    }

    /// The long spelling of a path, when it was given as a short name.
    #[cfg(windows)]
    pub fn long_path(path: &str) -> Option<String> {
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::GetLongPathNameW;

        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buffer = vec![0u16; 1024];
        // SAFETY: as above — a NUL-terminated input and a writable output buffer.
        let length =
            unsafe { GetLongPathNameW(PCWSTR(wide.as_ptr()), Some(buffer.as_mut_slice())) };
        if length == 0 || length as usize >= buffer.len() {
            return None;
        }
        let text = String::from_utf16_lossy(&buffer[..length as usize]);
        if text.eq_ignore_ascii_case(path) {
            None
        } else {
            Some(text)
        }
    }

    /// A build without the Windows API has no short names.
    #[cfg(not(windows))]
    pub fn short_path(_path: &str) -> Option<String> {
        None
    }

    /// A build without the Windows API has no short names.
    #[cfg(not(windows))]
    pub fn long_path(_path: &str) -> Option<String> {
        None
    }

    /// Free bytes on the volume holding `path`, when it can be read.
    #[cfg(windows)]
    pub fn free_disk_bytes(path: &std::path::Path) -> Option<i64> {
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        // The volume root is what GetDiskFreeSpaceExW wants; a path inside it also
        // works, so the data root is passed straight through.
        let wide: Vec<u16> = path
            .display()
            .to_string()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut free = 0u64;
        let ok = unsafe {
            GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None).is_ok()
        };
        if ok {
            Some(free as i64)
        } else {
            None
        }
    }

    #[cfg(not(windows))]
    pub fn free_disk_bytes(_path: &std::path::Path) -> Option<i64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_fixtures::TempDir;

    fn install_of(root: &Path) -> Install {
        Install {
            install_id: "local-test".to_owned(),
            kind: "desktop".to_owned(),
            root: root.display().to_string(),
            version: o824_fixtures::DSH_VERSION.to_owned(),
            profiles: vec!["desktop".to_owned()],
            health: "degraded".to_owned(),
            home: root.display().to_string(),
        }
    }

    fn write_bundle(kind: &str, include_paths: bool) -> (TempDir, Manifest, Vec<String>) {
        let fixture = o824_fixtures::build(kind).expect("fixture");
        let store = TempDir::new("o824-diagnose-").expect("store");
        let out = store.path().join("bundle.zip");
        let manifest = write(Options {
            install: install_of(fixture.root()),
            data_root: store.path().to_path_buf(),
            out: out.clone(),
            include_paths,
            now: Timestamp::parse("2026-11-01T12:00:00Z").expect("timestamp"),
        })
        .expect("write");
        let names = zip_entries(&out);
        (store, manifest, names)
    }

    /// Reads the entry names of a zip, in archive order.
    fn zip_entries(path: &Path) -> Vec<String> {
        let file = std::fs::File::open(path).expect("open the zip");
        let mut archive = zip::ZipArchive::new(file).expect("read the zip");
        (0..archive.len())
            .map(|index| archive.by_index(index).expect("entry").name().to_owned())
            .collect()
    }

    fn zip_entry(path: &Path, name: &str) -> String {
        let file = std::fs::File::open(path).expect("open the zip");
        let mut archive = zip::ZipArchive::new(file).expect("read the zip");
        let mut entry = archive.by_name(name).expect("entry");
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text).expect("read");
        text
    }

    #[test]
    fn the_bundle_has_the_documented_entries() {
        let (_store, manifest, names) = write_bundle("git-dep", false);
        assert_eq!(
            names,
            [
                "doctor.json",
                "manifest.json",
                "profile/desktop/cordis.patch.yml",
                "profile/desktop/cordis.yml",
                "profile/desktop/package.json",
                "profile/desktop/pnpm-lock.yaml",
                "profile/desktop/pnpm-workspace.yaml",
                "system.json",
                "version.json"
            ]
        );
        assert_eq!(manifest.entries, names);
    }

    #[test]
    fn the_manifest_is_redacted_by_default() {
        let (store, manifest, _names) = write_bundle("git-dep", false);
        assert!(manifest.redacted);
        assert_eq!(manifest.install.root, PLACEHOLDER_ROOT);

        let text = zip_entry(&store.path().join("bundle.zip"), "manifest.json");
        assert!(text.contains(PLACEHOLDER_ROOT), "{text}");
    }

    #[test]
    fn no_installation_path_survives_in_a_redacted_bundle() {
        let fixture = o824_fixtures::build("git-dep").expect("fixture");
        let store = TempDir::new("o824-diagnose-").expect("store");
        let out = store.path().join("bundle.zip");
        write(Options {
            install: install_of(fixture.root()),
            data_root: store.path().to_path_buf(),
            out: out.clone(),
            include_paths: false,
            now: Timestamp::parse("2026-11-01T12:00:00Z").expect("timestamp"),
        })
        .expect("write");

        // The three paths the redactor promises to remove, in every spelling this
        // machine knows.
        let mut forbidden: Vec<String> = Vec::new();
        for path in [
            fixture.root(),
            std::path::Path::new(&std::env::var("USERPROFILE").unwrap_or_default()),
        ] {
            let clean = o824_model::path::clean(path).display().to_string();
            forbidden.extend(path_spellings(&clean));
        }

        let file = std::fs::File::open(&out).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("read");
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).expect("entry");
            let mut text = String::new();
            std::io::Read::read_to_string(&mut entry, &mut text).expect("read");
            for path in &forbidden {
                assert!(
                    !text.contains(path.as_str()),
                    "{}: still carries {path:?}
{text}",
                    entry.name()
                );
            }
        }
    }

    /// A redaction table that only knew the long spelling of a path would leave
    /// the 8.3 spelling in the bundle — which is exactly what happens on a Windows
    /// CI runner, where `%TEMP%` is spelled `RUNNER~1`.
    #[test]
    fn the_redactor_knows_the_short_spelling_of_a_path() {
        let profile = std::env::var("USERPROFILE").unwrap_or_default();
        if profile.is_empty() {
            return;
        }
        let Some(short) = platform::short_path(&profile) else {
            // This volume has 8.3 names disabled; nothing to check.
            return;
        };
        let redactor = Redactor::new(&profile, "", "");
        assert_eq!(redactor.redact(&short), PLACEHOLDER_ROOT);
        // And the long spelling still works.
        assert_eq!(redactor.redact(&profile), PLACEHOLDER_ROOT);
    }

    #[test]
    fn include_paths_keeps_the_real_root_and_says_so() {
        let (_store, manifest, _names) = write_bundle("git-dep", true);
        assert!(!manifest.redacted);
        assert!(!manifest.install.root.is_empty());
        assert_ne!(manifest.install.root, PLACEHOLDER_ROOT);
    }

    #[test]
    fn the_doctor_entry_is_the_full_envelope() {
        let (store, _manifest, _names) = write_bundle("git-dep", true);
        let text = zip_entry(&store.path().join("bundle.zip"), "doctor.json");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["envelope"], "dsh-doctor/v1");
        assert_eq!(value["findings"][0]["checkId"], "F17");
    }

    #[test]
    fn a_directory_as_out_is_refused() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = TempDir::new("o824-diagnose-").expect("store");
        let error = write(Options {
            install: install_of(fixture.root()),
            data_root: store.path().to_path_buf(),
            out: store.path().to_path_buf(),
            include_paths: false,
            now: Timestamp::now(),
        })
        .expect_err("must fail");
        assert_eq!(error.code, Code::Usage);
    }

    #[test]
    fn the_redactor_prefers_the_longest_path() {
        let redactor = Redactor::new(
            r"C:\Users\demo\AppData\Local\Programs\DeepSeek Harness",
            r"C:\Users\demo\.dsh",
            r"C:\Users\demo",
        );
        assert_eq!(
            redactor.redact(r"C:\Users\demo\AppData\Local\Programs\DeepSeek Harness"),
            PLACEHOLDER_ROOT
        );
        assert_eq!(redactor.redact(r"C:\Users\demo\.dsh"), PLACEHOLDER_HOME);
        assert_eq!(
            redactor.redact(r"C:\Users\demo\other"),
            r"<USERPROFILE>\other"
        );
    }

    #[test]
    fn the_redactor_handles_the_json_escaped_spelling() {
        let redactor = Redactor::new(r"C:\root", r"C:\home", r"C:\Users\demo");
        assert_eq!(
            redactor.redact(r#"{"root":"C:\\root\\profiles"}"#),
            r#"{"root":"<ROOT>\\profiles"}"#
        );
    }
}
