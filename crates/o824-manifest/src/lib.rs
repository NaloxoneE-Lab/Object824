//! The release manifest: a per-file sha256 list, signed with ed25519.
//!
//! §13 of the design says what this is for. The launcher ships unsigned — no code signing
//! certificate was purchased, and that decision stands — so the thing that has to be
//! checkable is not the binary's signature but the **list** of what was published: which
//! files, with which hashes, signed by a key the publisher holds. Anyone can then confirm
//! that the file they downloaded is the file that was signed, and that the list came from the
//! publisher and not from whoever served the file.
//!
//! The signature covers the manifest **without** its own signature field, so the bytes that
//! are verified are exactly the bytes that were signed.

// Denied rather than forbidden: one platform call (`BCryptGenRandom`) needs an `unsafe`
// block, and the deny leaves room to allow exactly that one, with its reason attached.
#![deny(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature as Ed25519Signature, Signer, SigningKey, VerifyingKey};
use o824_errs::{Code, Error};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The manifest format this crate reads and writes.
pub const FORMAT: &str = "o824-manifest/v1";

/// The file name a published manifest uses.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The only signature algorithm accepted. Named in the format so a future algorithm can be
/// added without guessing at the old one.
pub const ALGORITHM: &str = "ed25519";

/// One published file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// The path inside the release, with forward slashes and no leading separator.
    pub path: String,
    /// The file's sha256, lowercase hex.
    pub sha256: String,
    /// Its size in bytes, which makes an accidental truncation obvious before hashing.
    pub size: u64,
}

/// The signature over a manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    /// Always [`ALGORITHM`]; present so a reader can refuse an algorithm it does not know.
    pub algorithm: String,
    /// The first 16 hex characters of the public key's sha256, so a key can be named in a log
    /// without printing the key itself.
    pub key_id: String,
    /// The public key, lowercase hex, so verification needs no second file.
    pub public_key: String,
    /// The signature itself, lowercase hex.
    pub value: String,
}

/// A release manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Always [`FORMAT`].
    pub format: String,
    /// The product version this release is.
    pub version: String,
    /// When it was built, RFC 3339.
    pub built_at: String,
    /// The files, sorted by path.
    pub files: Vec<Entry>,
    /// Absent until [`sign`] is called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
}

/// What a verification found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// How many files were read and hashed.
    pub checked: usize,
    /// Files whose bytes do not match the manifest.
    pub changed: Vec<String>,
    /// Files the manifest lists that are not on disk.
    pub missing: Vec<String>,
    /// Files on disk that the manifest does not list.
    pub unexpected: Vec<String>,
}

impl Report {
    /// True when nothing disagreed.
    pub fn is_clean(&self) -> bool {
        self.changed.is_empty() && self.missing.is_empty() && self.unexpected.is_empty()
    }
}

/// Builds a manifest for every regular file under `root`.
///
/// Directories are walked, symbolic links are skipped: a release is a set of files, and a
/// link inside one would describe some other machine's disk. The manifest file itself is
/// skipped so a signed manifest can live next to what it describes.
pub fn build(root: &Path, version: &str, built_at: &str) -> Result<Manifest, Error> {
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Manifest {
        format: FORMAT.to_owned(),
        version: version.to_owned(),
        built_at: built_at.to_owned(),
        files,
        signature: None,
    })
}

fn walk(root: &Path, directory: &Path, files: &mut Vec<Entry>) -> Result<(), Error> {
    let entries = fs::read_dir(directory).map_err(|error| {
        Error::new(
            Code::Permission,
            format!(
                "manifest: cannot list {:?}: {error}",
                directory.display().to_string()
            ),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            Error::new(
                Code::Permission,
                format!(
                    "manifest: cannot read an entry of {:?}: {error}",
                    directory.display().to_string()
                ),
            )
        })?;
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                return Err(Error::new(
                    Code::Permission,
                    format!(
                        "manifest: cannot stat {:?}: {error}",
                        path.display().to_string()
                    ),
                ))
            }
        };
        if metadata.is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            walk(root, &path, files)?;
            continue;
        }
        let relative = relative_path(root, &path);
        if relative == MANIFEST_FILE {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| {
            Error::new(
                Code::Permission,
                format!(
                    "manifest: cannot read {:?}: {error}",
                    path.display().to_string()
                ),
            )
        })?;
        files.push(Entry {
            path: relative,
            sha256: hex(&Sha256::digest(&bytes)),
            size: metadata.len(),
        });
    }
    Ok(())
}

/// The path as the manifest records it: relative, forward slashes.
fn relative_path(root: &Path, path: &Path) -> String {
    let relative: PathBuf = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// The bytes a signature covers: the manifest with no signature and no trailing newline.
///
/// `serde_json` preserves field order here (the workspace enables `preserve_order`), so the
/// same manifest always produces the same bytes — which is the whole point of signing them.
pub fn signed_bytes(manifest: &Manifest) -> Result<Vec<u8>, Error> {
    let mut bare = manifest.clone();
    bare.signature = None;
    serde_json::to_vec(&bare).map_err(|error| {
        Error::new(
            Code::Internal,
            format!("manifest: cannot encode the signed bytes: {error}"),
        )
    })
}

/// Signs a manifest in place.
pub fn sign(manifest: &mut Manifest, key: &SigningKey) -> Result<(), Error> {
    let bytes = signed_bytes(manifest)?;
    let signature = key.sign(&bytes);
    let public_key = key.verifying_key();
    manifest.signature = Some(Signature {
        algorithm: ALGORITHM.to_owned(),
        key_id: key_id(&public_key),
        public_key: hex(public_key.as_bytes()),
        value: hex(&signature.to_bytes()),
    });
    Ok(())
}

/// Verifies the signature over a manifest.
///
/// `expected` is the public key the caller trusts. When it is `None` the key inside the
/// manifest is used, which only proves the manifest is self-consistent — useful for catching
/// a corrupted download, useless against someone who can rewrite the whole file. The
/// distinction is the caller's to make, and it is spelled out here rather than hidden.
pub fn verify_signature(manifest: &Manifest, expected: Option<&str>) -> Result<(), Error> {
    let Some(signature) = &manifest.signature else {
        return Err(Error::new(
            Code::StateDrift,
            "manifest: it carries no signature",
        ));
    };
    if signature.algorithm != ALGORITHM {
        return Err(Error::new(
            Code::StateDrift,
            format!(
                "manifest: signature algorithm {:?} is not {ALGORITHM}",
                signature.algorithm
            ),
        ));
    }
    let public_bytes = unhex(&signature.public_key, "public key")?;
    let public: [u8; 32] = public_bytes.as_slice().try_into().map_err(|_| {
        Error::new(
            Code::StateDrift,
            "manifest: the public key is not 32 bytes".to_owned(),
        )
    })?;
    let key = VerifyingKey::from_bytes(&public).map_err(|error| {
        Error::new(
            Code::StateDrift,
            format!("manifest: the public key is not a valid ed25519 key: {error}"),
        )
    })?;

    if let Some(expected_key) = expected {
        if !expected_key
            .trim()
            .eq_ignore_ascii_case(&signature.public_key)
        {
            // Name both: the key that signed it, and the key that was expected. Naming the
            // same id twice is what happens when the expected key's id is computed from the
            // manifest's key instead of from the caller's argument.
            let expected_id = expected_key_id(expected_key);
            return Err(Error::new(
                Code::StateDrift,
                format!(
                    "manifest: it is signed by {}, not by the expected key {expected_id}",
                    signature.key_id
                ),
            ));
        }
    }

    let signature_bytes = unhex(&signature.value, "signature")?;
    let signature_bytes: [u8; 64] = signature_bytes.as_slice().try_into().map_err(|_| {
        Error::new(
            Code::StateDrift,
            "manifest: the signature is not 64 bytes".to_owned(),
        )
    })?;
    let signature = Ed25519Signature::from_bytes(&signature_bytes);
    let bytes = signed_bytes(manifest)?;
    key.verify_strict(&bytes, &signature).map_err(|_| {
        Error::new(
            Code::StateDrift,
            "manifest: the signature does not match its contents".to_owned(),
        )
    })
}

/// Compares a manifest against the files under `root`.
pub fn verify_files(manifest: &Manifest, root: &Path) -> Result<Report, Error> {
    let mut report = Report::default();

    let mut present: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut found = Vec::new();
    walk(root, root, &mut found)?;
    for entry in &found {
        present.insert(
            entry.path.clone(),
            root.join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR)),
        );
    }

    for entry in &manifest.files {
        let Some(path) = present.get(&entry.path) else {
            report.missing.push(entry.path.clone());
            continue;
        };
        let bytes = fs::read(path).map_err(|error| {
            Error::new(
                Code::Permission,
                format!(
                    "manifest: cannot read {:?}: {error}",
                    path.display().to_string()
                ),
            )
        })?;
        report.checked += 1;
        let digest = hex(&Sha256::digest(&bytes));
        if digest != entry.sha256 || bytes.len() as u64 != entry.size {
            report.changed.push(entry.path.clone());
        }
    }

    let listed: BTreeMap<&str, ()> = manifest
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), ()))
        .collect();
    for entry in &found {
        if !listed.contains_key(entry.path.as_str()) {
            report.unexpected.push(entry.path.clone());
        }
    }
    Ok(report)
}

/// Reads a manifest from disk.
pub fn load(path: &Path) -> Result<Manifest, Error> {
    let text = fs::read_to_string(path).map_err(|error| {
        Error::new(
            Code::Permission,
            format!(
                "manifest: cannot read {:?}: {error}",
                path.display().to_string()
            ),
        )
    })?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|error| {
        Error::new(
            Code::Usage,
            format!(
                "manifest: {:?} is not a manifest: {error}",
                path.display().to_string()
            ),
        )
    })?;
    if manifest.format != FORMAT {
        return Err(Error::new(
            Code::Usage,
            format!("manifest: format {:?} is not {FORMAT}", manifest.format),
        ));
    }
    Ok(manifest)
}

/// Writes a manifest, with a trailing newline so a diff of two manifests reads properly.
pub fn save(manifest: &Manifest, path: &Path) -> Result<(), Error> {
    let mut text = serde_json::to_string_pretty(manifest).map_err(|error| {
        Error::new(
            Code::Internal,
            format!("manifest: cannot encode it: {error}"),
        )
    })?;
    text.push('\n');
    fs::write(path, text).map_err(|error| {
        Error::new(
            Code::Permission,
            format!(
                "manifest: cannot write {:?}: {error}",
                path.display().to_string()
            ),
        )
    })
}

/// A fresh signing key from the operating system's random source.
///
/// The randomness comes from the platform (`BCryptGenRandom`, or `/dev/urandom`), not from a
/// new dependency: pulling in a crate for 32 bytes also pulled in an import-library build
/// step this workspace's toolchain cannot run, and a signing key is not the place to spend
/// that.
pub fn new_key() -> SigningKey {
    let mut secret = [0u8; 32];
    random_bytes(&mut secret).expect("the operating system refused to provide randomness");
    SigningKey::from_bytes(&secret)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn random_bytes(out: &mut [u8]) -> Result<(), Error> {
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };

    // SAFETY: the buffer is ours and its length is passed with it.
    let status = unsafe { BCryptGenRandom(None, out, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status.is_ok() {
        Ok(())
    } else {
        Err(Error::new(
            Code::Internal,
            format!("manifest: the system random source refused: {status:?}"),
        ))
    }
}

#[cfg(unix)]
fn random_bytes(out: &mut [u8]) -> Result<(), Error> {
    use std::io::Read;

    let mut file = fs::File::open("/dev/urandom").map_err(|error| {
        Error::new(
            Code::Internal,
            format!("manifest: cannot open /dev/urandom: {error}"),
        )
    })?;
    file.read_exact(out).map_err(|error| {
        Error::new(
            Code::Internal,
            format!("manifest: cannot read /dev/urandom: {error}"),
        )
    })
}

#[cfg(not(any(windows, unix)))]
fn random_bytes(_out: &mut [u8]) -> Result<(), Error> {
    Err(Error::new(
        Code::Internal,
        "manifest: this platform has no random source wired up".to_owned(),
    ))
}

/// Parses a secret key from 32 bytes of hex.
pub fn parse_key(text: &str) -> Result<SigningKey, Error> {
    let bytes = unhex(text.trim(), "secret key")?;
    let bytes: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
        Error::new(
            Code::Usage,
            "manifest: a secret key is 32 bytes, which is 64 hex characters".to_owned(),
        )
    })?;
    Ok(SigningKey::from_bytes(&bytes))
}

/// The public key for a secret key, as hex.
pub fn public_hex(key: &SigningKey) -> String {
    hex(key.verifying_key().as_bytes())
}

/// The short name of a public key given as hex, for an error message.
fn expected_key_id(hex_key: &str) -> String {
    let Ok(bytes) = unhex(hex_key, "expected public key") else {
        return hex_key.trim().to_owned();
    };
    let Ok(bytes): Result<[u8; 32], _> = bytes.as_slice().try_into() else {
        return hex_key.trim().to_owned();
    };
    match VerifyingKey::from_bytes(&bytes) {
        Ok(key) => key_id(&key),
        Err(_) => hex_key.trim().to_owned(),
    }
}

/// A short, stable name for a public key.
pub fn key_id(key: &VerifyingKey) -> String {
    hex(&Sha256::digest(key.as_bytes()))[..16].to_owned()
}

/// Lowercase hex.
fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// Parses lowercase or uppercase hex.
fn unhex(text: &str, what: &str) -> Result<Vec<u8>, Error> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return Err(Error::new(
            Code::Usage,
            format!("manifest: the {what} is not whole bytes of hex"),
        ));
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().chunks(2) {
        let pair = std::str::from_utf8(pair)
            .map_err(|_| Error::new(Code::Usage, format!("manifest: the {what} is not hex")))?;
        let byte = u8::from_str_radix(pair, 16)
            .map_err(|_| Error::new(Code::Usage, format!("manifest: the {what} is not hex")))?;
        bytes.push(byte);
    }
    Ok(bytes)
}
