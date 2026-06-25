//! EPC archive assembly primitives.
//!
//! This crate contains the first reference `core-format` writer. It assembles a
//! valid source directory into a `.epc` ZIP archive, regenerating
//! `proof/hashes.json` so the result is accepted by `epc-validate`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use epc_core::{
    cover_mime_for_path, is_supported_cover_path, is_valid_card_id, CreatedLocalTime, HashEntry,
    HashTransform, Hashes, Manifest, SignatureEntry, SignaturePayload, SignaturePolicy,
    SignatureProof, SignaturePublicKey, SignatureRequiredKey, SignatureSigner,
    CORE_DOMAIN_SEPARATOR, COVER_PATH, HASHES_PATH, HASH_ALGORITHM_SHA256, INTEGRITY_VERSION_1,
    MANIFEST_PATH, MAX_COVER_SIZE, MAX_MESSAGE_SIZE, MESSAGE_PATH, SIGNATURE_DOMAIN_SEPARATOR,
    SIGNATURE_PATH, THUMBNAIL_PATH,
};
use epc_image::ImageMetadataError;
use epc_validate::{
    validate_core_directory, validate_core_directory_with_options, validate_epc_file,
    ValidationOptions, ValidationReport,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);
static ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Request to create a new unpacked EPC draft directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDraftRequest {
    /// Directory where the draft files should be initialized.
    pub output_dir: PathBuf,

    /// Human-facing author name for `manifest.json`.
    pub author_display_name: String,

    /// Device-local creation context captured when the draft is initialized.
    pub created_local_time: CreatedLocalTime,

    /// Optional card identifier to write into `manifest.json`.
    pub id: Option<String>,

    /// Whether an existing `manifest.json` should be overwritten.
    pub force: bool,

    /// Capsule-relative cover path to declare in `manifest.json`.
    pub cover_path: String,
}

impl CreateDraftRequest {
    /// Creates a draft creation request.
    pub fn new(output_dir: impl Into<PathBuf>, author_display_name: impl Into<String>) -> Self {
        Self {
            output_dir: output_dir.into(),
            author_display_name: author_display_name.into(),
            created_local_time: detect_device_created_local_time(),
            id: None,
            force: false,
            cover_path: COVER_PATH.to_string(),
        }
    }

    /// Sets the creation time zone metadata supplied by the creating device.
    pub fn with_created_local_time(mut self, created_local_time: CreatedLocalTime) -> Self {
        self.created_local_time = created_local_time;
        self
    }

    /// Sets the card identifier written to `manifest.json`.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Allows replacing an existing `manifest.json`.
    pub fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    /// Sets the cover path declared in `manifest.json`.
    pub fn with_cover_path(mut self, cover_path: impl Into<String>) -> Self {
        self.cover_path = cover_path.into();
        self
    }
}

/// Request to assemble an EPC archive from an unpacked source directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackRequest {
    /// Directory containing the files to pack.
    pub source_dir: PathBuf,

    /// Destination `.epc` archive path.
    pub output_file: PathBuf,
}

/// Request to sign an unpacked EPC source directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignRequest {
    /// Directory containing the files to sign.
    pub source_dir: PathBuf,

    /// Base64URL, unpadded Ed25519 seed bytes.
    pub secret_seed: String,

    /// Human-facing signer display name.
    pub signer_display_name: String,

    /// Signer role recorded in `proof/signature.json`.
    pub signer_role: String,

    /// Whether an existing `proof/signature.json` should be overwritten.
    pub force: bool,
}

impl SignRequest {
    /// Creates a signing request using signer role `author`.
    pub fn new(
        source_dir: impl Into<PathBuf>,
        secret_seed: impl Into<String>,
        signer_display_name: impl Into<String>,
    ) -> Self {
        Self {
            source_dir: source_dir.into(),
            secret_seed: secret_seed.into(),
            signer_display_name: signer_display_name.into(),
            signer_role: "author".to_string(),
            force: false,
        }
    }

    /// Overrides the signer role recorded in the signature payload.
    pub fn with_signer_role(mut self, signer_role: impl Into<String>) -> Self {
        self.signer_role = signer_role.into();
        self
    }

    /// Allows replacing an existing `proof/signature.json`.
    pub fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }
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

    /// The manifest cannot produce an ADR-003 sealed capsule filename.
    InvalidFilenameMetadata(String),

    /// The signing request or generated signature proof is invalid.
    InvalidSignatureMetadata(String),

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

fn pack_error_to_io(error: PackError) -> io::Error {
    match error {
        PackError::Io(error) => error,
        other => io::Error::new(io::ErrorKind::InvalidData, format!("{other:?}")),
    }
}

/// Creates a new unpacked EPC draft directory.
///
/// This writes a fresh `manifest.json` with a generated `escale:<ULID>` id,
/// current UTC `created_at`, device-local creation metadata, empty `sealed_at`,
/// fixed core-format content paths, and a starter `text/message.md`. Existing
/// media and message files are left untouched. Existing generated proofs are
/// removed because they no longer bind to the new manifest. Existing manifests
/// are overwritten only when `force` is set.
pub fn create_draft_directory(request: CreateDraftRequest) -> Result<PathBuf, PackError> {
    let root = request.output_dir;
    let id = match request.id {
        Some(id) => id,
        None => generate_card_id()?,
    };
    if !is_valid_card_id(&id) {
        return Err(PackError::InvalidFilenameMetadata(
            "card id must use the escale:<26-char-ulid> format".to_string(),
        ));
    }
    if !is_supported_cover_path(&request.cover_path) {
        return Err(PackError::InvalidFilenameMetadata(
            "cover path must be media/cover.jpg, media/cover.jpeg, media/cover.png, media/cover.webp, or media/cover.jxl"
                .to_string(),
        ));
    }

    fs::create_dir_all(root.join("media"))?;
    fs::create_dir_all(root.join("text"))?;

    let mut manifest = Manifest {
        epc_version: epc_core::EPC_VERSION_1_0.to_string(),
        profile: epc_core::CORE_PROFILE.to_string(),
        object_type: epc_core::EPC_OBJECT_TYPE_POSTCARD.to_string(),
        id,
        created_at: current_utc_timestamp()?,
        created_local_time: request.created_local_time,
        sealed_at: String::new(),
        author: epc_core::Author {
            display_name: request.author_display_name,
        },
        content: core_content_manifest(&request.cover_path),
    };
    refresh_manifest_image_metadata_fields(&root, &mut manifest)?;

    let manifest_bytes = serde_json::to_string_pretty(&manifest)?;
    if request.force {
        fs::write(root.join(MANIFEST_PATH), manifest_bytes.as_bytes())?;
    } else {
        write_new_file(&root.join(MANIFEST_PATH), manifest_bytes.as_bytes())?;
    }
    remove_file_if_exists(&root.join(HASHES_PATH))?;
    remove_file_if_exists(&root.join(SIGNATURE_PATH))?;
    write_file_if_missing(&root.join(MESSAGE_PATH), b"")?;
    Ok(root)
}

/// Returns the ADR-003 draft filename for an unpacked draft directory.
///
/// The filename has the form `escale-<ID10>.epc`, derived from
/// `manifest.json` `id`.
pub fn draft_filename_from_directory(root: impl AsRef<Path>) -> Result<String, PackError> {
    let bytes = fs::read(root.as_ref().join(MANIFEST_PATH))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    draft_filename(&manifest)
}

/// Refreshes image metadata in `manifest.json` from the declared media files.
///
/// Missing image files leave their manifest metadata empty, which allows draft
/// initialization before media insertion. Unsupported or malformed image files
/// return an error so callers can report the failed insertion.
pub fn refresh_manifest_image_metadata(root: impl AsRef<Path>) -> Result<(), PackError> {
    let root = root.as_ref();
    let mut manifest = read_manifest(root)?;
    refresh_manifest_image_metadata_fields(root, &mut manifest)?;
    write_manifest(root, &manifest)
}

/// Signs an unpacked EPC directory by writing `proof/signature.json`.
///
/// The source is sealed when needed, `proof/hashes.json` is regenerated, and the
/// Ed25519 signature covers `UTF8("EPC-SIGNATURE-V1\n") || JCS(payload)`.
pub fn sign_core_format_directory(request: SignRequest) -> Result<PathBuf, PackError> {
    let source_dir = request.source_dir.clone();
    let output_file = source_dir.join(SIGNATURE_PATH);
    if output_file.exists() && !request.force {
        return Err(PackError::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("signature proof already exists: {}", output_file.display()),
        )));
    }
    if request.force {
        let _ = fs::remove_file(&output_file);
    }
    let manifest = seal_manifest_if_needed(&source_dir)?;
    write_manifest(&source_dir, &manifest)?;
    write_hashes(&source_dir)?;

    let report = validate_core_directory(&source_dir);
    if !report.is_valid() {
        return Err(PackError::InvalidSource(report));
    }

    let hashes = read_hashes(&source_dir)?;
    let proof = signature_proof(&manifest, &hashes, &request)?;
    if let Some(parent) = output_file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output_file, serde_json::to_string_pretty(&proof)?)?;
    Ok(output_file)
}

/// Signs an unpacked EPC directory using an OpenSSH Ed25519 private key file.
///
/// The signer display name is taken from `manifest.json` `author.display_name`.
/// Encrypted OpenSSH private keys are not supported by this low-level helper.
pub fn sign_core_format_directory_with_ssh_key(
    source_dir: impl AsRef<Path>,
    private_key_file: impl AsRef<Path>,
    force: bool,
) -> Result<PathBuf, PackError> {
    let source_dir = source_dir.as_ref();
    let manifest = read_manifest(source_dir)?;
    let seed = read_openssh_ed25519_seed(private_key_file.as_ref())?;
    let request = SignRequest::new(
        source_dir,
        URL_SAFE_NO_PAD.encode(seed),
        manifest.author.display_name,
    )
    .with_force(force);
    sign_core_format_directory(request)
}

/// Packs a source directory into a valid EPC 1.0 `core-format` archive.
///
/// The source directory must contain `manifest.json`, one supported
/// `media/cover.*` image, `media/thumbnail.jxl`, and `text/message.md`.
/// `proof/hashes.json` is
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

/// Packs a source directory into `output_dir` using the ADR-003 sealed filename.
///
/// This writes `manifest.json` `sealed_at` to the current UTC time when the
/// source is still an unsealed draft. Already sealed sources keep their existing
/// timestamp, so repeated packs use a stable filename. The generated filename
/// has the form `<TIME6>-<ID10>.epc`, derived from the sealed manifest. The
/// final path is returned on success.
pub fn pack_core_format_to_directory(
    source_dir: impl AsRef<Path>,
    output_dir: impl AsRef<Path>,
) -> Result<PathBuf, PackError> {
    pack_core_format_to_directory_with_options(source_dir, output_dir, ValidationOptions::default())
}

fn pack_core_format_to_directory_with_options(
    source_dir: impl AsRef<Path>,
    output_dir: impl AsRef<Path>,
    validation_options: ValidationOptions,
) -> Result<PathBuf, PackError> {
    let source_dir = source_dir.as_ref();
    let output_dir = output_dir.as_ref();
    let staging = TempDir::new("epc-pack");
    copy_core_source(source_dir, staging.path())?;
    let sealed_manifest = seal_manifest_if_needed(staging.path())?;
    write_hashes(staging.path())?;

    let output_file = output_dir.join(sealed_filename_from_manifest(staging.path())?);
    if output_file.exists() {
        return Err(PackError::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("output file already exists: {}", output_file.display()),
        )));
    }

    let report = validate_core_directory_with_options(staging.path(), validation_options);
    if !report.is_valid() {
        return Err(PackError::InvalidSource(report));
    }

    write_manifest(source_dir, &sealed_manifest)?;
    write_zip(staging.path(), &output_file)?;
    Ok(output_file)
}

/// Signs a source directory with an OpenSSH Ed25519 private key and packs it.
pub fn pack_core_format_to_directory_signed(
    source_dir: impl AsRef<Path>,
    output_dir: impl AsRef<Path>,
    private_key_file: impl AsRef<Path>,
    force_signature: bool,
) -> Result<PathBuf, PackError> {
    let source_dir = source_dir.as_ref();
    let refreshed_signature = force_signature || !source_dir.join(SIGNATURE_PATH).exists();
    if refreshed_signature {
        sign_core_format_directory_with_ssh_key(source_dir, private_key_file.as_ref(), true)?;
    }
    let validation_options = if refreshed_signature {
        ValidationOptions::default().without_jxl_images()
    } else {
        ValidationOptions::default()
    };
    pack_core_format_to_directory_with_options(source_dir, output_dir, validation_options)
}

fn seal_manifest_if_needed(root: &Path) -> Result<Manifest, PackError> {
    let path = root.join(MANIFEST_PATH);
    let mut manifest = read_manifest(root)?;
    if manifest.sealed_at.is_empty() {
        manifest.sealed_at = current_utc_timestamp()?;
        fs::write(path, serde_json::to_string_pretty(&manifest)?)?;
    }
    Ok(manifest)
}

fn read_manifest(root: &Path) -> Result<Manifest, PackError> {
    let bytes = fs::read(root.join(MANIFEST_PATH))?;
    let mut value: Value = serde_json::from_slice(&bytes)?;
    complete_manifest_defaults(&mut value)?;
    Ok(serde_json::from_value(value)?)
}

fn write_manifest(root: &Path, manifest: &Manifest) -> Result<(), PackError> {
    fs::write(
        root.join(MANIFEST_PATH),
        serde_json::to_string_pretty(manifest)?,
    )?;
    Ok(())
}

fn complete_manifest_defaults(value: &mut Value) -> Result<(), PackError> {
    let Value::Object(object) = value else {
        return Ok(());
    };

    insert_missing_string(object, "epc_version", epc_core::EPC_VERSION_1_0);
    insert_missing_string(object, "profile", epc_core::CORE_PROFILE);
    insert_missing_string(object, "type", epc_core::EPC_OBJECT_TYPE_POSTCARD);
    if !object.contains_key("id") {
        object.insert("id".to_string(), Value::String(generate_card_id()?));
    }
    if !object.contains_key("created_at") {
        object.insert(
            "created_at".to_string(),
            Value::String(current_utc_timestamp()?),
        );
    }
    let mut created_local_time = serde_json::to_value(detect_device_created_local_time())?;
    if let Some(existing_created_local_time) = object.get_mut("created_local_time") {
        merge_missing(existing_created_local_time, &mut created_local_time);
    } else {
        object.insert("created_local_time".to_string(), created_local_time);
    }
    insert_missing_string(object, "sealed_at", "");

    let mut content = serde_json::to_value(core_content_manifest(COVER_PATH))?;
    if let Some(existing_content) = object.get_mut("content") {
        merge_missing(existing_content, &mut content);
    } else {
        object.insert("content".to_string(), content);
    }

    Ok(())
}

fn insert_missing_string(object: &mut Map<String, Value>, key: &str, value: &str) {
    object
        .entry(key.to_string())
        .or_insert_with(|| Value::String(value.to_string()));
}

fn merge_missing(target: &mut Value, defaults: &mut Value) {
    let (Value::Object(target), Value::Object(defaults)) = (target, defaults) else {
        return;
    };

    for (key, default_value) in defaults {
        if let Some(target_value) = target.get_mut(key) {
            merge_missing(target_value, default_value);
        } else {
            target.insert(key.clone(), default_value.take());
        }
    }
}

fn core_content_manifest(cover_path: &str) -> epc_core::Content {
    epc_core::Content {
        cover: epc_core::MediaContent {
            path: cover_path.to_string(),
            mime: cover_mime_for_path(cover_path)
                .unwrap_or("image/jxl")
                .to_string(),
            image: None,
        },
        thumbnail: epc_core::MediaContent {
            path: THUMBNAIL_PATH.to_string(),
            mime: "image/jxl".to_string(),
            image: None,
        },
        message: epc_core::MessageContent {
            path: MESSAGE_PATH.to_string(),
            mime: "text/markdown".to_string(),
            markdown_profile: epc_core::MARKDOWN_CORE_PROFILE.to_string(),
            markdown_profile_version: epc_core::MARKDOWN_CORE_PROFILE_VERSION.to_string(),
        },
    }
}

fn refresh_manifest_image_metadata_fields(
    root: &Path,
    manifest: &mut Manifest,
) -> Result<(), PackError> {
    manifest.content.cover.image =
        read_optional_image_metadata(root, &manifest.content.cover.path)?;
    manifest.content.thumbnail.image =
        read_optional_image_metadata(root, &manifest.content.thumbnail.path)?;
    Ok(())
}

fn read_optional_image_metadata(
    root: &Path,
    content_path: &str,
) -> Result<Option<epc_core::ImageMetadata>, PackError> {
    let path = root.join(content_path);
    if !path.is_file() {
        return Ok(None);
    }
    epc_image::read_image_metadata_file(&path)
        .map(Some)
        .map_err(image_metadata_error_to_pack_error)
}

fn image_metadata_error_to_pack_error(error: ImageMetadataError) -> PackError {
    match error {
        ImageMetadataError::Io(error) => PackError::Io(error),
        other => {
            PackError::InvalidFilenameMetadata(format!("image metadata cannot be read: {other:?}"))
        }
    }
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), PackError> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn write_file_if_missing(path: &Path, bytes: &[u8]) -> Result<(), PackError> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(bytes)?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(PackError::Io(error)),
    }
}

fn remove_file_if_exists(path: &Path) -> Result<(), PackError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PackError::Io(error)),
    }
}

/// Generates the official `core-format` conformance test vector set.
///
/// `output_root` is the `test-vectors/core-format` directory. The function
/// writes valid archives under `valid/`, warning archives under `warning/`,
/// invalid archives under `invalid/`, and expected validation reports under
/// `reports/warning/` and `reports/invalid/`.
pub fn generate_core_format_test_vectors(output_root: impl AsRef<Path>) -> Result<(), PackError> {
    let output_root = output_root.as_ref();
    let valid_dir = output_root.join("valid");
    let warning_dir = output_root.join("warning");
    let invalid_dir = output_root.join("invalid");
    let warning_reports_dir = output_root.join("reports").join("warning");
    let invalid_reports_dir = output_root.join("reports").join("invalid");

    fs::create_dir_all(&valid_dir)?;
    fs::create_dir_all(&warning_dir)?;
    fs::create_dir_all(&invalid_dir)?;
    fs::create_dir_all(&warning_reports_dir)?;
    fs::create_dir_all(&invalid_reports_dir)?;

    generate_valid_minimal(&valid_dir.join("minimal.epc"))?;
    generate_valid_max_limits(&valid_dir.join("max-limits.epc"))?;
    generate_valid_with_directory_entries(&valid_dir.join("with-directory-entries.epc"))?;

    generate_warning_empty_message(&warning_dir.join("empty-message.epc"))?;
    write_expected_report(
        &warning_dir.join("empty-message.epc"),
        &warning_reports_dir.join("empty-message.json"),
    )?;

    generate_warning_empty_cover(&warning_dir.join("empty-cover.epc"))?;
    write_expected_report(
        &warning_dir.join("empty-cover.epc"),
        &warning_reports_dir.join("empty-cover.json"),
    )?;

    generate_warning_empty_thumbnail(&warning_dir.join("empty-thumbnail.epc"))?;
    write_expected_report(
        &warning_dir.join("empty-thumbnail.epc"),
        &warning_reports_dir.join("empty-thumbnail.json"),
    )?;

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

    generate_invalid_missing_thumbnail(&invalid_dir.join("missing-thumbnail.epc"))?;
    write_expected_report(
        &invalid_dir.join("missing-thumbnail.epc"),
        &invalid_reports_dir.join("missing-thumbnail.json"),
    )?;

    generate_invalid_missing_message(&invalid_dir.join("missing-message.epc"))?;
    write_expected_report(
        &invalid_dir.join("missing-message.epc"),
        &invalid_reports_dir.join("missing-message.json"),
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

    generate_invalid_signature_json(&invalid_dir.join("invalid-signature-json.epc"))?;
    write_expected_report(
        &invalid_dir.join("invalid-signature-json.epc"),
        &invalid_reports_dir.join("invalid-signature-json.json"),
    )?;

    generate_invalid_signature_card_id_mismatch(
        &invalid_dir.join("signature-card-id-mismatch.epc"),
    )?;
    write_expected_report(
        &invalid_dir.join("signature-card-id-mismatch.epc"),
        &invalid_reports_dir.join("signature-card-id-mismatch.json"),
    )?;

    generate_invalid_signature_core_digest_mismatch(
        &invalid_dir.join("signature-core-digest-mismatch.epc"),
    )?;
    write_expected_report(
        &invalid_dir.join("signature-core-digest-mismatch.epc"),
        &invalid_reports_dir.join("signature-core-digest-mismatch.json"),
    )?;

    generate_invalid_signature_required_key_missing(
        &invalid_dir.join("signature-required-key-missing.epc"),
    )?;
    write_expected_report(
        &invalid_dir.join("signature-required-key-missing.epc"),
        &invalid_reports_dir.join("signature-required-key-missing.json"),
    )?;

    generate_invalid_signature_unsupported_algorithm(
        &invalid_dir.join("signature-unsupported-algorithm.epc"),
    )?;
    write_expected_report(
        &invalid_dir.join("signature-unsupported-algorithm.epc"),
        &invalid_reports_dir.join("signature-unsupported-algorithm.json"),
    )?;

    Ok(())
}

fn copy_core_source(source: &Path, staging: &Path) -> io::Result<()> {
    for directory in ["media", "text", "proof"] {
        fs::create_dir_all(staging.join(directory))?;
    }

    let manifest = read_manifest(source).map_err(pack_error_to_io)?;
    let cover_path = manifest.content.cover.path.as_str();

    for path in [MANIFEST_PATH, cover_path, THUMBNAIL_PATH, MESSAGE_PATH] {
        let destination = staging.join(path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source.join(path), destination)?;
    }

    let signature = source.join(SIGNATURE_PATH);
    if signature.exists() {
        fs::copy(signature, staging.join(SIGNATURE_PATH))?;
    }

    Ok(())
}

fn write_hashes(root: &Path) -> Result<(), PackError> {
    let mut hashes = compute_hashes(root)?;
    hashes.core_digest = compute_core_digest(&hashes);
    let json = serde_json::to_string_pretty(&hashes)?;
    let path = root.join(HASHES_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, json)?;
    Ok(())
}

fn write_signature_proof(root: &Path, proof: SignatureProof) -> Result<(), PackError> {
    let json = serde_json::to_string_pretty(&proof)?;
    let path = root.join(SIGNATURE_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, json)?;
    Ok(())
}

fn compute_hashes(root: &Path) -> Result<Hashes, PackError> {
    let manifest = read_manifest(root)?;
    let entries = [
        (MANIFEST_PATH, HashTransform::Jcs),
        (
            manifest.content.cover.path.as_str(),
            HashTransform::Identity,
        ),
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

fn read_hashes(root: &Path) -> Result<Hashes, PackError> {
    let bytes = fs::read(root.join(HASHES_PATH))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn signature_proof(
    manifest: &Manifest,
    hashes: &Hashes,
    request: &SignRequest,
) -> Result<SignatureProof, PackError> {
    let seed = decode_ed25519_seed(&request.secret_seed)?;
    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();
    let public_key_x = URL_SAFE_NO_PAD.encode(verifying_key.as_bytes());
    let key_id = ed25519_jwk_thumbprint(&public_key_x);

    let payload = SignaturePayload {
        context: "EPC-SIGNATURE-V1".to_string(),
        card_id: manifest.id.clone(),
        epc_version: manifest.epc_version.clone(),
        core_digest: hashes.core_digest.clone(),
        hash_algorithm: hashes.hash_algorithm.clone(),
        signed_at: current_utc_timestamp()?,
        signer: SignatureSigner {
            display_name: request.signer_display_name.clone(),
            role: request.signer_role.clone(),
        },
        policy: SignaturePolicy {
            mode: "all".to_string(),
            required_keys: vec![SignatureRequiredKey {
                algorithm: "Ed25519".to_string(),
                key_id: key_id.clone(),
            }],
        },
    };

    let mut signature_input = Vec::new();
    signature_input.extend_from_slice(SIGNATURE_DOMAIN_SEPARATOR.as_bytes());
    signature_input
        .extend_from_slice(canonical_json(&signature_payload_value(&payload)).as_bytes());
    let signature = signing_key.sign(&signature_input);

    Ok(SignatureProof {
        signature_version: "1".to_string(),
        payload,
        signatures: vec![SignatureEntry {
            algorithm: "Ed25519".to_string(),
            key_id,
            public_key: SignaturePublicKey {
                kty: "OKP".to_string(),
                crv: "Ed25519".to_string(),
                x: public_key_x,
            },
            value: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        }],
    })
}

fn deterministic_signature_proof(
    manifest: &Manifest,
    hashes: &Hashes,
) -> Result<SignatureProof, PackError> {
    let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
    let verifying_key = signing_key.verifying_key();
    let public_key_x = URL_SAFE_NO_PAD.encode(verifying_key.as_bytes());
    let key_id = ed25519_jwk_thumbprint(&public_key_x);

    let payload = SignaturePayload {
        context: "EPC-SIGNATURE-V1".to_string(),
        card_id: manifest.id.clone(),
        epc_version: manifest.epc_version.clone(),
        core_digest: hashes.core_digest.clone(),
        hash_algorithm: hashes.hash_algorithm.clone(),
        signed_at: "2026-06-17T10:06:00Z".to_string(),
        signer: SignatureSigner {
            display_name: "Bruno".to_string(),
            role: "author".to_string(),
        },
        policy: SignaturePolicy {
            mode: "all".to_string(),
            required_keys: vec![SignatureRequiredKey {
                algorithm: "Ed25519".to_string(),
                key_id: key_id.clone(),
            }],
        },
    };

    let mut signature_input = Vec::new();
    signature_input.extend_from_slice(SIGNATURE_DOMAIN_SEPARATOR.as_bytes());
    signature_input
        .extend_from_slice(canonical_json(&signature_payload_value(&payload)).as_bytes());
    let signature = signing_key.sign(&signature_input);

    Ok(SignatureProof {
        signature_version: "1".to_string(),
        payload,
        signatures: vec![SignatureEntry {
            algorithm: "Ed25519".to_string(),
            key_id,
            public_key: SignaturePublicKey {
                kty: "OKP".to_string(),
                crv: "Ed25519".to_string(),
                x: public_key_x,
            },
            value: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        }],
    })
}

fn decode_ed25519_seed(value: &str) -> Result<[u8; 32], PackError> {
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| {
        PackError::InvalidSignatureMetadata(
            "secret seed must be Base64URL without padding".to_string(),
        )
    })?;
    bytes.try_into().map_err(|_| {
        PackError::InvalidSignatureMetadata(
            "secret seed must decode to exactly 32 bytes".to_string(),
        )
    })
}

fn read_openssh_ed25519_seed(path: &Path) -> Result<[u8; 32], PackError> {
    let pem = fs::read_to_string(path)?;
    let body = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<String>();
    let bytes = STANDARD.decode(body).map_err(|_| {
        PackError::InvalidSignatureMetadata(
            "OpenSSH private key is not valid PEM Base64".to_string(),
        )
    })?;
    parse_openssh_ed25519_seed(&bytes)
}

fn parse_openssh_ed25519_seed(bytes: &[u8]) -> Result<[u8; 32], PackError> {
    const AUTH_MAGIC: &[u8] = b"openssh-key-v1\0";

    if !bytes.starts_with(AUTH_MAGIC) {
        return Err(PackError::InvalidSignatureMetadata(
            "private key must use the OpenSSH private key format".to_string(),
        ));
    }

    let mut offset = AUTH_MAGIC.len();
    let ciphername = read_openssh_string(bytes, &mut offset)?;
    let kdfname = read_openssh_string(bytes, &mut offset)?;
    let _kdfoptions = read_openssh_string(bytes, &mut offset)?;
    let key_count = read_openssh_u32(bytes, &mut offset)?;

    if ciphername != b"none" || kdfname != b"none" {
        return Err(PackError::InvalidSignatureMetadata(
            "encrypted OpenSSH private keys are not supported yet".to_string(),
        ));
    }
    if key_count != 1 {
        return Err(PackError::InvalidSignatureMetadata(
            "OpenSSH private key must contain exactly one key".to_string(),
        ));
    }

    let _public_key = read_openssh_string(bytes, &mut offset)?;
    let private_blob = read_openssh_string(bytes, &mut offset)?;
    let mut private_offset = 0;
    let check1 = read_openssh_u32(private_blob, &mut private_offset)?;
    let check2 = read_openssh_u32(private_blob, &mut private_offset)?;
    if check1 != check2 {
        return Err(PackError::InvalidSignatureMetadata(
            "OpenSSH private key checkints do not match".to_string(),
        ));
    }

    let key_type = read_openssh_string(private_blob, &mut private_offset)?;
    let _public_key = read_openssh_string(private_blob, &mut private_offset)?;
    let private_key = read_openssh_string(private_blob, &mut private_offset)?;

    if key_type != b"ssh-ed25519" {
        return Err(PackError::InvalidSignatureMetadata(
            "OpenSSH private key must be ssh-ed25519".to_string(),
        ));
    }
    if private_key.len() != 64 {
        return Err(PackError::InvalidSignatureMetadata(
            "OpenSSH Ed25519 private key must contain 64 seed+public bytes".to_string(),
        ));
    }

    let mut seed = [0_u8; 32];
    seed.copy_from_slice(&private_key[..32]);
    Ok(seed)
}

fn read_openssh_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, PackError> {
    let end = offset.saturating_add(4);
    let value = bytes.get(*offset..end).ok_or_else(|| {
        PackError::InvalidSignatureMetadata("truncated OpenSSH private key".to_string())
    })?;
    *offset = end;
    Ok(u32::from_be_bytes(
        value.try_into().expect("slice has exactly 4 bytes"),
    ))
}

fn read_openssh_string<'a>(bytes: &'a [u8], offset: &mut usize) -> Result<&'a [u8], PackError> {
    let len = read_openssh_u32(bytes, offset)? as usize;
    let end = offset.checked_add(len).ok_or_else(|| {
        PackError::InvalidSignatureMetadata("invalid OpenSSH private key length".to_string())
    })?;
    let value = bytes.get(*offset..end).ok_or_else(|| {
        PackError::InvalidSignatureMetadata("truncated OpenSSH private key".to_string())
    })?;
    *offset = end;
    Ok(value)
}

fn ed25519_jwk_thumbprint(public_key_x: &str) -> String {
    let jwk = serde_json::json!({
        "crv": "Ed25519",
        "kty": "OKP",
        "x": public_key_x,
    });
    URL_SAFE_NO_PAD.encode(sha256(canonical_json(&jwk).as_bytes()))
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

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_file)?;
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

    let manifest = read_manifest(root)?;
    let mut paths = vec![
        MANIFEST_PATH,
        manifest.content.cover.path.as_str(),
        THUMBNAIL_PATH,
        MESSAGE_PATH,
        HASHES_PATH,
    ];
    if root.join(SIGNATURE_PATH).exists() {
        paths.push(SIGNATURE_PATH);
    }

    for path in paths {
        zip.start_file(path, file_options)?;
        let bytes = fs::read(root.join(path))?;
        zip.write_all(&bytes)?;
    }

    zip.finish()?;
    Ok(())
}

fn sealed_filename_from_manifest(root: &Path) -> Result<String, PackError> {
    let bytes = fs::read(root.join(MANIFEST_PATH))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    sealed_filename(&manifest)
}

fn sealed_filename(manifest: &Manifest) -> Result<String, PackError> {
    let Some(card_id) = manifest.id.strip_prefix("escale:") else {
        return Err(PackError::InvalidFilenameMetadata(
            "card id must use the canonical escale:<ULID> form".to_string(),
        ));
    };

    if !is_valid_card_id(&manifest.id) {
        return Err(PackError::InvalidFilenameMetadata(
            "card id must use the canonical escale:<ULID> form".to_string(),
        ));
    }

    let minutes = sealed_minutes_since_epoch(&manifest.sealed_at)?;
    let time6 = crockford_base32_fixed(minutes, 6)?;
    let id10 = &card_id[card_id.len() - 10..];
    Ok(format!("{time6}-{id10}.epc"))
}

fn draft_filename(manifest: &Manifest) -> Result<String, PackError> {
    let Some(card_id) = manifest.id.strip_prefix("escale:") else {
        return Err(PackError::InvalidFilenameMetadata(
            "card id must use the canonical escale:<ULID> form".to_string(),
        ));
    };

    if !is_valid_card_id(&manifest.id) {
        return Err(PackError::InvalidFilenameMetadata(
            "card id must use the canonical escale:<ULID> form".to_string(),
        ));
    }

    let id10 = &card_id[card_id.len() - 10..];
    Ok(format!("escale-{id10}.epc"))
}

fn current_utc_timestamp() -> Result<String, PackError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            PackError::InvalidFilenameMetadata(
                "system clock must not be before the Unix epoch".to_string(),
            )
        })?
        .as_secs() as i64;
    Ok(format_utc_timestamp(seconds))
}

/// Returns best-effort device-local creation metadata for the current process.
///
/// Mobile or embedded callers should prefer `CreateDraftRequest::with_created_local_time`
/// and pass values read from the device OS at the moment the capsule is created.
pub fn detect_device_created_local_time() -> CreatedLocalTime {
    CreatedLocalTime {
        time_zone: detect_device_time_zone(),
        utc_offset: detect_device_utc_offset(),
    }
}

fn detect_device_time_zone() -> String {
    std::env::var("ESCALE_DEVICE_TIME_ZONE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("TZ")
                .ok()
                .map(|value| value.trim_start_matches(':').to_string())
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            fs::read_link("/etc/localtime").ok().and_then(|path| {
                let text = path.to_string_lossy();
                text.split("zoneinfo/")
                    .nth(1)
                    .map(str::to_string)
                    .filter(|value| !value.trim().is_empty())
            })
        })
        .unwrap_or_else(|| "Etc/UTC".to_string())
}

fn detect_device_utc_offset() -> String {
    std::env::var("ESCALE_DEVICE_UTC_OFFSET")
        .ok()
        .and_then(|value| normalize_utc_offset(&value))
        .or_else(|| {
            Command::new("date")
                .arg("+%z")
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .and_then(|value| normalize_utc_offset(value.trim()))
        })
        .unwrap_or_else(|| "+00:00".to_string())
}

fn normalize_utc_offset(value: &str) -> Option<String> {
    if value.len() == 6
        && matches!(value.as_bytes()[0], b'+' | b'-')
        && &value[3..4] == ":"
        && is_normalized_utc_offset(value)
    {
        return Some(value.to_string());
    }
    if value.len() == 5 && matches!(value.as_bytes()[0], b'+' | b'-') {
        let hours = &value[1..3];
        let minutes = &value[3..5];
        if hours.bytes().all(|byte| byte.is_ascii_digit())
            && minutes.bytes().all(|byte| byte.is_ascii_digit())
        {
            let normalized = format!("{}{}:{}", &value[..1], hours, minutes);
            if is_normalized_utc_offset(&normalized) {
                return Some(normalized);
            }
        }
    }
    None
}

fn is_normalized_utc_offset(value: &str) -> bool {
    let Ok(hour) = value[1..3].parse::<u8>() else {
        return false;
    };
    let Ok(minute) = value[4..6].parse::<u8>() else {
        return false;
    };
    hour <= 23 && minute <= 59
}

/// Generates a new EPC card identifier using an ULID-compatible payload.
pub fn generate_card_id() -> Result<String, PackError> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
        PackError::InvalidFilenameMetadata(
            "system clock must not be before the Unix epoch".to_string(),
        )
    })?;
    let millis = duration.as_millis() as u64;

    let mut bytes = [0_u8; 16];
    bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
    fill_random_bytes(&mut bytes[6..])?;

    Ok(format!("escale:{}", encode_ulid_bytes(bytes)))
}

fn fill_random_bytes(bytes: &mut [u8]) -> Result<(), PackError> {
    match File::open("/dev/urandom").and_then(|mut file| file.read_exact(bytes)) {
        Ok(()) => Ok(()),
        Err(_) => {
            let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| {
                    PackError::InvalidFilenameMetadata(
                        "system clock must not be before the Unix epoch".to_string(),
                    )
                })?
                .as_nanos();
            let mut hasher = Sha256::new();
            hasher.update(now.to_be_bytes());
            hasher.update(counter.to_be_bytes());
            let digest = hasher.finalize();
            bytes.copy_from_slice(&digest[..bytes.len()]);
            Ok(())
        }
    }
}

fn encode_ulid_bytes(bytes: [u8; 16]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut value = u128::from_be_bytes(bytes);
    let mut output = [b'0'; 26];
    for byte in output.iter_mut().rev() {
        *byte = ALPHABET[(value & 0b1_1111) as usize];
        value >>= 5;
    }
    String::from_utf8(output.to_vec()).expect("Crockford alphabet is valid UTF-8")
}

fn format_utc_timestamp(seconds_since_epoch: i64) -> String {
    let days = seconds_since_epoch.div_euclid(86_400);
    let seconds_of_day = seconds_since_epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn sealed_minutes_since_epoch(value: &str) -> Result<u64, PackError> {
    let Some(value) = value.strip_suffix('Z') else {
        return Err(PackError::InvalidFilenameMetadata(
            "sealed_at must be a UTC RFC 3339 timestamp ending in Z".to_string(),
        ));
    };
    let Some((date, time)) = value.split_once('T') else {
        return Err(PackError::InvalidFilenameMetadata(
            "sealed_at must contain a UTC date and time".to_string(),
        ));
    };

    let mut date_parts = date.split('-');
    let year = parse_fixed_u64(date_parts.next(), "sealed_at year", 4)? as i32;
    let month = parse_fixed_u64(date_parts.next(), "sealed_at month", 2)? as u32;
    let day = parse_fixed_u64(date_parts.next(), "sealed_at day", 2)? as u32;
    if date_parts.next().is_some() {
        return Err(PackError::InvalidFilenameMetadata(
            "sealed_at date must use YYYY-MM-DD".to_string(),
        ));
    }

    let mut time_parts = time.split(':');
    let hour = parse_fixed_u64(time_parts.next(), "sealed_at hour", 2)?;
    let minute = parse_fixed_u64(time_parts.next(), "sealed_at minute", 2)?;
    let second = parse_fixed_u64(time_parts.next(), "sealed_at second", 2)?;
    if time_parts.next().is_some() {
        return Err(PackError::InvalidFilenameMetadata(
            "sealed_at time must use HH:MM:SS".to_string(),
        ));
    }

    let max_day = days_in_month(year, month);
    if !(1..=12).contains(&month)
        || day == 0
        || day > max_day
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(PackError::InvalidFilenameMetadata(
            "sealed_at contains an out-of-range date or time component".to_string(),
        ));
    }

    let days = days_from_civil(year, month, day);
    let epoch_days = days_from_civil(2026, 1, 1);
    if days < epoch_days {
        return Err(PackError::InvalidFilenameMetadata(
            "sealed_at must not be before 2026-01-01T00:00:00Z".to_string(),
        ));
    }

    Ok(((days - epoch_days) as u64 * 24 * 60) + hour * 60 + minute)
}

fn parse_fixed_u64(value: Option<&str>, label: &str, width: usize) -> Result<u64, PackError> {
    let value = value.ok_or_else(|| {
        PackError::InvalidFilenameMetadata(format!("{label} is missing from sealed_at"))
    })?;
    if value.len() != width {
        return Err(PackError::InvalidFilenameMetadata(format!(
            "{label} must be {width} digits"
        )));
    }
    value
        .parse::<u64>()
        .map_err(|_| PackError::InvalidFilenameMetadata(format!("{label} is not a decimal number")))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn crockford_base32_fixed(mut value: u64, width: usize) -> Result<String, PackError> {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut buffer = vec![b'0'; width];
    for byte in buffer.iter_mut().rev() {
        *byte = ALPHABET[(value % 32) as usize];
        value /= 32;
    }
    if value != 0 {
        return Err(PackError::InvalidFilenameMetadata(
            "sealed_at is too far in the future for TIME6".to_string(),
        ));
    }
    Ok(String::from_utf8(buffer).expect("Crockford alphabet is valid UTF-8"))
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

fn write_zip_with_signature(root: &Path, output_file: &Path) -> Result<(), PackError> {
    write_zip_with_entries(
        root,
        output_file,
        false,
        &[
            MANIFEST_PATH,
            COVER_PATH,
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
            SIGNATURE_PATH,
        ],
        &[],
    )
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

fn generate_invalid_missing_thumbnail(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-missing-thumbnail");
    write_vector_source(
        source.path(),
        "escale:0000000000000000000000000B",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[MANIFEST_PATH, COVER_PATH, MESSAGE_PATH, HASHES_PATH],
        &[],
    )
}

fn generate_invalid_missing_message(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-missing-message");
    write_vector_source(
        source.path(),
        "escale:0000000000000000000000000C",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    write_zip_with_entries(
        source.path(),
        output_file,
        false,
        &[MANIFEST_PATH, COVER_PATH, THUMBNAIL_PATH, HASHES_PATH],
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

fn generate_warning_empty_message(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-empty-message");
    write_vector_source(
        source.path(),
        "escale:0000000000000000000000000D",
        MessageKind::Minimal,
    )?;
    fs::write(source.path().join(MESSAGE_PATH), [])?;
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

fn generate_warning_empty_cover(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-empty-cover");
    write_vector_source(
        source.path(),
        "escale:0000000000000000000000000E",
        MessageKind::Minimal,
    )?;
    fs::write(source.path().join(COVER_PATH), [])?;
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

fn generate_warning_empty_thumbnail(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-empty-thumbnail");
    write_vector_source(
        source.path(),
        "escale:0000000000000000000000000F",
        MessageKind::Minimal,
    )?;
    fs::write(source.path().join(THUMBNAIL_PATH), [])?;
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
        &[
            ("extra.txt", b"too many entries"),
            ("extra-2.txt", b"too many entries"),
        ],
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

fn generate_invalid_signature_json(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-invalid-signature-json");
    write_vector_source(
        source.path(),
        "escale:0000000000000000000000000G",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    fs::write(source.path().join(SIGNATURE_PATH), b"{not json")?;
    write_zip_with_signature(source.path(), output_file)
}

fn generate_invalid_signature_card_id_mismatch(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-signature-card-id-mismatch");
    write_vector_source(
        source.path(),
        "escale:0000000000000000000000000H",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    let mut manifest = read_manifest(source.path())?;
    let hashes = read_hashes(source.path())?;
    manifest.id = "escale:0000000000000000000000000Z".to_string();
    write_signature_proof(
        source.path(),
        deterministic_signature_proof(&manifest, &hashes)?,
    )?;
    write_zip_with_signature(source.path(), output_file)
}

fn generate_invalid_signature_core_digest_mismatch(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-signature-core-digest-mismatch");
    write_vector_source(
        source.path(),
        "escale:0000000000000000000000000J",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    let manifest = read_manifest(source.path())?;
    let mut hashes = read_hashes(source.path())?;
    hashes.core_digest = URL_SAFE_NO_PAD.encode([9_u8; 32]);
    write_signature_proof(
        source.path(),
        deterministic_signature_proof(&manifest, &hashes)?,
    )?;
    write_zip_with_signature(source.path(), output_file)
}

fn generate_invalid_signature_required_key_missing(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-signature-required-key-missing");
    write_vector_source(
        source.path(),
        "escale:0000000000000000000000000K",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    let manifest = read_manifest(source.path())?;
    let hashes = read_hashes(source.path())?;
    let mut proof = deterministic_signature_proof(&manifest, &hashes)?;
    proof.signatures.clear();
    write_signature_proof(source.path(), proof)?;
    write_zip_with_signature(source.path(), output_file)
}

fn generate_invalid_signature_unsupported_algorithm(output_file: &Path) -> Result<(), PackError> {
    let source = TempDir::new("epc-vector-signature-unsupported-algorithm");
    write_vector_source(
        source.path(),
        "escale:0000000000000000000000000M",
        MessageKind::Minimal,
    )?;
    write_hashes(source.path())?;
    let manifest = read_manifest(source.path())?;
    let hashes = read_hashes(source.path())?;
    let mut proof = deterministic_signature_proof(&manifest, &hashes)?;
    proof.payload.policy.required_keys[0].algorithm = "ML-DSA-65".to_string();
    proof.signatures[0].algorithm = "ML-DSA-65".to_string();
    write_signature_proof(source.path(), proof)?;
    write_zip_with_signature(source.path(), output_file)
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
        "created_local_time": {
            "time_zone": "Europe/Paris",
            "utc_offset": "+02:00"
        },
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
    write_sample_jxl_files(root)?;
    match kind {
        MessageKind::Max => write_max_markdown(&root.join(MESSAGE_PATH))?,
        MessageKind::Minimal | MessageKind::UnsupportedMarkdownProfile => {
            fs::write(root.join(MESSAGE_PATH), "Hello **Escale**.\n")?;
        }
    }

    Ok(())
}

#[cfg(any(test, feature = "test-vectors"))]
fn write_sample_jxl_files(root: &Path) -> io::Result<()> {
    fs::write(
        root.join(COVER_PATH),
        include_bytes!("../../../testcases/images/arc-de-triomphe-paris.jxl"),
    )?;
    fs::write(
        root.join(THUMBNAIL_PATH),
        include_bytes!("../../../testcases/images/thumbnail-256.jxl"),
    )?;
    Ok(())
}

#[cfg(not(any(test, feature = "test-vectors")))]
fn write_sample_jxl_files(_root: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "test vector fixtures are not embedded; rebuild epc-pack with the `test-vectors` feature",
    ))
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

fn signature_payload_value(payload: &SignaturePayload) -> Value {
    serde_json::to_value(payload).expect("signature payload serialization cannot fail")
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
    use epc_validate::{validate_core_directory, validate_epc_file};
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

    #[test]
    fn creates_draft_with_generated_id_and_created_timestamp() {
        let draft = TestDir::new();
        let request = CreateDraftRequest::new(draft.path(), "Bruno");

        create_draft_directory(request).unwrap();

        let manifest_bytes = fs::read(draft.path().join(MANIFEST_PATH)).unwrap();
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes).unwrap();
        assert!(epc_core::is_valid_card_id(&manifest.id));
        assert!(manifest.created_at.ends_with('Z'));
        assert!(manifest.created_at.contains('T'));
        assert!(!manifest.created_local_time.time_zone.is_empty());
        assert!(manifest.created_local_time.utc_offset.contains(':'));
        assert_eq!(manifest.sealed_at, "");
        assert_eq!(manifest.author.display_name, "Bruno");
        assert!(draft.path().join(MESSAGE_PATH).exists());
        assert!(draft.path().join("media").is_dir());
    }

    #[test]
    fn create_removes_stale_generated_proofs() {
        let draft = TestDir::new();
        fs::create_dir_all(draft.path().join("proof")).unwrap();
        fs::write(draft.path().join(HASHES_PATH), b"stale hashes").unwrap();
        fs::write(draft.path().join(SIGNATURE_PATH), b"stale signature").unwrap();

        create_draft_directory(CreateDraftRequest::new(draft.path(), "Bruno")).unwrap();

        assert!(draft.path().join(MANIFEST_PATH).exists());
        assert!(!draft.path().join(HASHES_PATH).exists());
        assert!(!draft.path().join(SIGNATURE_PATH).exists());
    }

    #[test]
    fn create_keeps_existing_message_file() {
        let draft = TestDir::new();
        fs::create_dir_all(draft.path().join("text")).unwrap();
        fs::write(draft.path().join(MESSAGE_PATH), "Already here.\n").unwrap();

        let request = CreateDraftRequest::new(draft.path(), "Bruno");
        create_draft_directory(request).unwrap();

        let message = fs::read_to_string(draft.path().join(MESSAGE_PATH)).unwrap();
        assert_eq!(message, "Already here.\n");
        assert!(draft.path().join(MANIFEST_PATH).exists());
    }

    #[test]
    fn create_refuses_existing_manifest_without_force() {
        let draft = TestDir::new();
        fs::write(draft.path().join(MANIFEST_PATH), "{}").unwrap();

        let error =
            create_draft_directory(CreateDraftRequest::new(draft.path(), "Bruno")).unwrap_err();

        assert!(matches!(
            error,
            PackError::Io(error) if error.kind() == io::ErrorKind::AlreadyExists
        ));
    }

    #[test]
    fn force_create_replaces_manifest_but_keeps_message_file() {
        let draft = TestDir::new();
        fs::create_dir_all(draft.path().join("text")).unwrap();
        fs::write(draft.path().join(MANIFEST_PATH), "{}").unwrap();
        fs::write(draft.path().join(MESSAGE_PATH), "Message conserve.\n").unwrap();

        let request = CreateDraftRequest::new(draft.path(), "GeeBe").with_force(true);
        create_draft_directory(request).unwrap();

        let manifest_bytes = fs::read(draft.path().join(MANIFEST_PATH)).unwrap();
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes).unwrap();
        assert!(epc_core::is_valid_card_id(&manifest.id));
        assert_eq!(manifest.author.display_name, "GeeBe");
        assert_eq!(manifest.sealed_at, "");
        assert_eq!(
            fs::read_to_string(draft.path().join(MESSAGE_PATH)).unwrap(),
            "Message conserve.\n"
        );
    }

    #[test]
    fn packs_to_adr_003_filename_in_output_directory() {
        let source = TestDir::new();
        let output_dir = TestDir::new();
        write_minimal_source(source.path(), "escale:00000000000000009X8Q2E5M0A");

        let output = pack_core_format_to_directory(source.path(), output_dir.path()).unwrap();

        let file_name = output.file_name().and_then(|name| name.to_str()).unwrap();
        assert!(file_name.ends_with("-9X8Q2E5M0A.epc"));
        assert_eq!(file_name.len(), 21);
        assert!(validate_epc_file(&output).is_valid());
    }

    #[test]
    fn pack_seals_draft_manifest_once_and_reuses_filename() {
        let source = TestDir::new();
        let first_output_dir = TestDir::new();
        let second_output_dir = TestDir::new();
        write_draft_source(source.path(), "escale:00000000000000009X8Q2E5M0A");

        let first_output =
            pack_core_format_to_directory(source.path(), first_output_dir.path()).unwrap();
        let manifest_after_first = read_manifest(source.path());
        assert!(!manifest_after_first.sealed_at.is_empty());

        let second_output =
            pack_core_format_to_directory(source.path(), second_output_dir.path()).unwrap();
        let manifest_after_second = read_manifest(source.path());

        assert_eq!(
            first_output.file_name().and_then(|name| name.to_str()),
            second_output.file_name().and_then(|name| name.to_str())
        );
        assert_eq!(
            manifest_after_first.sealed_at,
            manifest_after_second.sealed_at
        );
        assert!(validate_epc_file(&first_output).is_valid());
        assert!(validate_epc_file(&second_output).is_valid());
    }

    #[test]
    fn pack_completes_missing_generated_manifest_fields() {
        let source = TestDir::new();
        let output_dir = TestDir::new();
        fs::create_dir_all(source.path().join("media")).unwrap();
        fs::create_dir_all(source.path().join("text")).unwrap();
        fs::write(
            source.path().join(MANIFEST_PATH),
            serde_json::to_string_pretty(&serde_json::json!({
                "author": {
                    "display_name": "Bruno"
                }
            }))
            .unwrap(),
        )
        .unwrap();
        write_sample_jxl_files(source.path()).unwrap();
        fs::write(source.path().join(MESSAGE_PATH), "Hello **Escale**.\n").unwrap();

        let output = pack_core_format_to_directory(source.path(), output_dir.path()).unwrap();
        let manifest_after_pack = read_manifest(source.path());

        assert_eq!(manifest_after_pack.epc_version, epc_core::EPC_VERSION_1_0);
        assert_eq!(manifest_after_pack.profile, epc_core::CORE_PROFILE);
        assert_eq!(
            manifest_after_pack.object_type,
            epc_core::EPC_OBJECT_TYPE_POSTCARD
        );
        assert!(is_valid_card_id(&manifest_after_pack.id));
        assert!(!manifest_after_pack.created_at.is_empty());
        assert!(!manifest_after_pack.created_local_time.time_zone.is_empty());
        assert!(!manifest_after_pack.sealed_at.is_empty());
        assert_eq!(
            manifest_after_pack.content,
            core_content_manifest(COVER_PATH)
        );
        assert!(validate_epc_file(&output).is_valid());
    }

    #[test]
    fn pack_refuses_to_overwrite_existing_archive() {
        let source = TestDir::new();
        let output_dir = TestDir::new();
        write_draft_source(source.path(), "escale:00000000000000009X8Q2E5M0A");

        let first_output = pack_core_format_to_directory(source.path(), output_dir.path()).unwrap();
        let error = pack_core_format_to_directory(source.path(), output_dir.path()).unwrap_err();

        assert!(first_output.exists());
        assert!(matches!(
            error,
            PackError::Io(error) if error.kind() == io::ErrorKind::AlreadyExists
        ));
    }

    #[test]
    fn signs_directory_and_packs_signature_proof() {
        let source = TestDir::new();
        let output_dir = TestDir::new();
        write_draft_source(source.path(), "escale:00000000000000009X8Q2E5M0A");
        let seed = URL_SAFE_NO_PAD.encode([7_u8; 32]);

        let signature_file =
            sign_core_format_directory(SignRequest::new(source.path(), seed, "Bruno")).unwrap();
        let signature_bytes = fs::read(&signature_file).unwrap();
        let signature: SignatureProof = serde_json::from_slice(&signature_bytes).unwrap();
        assert_eq!(signature.signature_version, "1");
        assert_eq!(
            signature.payload.card_id,
            "escale:00000000000000009X8Q2E5M0A"
        );
        assert_eq!(signature.payload.policy.mode, "all");
        assert!(validate_core_directory(source.path()).is_valid());

        let output = pack_core_format_to_directory(source.path(), output_dir.path()).unwrap();

        assert!(validate_epc_file(&output).is_valid());
    }

    #[test]
    fn sign_refuses_existing_signature_unless_forced() {
        let source = TestDir::new();
        write_draft_source(source.path(), "escale:00000000000000009X8Q2E5M0A");
        let seed = URL_SAFE_NO_PAD.encode([7_u8; 32]);

        let first_signature =
            sign_core_format_directory(SignRequest::new(source.path(), &seed, "Bruno")).unwrap();
        let error = sign_core_format_directory(SignRequest::new(source.path(), &seed, "Bruno"))
            .unwrap_err();

        assert!(matches!(
            error,
            PackError::Io(error) if error.kind() == io::ErrorKind::AlreadyExists
        ));

        fs::write(&first_signature, b"stale signature").unwrap();
        sign_core_format_directory(SignRequest::new(source.path(), seed, "Bruno").with_force(true))
            .unwrap();
        let forced_bytes = fs::read(&first_signature).unwrap();
        assert_ne!(forced_bytes, b"stale signature");
        assert!(validate_core_directory(source.path()).is_valid());
    }

    #[test]
    fn signs_and_packs_with_openssh_ed25519_key() {
        let source = TestDir::new();
        let key_dir = TestDir::new();
        let output_dir = TestDir::new();
        write_draft_source(source.path(), "escale:00000000000000009X8Q2E5M0A");
        let key_file = key_dir.path().join("id_ed25519");
        fs::write(&key_file, openssh_ed25519_private_key([9_u8; 32])).unwrap();

        let output = pack_core_format_to_directory_signed(
            source.path(),
            output_dir.path(),
            &key_file,
            false,
        )
        .unwrap();

        assert!(source.path().join(SIGNATURE_PATH).exists());
        assert!(validate_epc_file(&output).is_valid());
    }

    #[test]
    fn signed_pack_reuses_existing_signature_without_force() {
        let source = TestDir::new();
        let key_dir = TestDir::new();
        let first_output_dir = TestDir::new();
        let second_output_dir = TestDir::new();
        write_draft_source(source.path(), "escale:00000000000000009X8Q2E5M0A");
        let key_file = key_dir.path().join("id_ed25519");
        fs::write(&key_file, openssh_ed25519_private_key([9_u8; 32])).unwrap();

        let first_output = pack_core_format_to_directory_signed(
            source.path(),
            first_output_dir.path(),
            &key_file,
            false,
        )
        .unwrap();
        let signature_file = source.path().join(SIGNATURE_PATH);
        let first_signature = fs::read(&signature_file).unwrap();

        let second_output = pack_core_format_to_directory_signed(
            source.path(),
            second_output_dir.path(),
            &key_file,
            false,
        )
        .unwrap();
        let second_signature = fs::read(&signature_file).unwrap();

        assert_eq!(first_signature, second_signature);
        assert_eq!(
            first_output.file_name().and_then(|name| name.to_str()),
            second_output.file_name().and_then(|name| name.to_str())
        );
        assert!(validate_epc_file(&second_output).is_valid());
    }

    #[test]
    fn generates_adr_003_filename_from_manifest() {
        let manifest = Manifest {
            epc_version: "1.0".to_string(),
            profile: "core-format".to_string(),
            object_type: "postcard".to_string(),
            id: "escale:00000000000000009X8Q2E5M0A".to_string(),
            created_at: "2026-06-17T10:00:00Z".to_string(),
            created_local_time: epc_core::CreatedLocalTime {
                time_zone: "Europe/Paris".to_string(),
                utc_offset: "+02:00".to_string(),
            },
            sealed_at: "2026-06-17T10:05:00Z".to_string(),
            author: epc_core::Author {
                display_name: "Bruno".to_string(),
            },
            content: minimal_content(),
        };

        assert_eq!(sealed_filename(&manifest).unwrap(), "007BDX-9X8Q2E5M0A.epc");
    }

    #[test]
    fn generates_adr_003_draft_filename_from_manifest() {
        let manifest = Manifest {
            epc_version: "1.0".to_string(),
            profile: "core-format".to_string(),
            object_type: "postcard".to_string(),
            id: "escale:00000000000000009X8Q2E5M0A".to_string(),
            created_at: "2026-06-17T10:00:00Z".to_string(),
            created_local_time: epc_core::CreatedLocalTime {
                time_zone: "Europe/Paris".to_string(),
                utc_offset: "+02:00".to_string(),
            },
            sealed_at: String::new(),
            author: epc_core::Author {
                display_name: "Bruno".to_string(),
            },
            content: minimal_content(),
        };

        assert_eq!(draft_filename(&manifest).unwrap(), "escale-9X8Q2E5M0A.epc");
    }

    #[test]
    fn rejects_filename_generation_without_sealed_timestamp() {
        let manifest = Manifest {
            epc_version: "1.0".to_string(),
            profile: "core-format".to_string(),
            object_type: "postcard".to_string(),
            id: "escale:00000000000000009X8Q2E5M0A".to_string(),
            created_at: "2026-06-17T10:00:00Z".to_string(),
            created_local_time: epc_core::CreatedLocalTime {
                time_zone: "Europe/Paris".to_string(),
                utc_offset: "+02:00".to_string(),
            },
            sealed_at: String::new(),
            author: epc_core::Author {
                display_name: "Bruno".to_string(),
            },
            content: minimal_content(),
        };

        assert!(matches!(
            sealed_filename(&manifest),
            Err(PackError::InvalidFilenameMetadata(_))
        ));
    }

    #[test]
    fn formats_utc_timestamp() {
        assert_eq!(format_utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc_timestamp(1_787_137_500), "2026-08-19T11:05:00Z");
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
        write_source_with_sealed_at(root, card_id, "2026-06-17T10:05:00Z");
    }

    fn write_draft_source(root: &Path, card_id: &str) {
        write_source_with_sealed_at(root, card_id, "");
    }

    fn write_source_with_sealed_at(root: &Path, card_id: &str, sealed_at: &str) {
        fs::create_dir_all(root.join("media")).unwrap();
        fs::create_dir_all(root.join("text")).unwrap();

        let manifest = serde_json::json!({
            "epc_version": "1.0",
            "profile": "core-format",
            "type": "postcard",
            "id": card_id,
            "created_at": "2026-06-17T10:00:00Z",
            "created_local_time": {
                "time_zone": "Europe/Paris",
                "utc_offset": "+02:00"
            },
            "sealed_at": sealed_at,
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
        write_sample_jxl_files(root).unwrap();
        fs::write(root.join(MESSAGE_PATH), "Hello **Escale**.\n").unwrap();
    }

    fn read_manifest(root: &Path) -> Manifest {
        let bytes = fs::read(root.join(MANIFEST_PATH)).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn openssh_ed25519_private_key(seed: [u8; 32]) -> String {
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let public_key = verifying_key.as_bytes();
        let mut private_key = Vec::new();
        private_key.extend_from_slice(&seed);
        private_key.extend_from_slice(public_key);

        let mut public_blob = Vec::new();
        push_openssh_string(&mut public_blob, b"ssh-ed25519");
        push_openssh_string(&mut public_blob, public_key);

        let mut private_blob = Vec::new();
        private_blob.extend_from_slice(&0x1234_5678_u32.to_be_bytes());
        private_blob.extend_from_slice(&0x1234_5678_u32.to_be_bytes());
        push_openssh_string(&mut private_blob, b"ssh-ed25519");
        push_openssh_string(&mut private_blob, public_key);
        push_openssh_string(&mut private_blob, &private_key);
        push_openssh_string(&mut private_blob, b"test-key");
        private_blob.extend_from_slice(&[1, 2, 3, 4]);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"openssh-key-v1\0");
        push_openssh_string(&mut bytes, b"none");
        push_openssh_string(&mut bytes, b"none");
        push_openssh_string(&mut bytes, b"");
        bytes.extend_from_slice(&1_u32.to_be_bytes());
        push_openssh_string(&mut bytes, &public_blob);
        push_openssh_string(&mut bytes, &private_blob);

        format!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
            STANDARD.encode(bytes)
        )
    }

    fn push_openssh_string(output: &mut Vec<u8>, value: &[u8]) {
        output.extend_from_slice(&(value.len() as u32).to_be_bytes());
        output.extend_from_slice(value);
    }

    fn minimal_content() -> epc_core::Content {
        epc_core::Content {
            cover: epc_core::MediaContent {
                path: COVER_PATH.to_string(),
                mime: "image/jxl".to_string(),
                image: None,
            },
            thumbnail: epc_core::MediaContent {
                path: THUMBNAIL_PATH.to_string(),
                mime: "image/jxl".to_string(),
                image: None,
            },
            message: epc_core::MessageContent {
                path: MESSAGE_PATH.to_string(),
                mime: "text/markdown".to_string(),
                markdown_profile: "epc-markdown-core".to_string(),
                markdown_profile_version: "1.0".to_string(),
            },
        }
    }
}
