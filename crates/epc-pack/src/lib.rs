//! EPC archive assembly primitives.
//!
//! This crate contains the first reference `core-format` writer. It assembles a
//! valid source directory into a `.epc` ZIP archive, regenerating
//! `proof/hashes.json` so the result is accepted by `epc-validate`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use epc_core::{
    HashEntry, HashTransform, Hashes, CORE_DOMAIN_SEPARATOR, COVER_PATH, HASHES_PATH,
    HASH_ALGORITHM_SHA256, INTEGRITY_VERSION_1, MANIFEST_PATH, MAX_COVER_SIZE, MAX_MESSAGE_SIZE,
    MAX_THUMBNAIL_SIZE, MESSAGE_PATH, THUMBNAIL_PATH,
};
use epc_validate::{validate_core_directory, validate_epc_file, ValidationReport};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Request to assemble an EPC archive from an unpacked source directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackRequest {
    /// Directory containing the files to pack.
    pub source_dir: PathBuf,

    /// Destination `.epc` archive path.
    pub output_file: PathBuf,
}

impl PackRequest {
    /// Creates a packing request from a source directory and output file.
    pub fn new(source_dir: impl Into<PathBuf>, output_file: impl Into<PathBuf>) -> Self {
        Self {
            source_dir: source_dir.into(),
            output_file: output_file.into(),
        }
    }

    /// Returns the source directory.
    pub fn source_dir(&self) -> &Path {
        &self.source_dir
    }

    /// Returns the destination archive file.
    pub fn output_file(&self) -> &Path {
        &self.output_file
    }
}

/// Error returned when packing a `core-format` capsule fails.
#[derive(Debug)]
pub enum PackError {
    /// The staged capsule failed validation before ZIP assembly.
    InvalidSource(ValidationReport),

    /// Filesystem or ZIP writer error.
    Io(io::Error),

    /// JSON serialization error while generating `proof/hashes.json`.
    Json(serde_json::Error),

    /// ZIP writer error.
    Zip(zip::result::ZipError),
}

impl From<io::Error> for PackError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for PackError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<zip::result::ZipError> for PackError {
    fn from(error: zip::result::ZipError) -> Self {
        Self::Zip(error)
    }
}

/// Packs a source directory into a valid EPC 1.0 `core-format` archive.
///
/// The source directory must contain `manifest.json`, `media/cover.jxl`,
/// `media/thumbnail.jxl`, and `text/message.md`. `proof/hashes.json` is
/// regenerated during packing, even when a source copy already exists.
pub fn pack_core_format(request: PackRequest) -> Result<(), PackError> {
    let staging = TempDir::new("epc-pack");
    copy_core_source(request.source_dir(), staging.path())?;
    write_hashes(staging.path())?;

    let report = validate_core_directory(staging.path());
    if !report.is_valid() {
        return Err(PackError::InvalidSource(report));
    }

    write_zip(staging.path(), request.output_file())?;
    Ok(())
}

/// Generates the official `core-format` conformance test vector set.
///
/// `output_root` is the `test-vectors/core-format` directory. The function
/// writes valid archives under `valid/`, invalid archives under `invalid/`, and
/// expected validation reports under `reports/invalid/`.
pub fn generate_core_format_test_vectors(output_root: impl AsRef<Path>) -> Result<(), PackError> {
    let output_root = output_root.as_ref();
    let valid_dir = output_root.join("valid");
    let invalid_dir = output_root.join("invalid");
    let invalid_reports_dir = output_root.join("reports").join("invalid");

    fs::create_dir_all(&valid_dir)?;
    fs::create_dir_all(&invalid_dir)?;
    fs::create_dir_all(&invalid_reports_dir)?;

    generate_valid_minimal(&valid_dir.join("minimal.epc"))?;
    generate_valid_max_limits(&valid_dir.join("max-limits.epc"))?;
    generate_valid_with_directory_entries(&valid_dir.join("with-directory-entries.epc"))?;

    generate_invalid_missing_manifest(&invalid_dir.join("missing-manifest.epc"))?;
    write_expected_report(
        &invalid_dir.join("missing-manifest.epc"),
        &invalid_reports_dir.join("missing-manifest.json"),
    )?;

    generate_invalid_missing_cover(&invalid_dir.join("missing-cover.epc"))?;
    write_expected_report(
        &invalid_dir.join("missing-cover.epc"),
        &invalid_reports_dir.join("missing-cover.json"),
    )?;

    generate_invalid_unexpected_entry(&invalid_dir.join("unexpected-entry.epc"))?;
    write_expected_report(
        &invalid_dir.join("unexpected-entry.epc"),
        &invalid_reports_dir.join("unexpected-entry.json"),
    )?;

    generate_invalid_digest_mismatch(&invalid_dir.join("digest-mismatch.epc"))?;
    write_expected_report(
        &invalid_dir.join("digest-mismatch.epc"),
        &invalid_reports_dir.join("digest-mismatch.json"),
    )?;

    generate_invalid_bad_card_id(&invalid_dir.join("bad-card-id.epc"))?;
    write_expected_report(
        &invalid_dir.join("bad-card-id.epc"),
        &invalid_reports_dir.join("bad-card-id.json"),
    )?;

    generate_invalid_zip64(&invalid_dir.join("zip64.epc"))?;
    write_expected_report(
        &invalid_dir.join("zip64.epc"),
        &invalid_reports_dir.join("zip64.json"),
    )?;

    generate_invalid_too_large_cover(&invalid_dir.join("too-large-cover.epc"))?;
    write_expected_report(
        &invalid_dir.join("too-large-cover.epc"),
        &invalid_reports_dir.join("too-large-cover.json"),
    )?;

    generate_invalid_too_many_entries(&invalid_dir.join("too-many-entries.epc"))?;
    write_expected_report(
        &invalid_dir.join("too-many-entries.epc"),
        &invalid_reports_dir.join("too-many-entries.json"),
    )?;

    generate_invalid_unsafe_path(&invalid_dir.join("unsafe-path.epc"))?;
    write_expected_report(
        &invalid_dir.join("unsafe-path.epc"),
        &invalid_reports_dir.join("unsafe-path.json"),
    )?;

    generate_invalid_unsupported_markdown_profile(
        &invalid_dir.join("unsupported-markdown-profile.epc"),
    )?;
    write_expected_report(
        &invalid_dir.join("unsupported-markdown-profile.epc"),
        &invalid_reports_dir.join("unsupported-markdown-profile.json"),
    )?;

    Ok(())
}

fn copy_core_source(source: &Path, staging: &Path) -> io::Result<()> {
    for directory in ["media", "text", "proof"] {
        fs::create_dir_all(staging.join(directory))?;
    }

    for path in [MANIFEST_PATH, COVER_PATH, THUMBNAIL_PATH, MESSAGE_PATH] {
        let destination = staging.join(path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source.join(path), destination)?;
    }

    Ok(())
}

fn write_hashes(root: &Path) -> Result<(), PackError> {
    let mut hashes = compute_hashes(root)?;
    hashes.core_digest = compute_core_digest(&hashes);
    let json = serde_json::to_string_pretty(&hashes)?;
    fs::write(root.join(HASHES_PATH), json)?;
    Ok(())
}

fn compute_hashes(root: &Path) -> Result<Hashes, PackError> {
    let entries = [
        (MANIFEST_PATH, HashTransform::Jcs),
        (COVER_PATH, HashTransform::Identity),
        (THUMBNAIL_PATH, HashTransform::Identity),
        (MESSAGE_PATH, HashTransform::Identity),
    ]
    .into_iter()
    .map(|(path, transform)| {
        Ok(HashEntry {
            path: path.to_string(),
            transform,
            digest: digest_entry(&root.join(path), transform)?,
        })
    })
    .collect::<Result<Vec<_>, PackError>>()?;

    Ok(Hashes {
        integrity_version: INTEGRITY_VERSION_1.to_string(),
        hash_algorithm: HASH_ALGORITHM_SHA256.to_string(),
        entries,
        core_digest: String::new(),
    })
}

fn digest_entry(path: &Path, transform: HashTransform) -> Result<String, PackError> {
    let bytes = fs::read(path)?;
    let digest_bytes = match transform {
        HashTransform::Identity => sha256(&bytes),
        HashTransform::Jcs => {
            let value: Value = serde_json::from_slice(&bytes)?;
            sha256(canonical_json(&value).as_bytes())
        }
    };
    Ok(URL_SAFE_NO_PAD.encode(digest_bytes))
}

fn compute_core_digest(hashes: &Hashes) -> String {
    let descriptor = integrity_descriptor_value(hashes);
    let mut hasher = Sha256::new();
    hasher.update(CORE_DOMAIN_SEPARATOR.as_bytes());
    hasher.update(canonical_json(&descriptor).as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn write_zip(root: &Path, output_file: &Path) -> Result<(), PackError> {
    if let Some(parent) = output_file.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = File::create(output_file)?;
    let mut zip = ZipWriter::new(file);
    let directory_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o755);
    let file_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);

    for directory in ["media/", "text/", "proof/"] {
        zip.add_directory(directory, directory_options)?;
    }

    for path in [
        MANIFEST_PATH,
        COVER_PATH,
        THUMBNAIL_PATH,
        MESSAGE_PATH,
        HASHES_PATH,
    ] {
        zip.start_file(path, file_options)?;
        let bytes = fs::read(root.join(path))?;
        zip.write_all(&bytes)?;
    }

    zip.finish()?;
    Ok(())
}

fn write_zip_with_entries(
    root: &Path,
    output_file: &Path,
    include_directories: bool,
    entries: &[&str],
    extra_entries: &[(&str, &[u8])],
) -> Result<(), PackError> {
    if let Some(parent) = output_file.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = File::create(output_file)?;
    let mut zip = ZipWriter::new(file);
    let directory_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o755);
    let file_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);

    if include_directories {
        for directory in ["media/", "text/", "proof/"] {
            zip.add_directory(directory, directory_options)?;
        }
    }

    for path in entries {
        zip.start_file(*path, file_options)?;
        let bytes = fs::read(root.join(path))?;
        zip.write_all(&bytes)?;
    }

    for (path, bytes) in extra_entries {
        zip.start_file(*path, file_options)?;
        zip.write_all(bytes)?;
    }

    zip.finish()?;
    Ok(())
}

fn generate_valid_minimal(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-minimal");
    write_vector_source(
        source.path(),
        "escale:00000000000000000000000000",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[
            MANIFEST_PATH,
            COVER_PATH,
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
        ],
        &[],
    )
}

fn generate_valid_max_limits(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-max-limits");
    write_vector_source(
        source.path(),
        "escale:00000000000000000000000001",
        MessageKind::Max,
    )?;
    write_deterministic_bytes(&source.path().join(COVER_PATH), MAX_COVER_SIZE)?;
    write_deterministic_bytes(&source.path().join(THUMBNAIL_PATH), MAX_THUMBNAIL_SIZE)?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[
            MANIFEST_PATH,
            COVER_PATH,
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
        ],
        &[],
    )
}

fn generate_valid_with_directory_entries(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-with-dirs");
    write_vector_source(
        source.path(),
        "escale:00000000000000000000000002",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        true,
        &[
            MANIFEST_PATH,
            COVER_PATH,
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
        ],
        &[],
    )
}

fn generate_invalid_missing_manifest(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-missing-manifest");
    write_vector_source(
        source.path(),
        "escale:00000000000000000000000003",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[COVER_PATH, THUMBNAIL_PATH, MESSAGE_PATH, HASHES_PATH],
        &[],
    )
}

fn generate_invalid_missing_cover(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-missing-cover");
    write_vector_source(
        source.path(),
        "escale:00000000000000000000000004",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[MANIFEST_PATH, THUMBNAIL_PATH, MESSAGE_PATH, HASHES_PATH],
        &[],
    )
}

fn generate_invalid_unexpected_entry(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-unexpected-entry");
    write_vector_source(
        source.path(),
        "escale:00000000000000000000000005",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[
            MANIFEST_PATH,
            COVER_PATH,
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
        ],
        &[("extra.txt", b"unexpected")],
    )
}

fn generate_invalid_digest_mismatch(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-digest-mismatch");
    write_vector_source(
        source.path(),
        "escale:00000000000000000000000006",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    fs::write(source.path().join(MESSAGE_PATH), "Changed after hashing.\n")?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[
            MANIFEST_PATH,
            COVER_PATH,
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
        ],
        &[],
    )
}

fn generate_invalid_bad_card_id(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-bad-card-id");
    write_vector_source(source.path(), "escale:not-a-ulid", MessageKind::Minimal)?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[
            MANIFEST_PATH,
            COVER_PATH,
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
        ],
        &[],
    )
}

fn generate_invalid_zip64(output_file: &Path) -> Result<(), PackError> {
    generate_valid_minimal(output_file)?;
    let mut file = fs::OpenOptions::new().append(true).open(output_file)?;
    file.write_all(&[0x50, 0x4b, 0x06, 0x06])?;
    Ok(())
}

fn generate_invalid_too_large_cover(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-too-large-cover");
    write_vector_source(
        source.path(),
        "escale:00000000000000000000000007",
        MessageKind::Minimal,
    )?;
    write_deterministic_bytes(&source.path().join(COVER_PATH), MAX_COVER_SIZE + 1)?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[
            MANIFEST_PATH,
            COVER_PATH,
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
        ],
        &[],
    )
}

fn generate_invalid_too_many_entries(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-too-many-entries");
    write_vector_source(
        source.path(),
        "escale:00000000000000000000000008",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        true,
        &[
            MANIFEST_PATH,
            COVER_PATH,
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
        ],
        &[("extra.txt", b"too many entries")],
    )
}

fn generate_invalid_unsafe_path(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-unsafe-path");
    write_vector_source(
        source.path(),
        "escale:00000000000000000000000009",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[
            MANIFEST_PATH,
            COVER_PATH,
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
        ],
        &[("../evil.txt", b"unsafe")],
    )
}

fn generate_invalid_unsupported_markdown_profile(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-unsupported-markdown-profile");
    write_vector_source(
        source.path(),
        "escale:0000000000000000000000000A",
        MessageKind::UnsupportedMarkdownProfile,
    )?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[
            MANIFEST_PATH,
            COVER_PATH,
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
        ],
        &[],
    )
}

fn write_expected_report(vector_file: &Path, report_file: &Path) -> Result<(), PackError> {
    let report = validate_epc_file(vector_file);
    let mut json = serde_json::to_string_pretty(&report)?;
    json.push('\n');
    fs::write(report_file, json)?;
    Ok(())
}

enum MessageKind {
    Minimal,
    Max,
    UnsupportedMarkdownProfile,
}

fn write_vector_source(root: &Path, card_id: &str, kind: MessageKind) -> Result<(), PackError> {
    fs::create_dir_all(root.join("media"))?;
    fs::create_dir_all(root.join("text"))?;
    fs::create_dir_all(root.join("proof"))?;

    let markdown_profile = match kind {
        MessageKind::UnsupportedMarkdownProfile => "epc-markdown-future",
        MessageKind::Minimal | MessageKind::Max => "epc-markdown-core",
    };

    let manifest = serde_json::json!({
        "epc_version": "1.0",
        "profile": "core-format",
        "type": "postcard",
        "id": card_id,
        "created_at": "2026-06-17T10:00:00Z",
        "sealed_at": "2026-06-17T10:05:00Z",
        "author": {
            "display_name": "Bruno"
        },
        "content": {
            "cover": {
                "path": "media/cover.jxl",
                "mime": "image/jxl"
            },
            "thumbnail": {
                "path": "media/thumbnail.jxl",
                "mime": "image/jxl"
            },
            "message": {
                "path": "text/message.md",
                "mime": "text/markdown",
                "markdown_profile": markdown_profile,
                "markdown_profile_version": "1.0"
            }
        }
    });

    fs::write(
        root.join(MANIFEST_PATH),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    fs::write(root.join(COVER_PATH), b"fake jxl cover")?;
    fs::write(root.join(THUMBNAIL_PATH), b"fake jxl thumbnail")?;
    match kind {
        MessageKind::Max => write_max_markdown(&root.join(MESSAGE_PATH))?,
        MessageKind::Minimal | MessageKind::UnsupportedMarkdownProfile => {
            fs::write(root.join(MESSAGE_PATH), "Bonjour **Escale**.\n")?;
        }
    }

    Ok(())
}

fn write_max_markdown(path: &Path) -> io::Result<()> {
    let mut file = File::create(path)?;
    let mut state = 0x4550_435f_4d44_4d41_u64;

    for _ in 0..16 {
        for _ in 0..4095 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let byte = b'a' + ((state >> 32) % 26) as u8;
            file.write_all(&[byte])?;
        }
        file.write_all(b"\n")?;
    }
    debug_assert_eq!(fs::metadata(path)?.len(), MAX_MESSAGE_SIZE);
    Ok(())
}

fn write_deterministic_bytes(path: &Path, len: u64) -> io::Result<()> {
    let mut file = File::create(path)?;
    let mut state = 0x4550_435f_5645_4354_u64;
    let mut remaining = len;
    let mut buffer = [0_u8; 8192];

    while remaining > 0 {
        let chunk_len = remaining.min(buffer.len() as u64) as usize;
        for byte in &mut buffer[..chunk_len] {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            *byte = (state >> 32) as u8;
        }
        file.write_all(&buffer[..chunk_len])?;
        remaining -= chunk_len as u64;
    }

    Ok(())
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

fn integrity_descriptor_value(hashes: &Hashes) -> Value {
    let entries = hashes
        .entries
        .iter()
        .map(hash_entry_value)
        .collect::<Vec<_>>();
    let mut object = Map::new();
    object.insert("entries".to_string(), Value::Array(entries));
    object.insert(
        "hash_algorithm".to_string(),
        Value::String(hashes.hash_algorithm.clone()),
    );
    object.insert(
        "integrity_version".to_string(),
        Value::String(hashes.integrity_version.clone()),
    );
    Value::Object(object)
}

fn hash_entry_value(entry: &HashEntry) -> Value {
    let mut object = Map::new();
    object.insert("digest".to_string(), Value::String(entry.digest.clone()));
    object.insert("path".to_string(), Value::String(entry.path.clone()));
    object.insert(
        "transform".to_string(),
        Value::String(
            match entry.transform {
                HashTransform::Jcs => "jcs",
                HashTransform::Identity => "identity",
            }
            .to_string(),
        ),
    );
    Value::Object(object)
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => {
            serde_json::to_string(value).expect("string serialization cannot fail")
        }
        Value::Array(values) => {
            let values = values.iter().map(canonical_json).collect::<Vec<_>>();
            format!("[{}]", values.join(","))
        }
        Value::Object(object) => {
            let sorted = object.iter().collect::<BTreeMap<_, _>>();
            let pairs = sorted
                .into_iter()
                .map(|(key, value)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("key serialization cannot fail"),
                        canonical_json(value)
                    )
                })
                .collect::<Vec<_>>();
            format!("{{{}}}", pairs.join(","))
        }
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let suffix = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{suffix}"));
        let _ = fs::remove_dir_all(&path);
        let _ = fs::create_dir_all(&path);
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use epc_validate::validate_epc_file;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn packs_core_format_archive_that_validator_accepts() {
        let source = TestDir::new();
        let output = std::env::temp_dir().join("epc-pack-valid.epc");
        let _ = fs::remove_file(&output);
        write_minimal_source(source.path(), "escale:00000000000000000000000000");

        pack_core_format(PackRequest::new(source.path(), &output)).unwrap();
        let report = validate_epc_file(&output);

        let _ = fs::remove_file(&output);
        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn refuses_to_pack_invalid_source() {
        let source = TestDir::new();
        let output = std::env::temp_dir().join("epc-pack-invalid.epc");
        let _ = fs::remove_file(&output);
        write_minimal_source(source.path(), "escale:not-a-ulid");

        let error = pack_core_format(PackRequest::new(source.path(), &output)).unwrap_err();

        let PackError::InvalidSource(report) = error else {
            panic!("expected invalid source report");
        };
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "EPC_MANIFEST_INVALID_CARD_ID"));
        assert!(!output.exists());
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let suffix = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("epc-pack-test-{suffix}"));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_minimal_source(root: &Path, card_id: &str) {
        fs::create_dir_all(root.join("media")).unwrap();
        fs::create_dir_all(root.join("text")).unwrap();

        let manifest = serde_json::json!({
            "epc_version": "1.0",
            "profile": "core-format",
            "type": "postcard",
            "id": card_id,
            "created_at": "2026-06-17T10:00:00Z",
            "sealed_at": "2026-06-17T10:05:00Z",
            "author": {
                "display_name": "Bruno"
            },
            "content": {
                "cover": {
                    "path": "media/cover.jxl",
                    "mime": "image/jxl"
                },
                "thumbnail": {
                    "path": "media/thumbnail.jxl",
                    "mime": "image/jxl"
                },
                "message": {
                    "path": "text/message.md",
                    "mime": "text/markdown",
                    "markdown_profile": "epc-markdown-core",
                    "markdown_profile_version": "1.0"
                }
            }
        });

        fs::write(
            root.join(MANIFEST_PATH),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(root.join(COVER_PATH), b"fake jxl cover").unwrap();
        fs::write(root.join(THUMBNAIL_PATH), b"fake jxl thumbnail").unwrap();
        fs::write(root.join(MESSAGE_PATH), "Bonjour **Escale**.\n").unwrap();
    }
}
