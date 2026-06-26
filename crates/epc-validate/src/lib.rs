//! Validation model and reference checks for EPC `core-format`.
//!
//! The current validator targets an unpacked capsule directory. It verifies the
//! EPC 1.0 `core-format` manifest, required files, resource limits, Markdown
//! profile metadata, SHA-256 per-file digests, and `core_digest`.
//!
//! ZIP-container validation is also supported for real `.epc` archives. JPEG XL
//! bitstream and dimension checks are performed through the default `jxl`
//! feature.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use epc_core::{
    cover_mime_for_path, expected_file_size_limit, is_allowed_regular_file,
    is_expected_hashed_core_file, is_safe_core_path, is_supported_cover_path, is_valid_card_id,
    HashEntry, HashTransform, Hashes, Manifest, ManifestStatus, SignatureProof,
    CORE_DOMAIN_SEPARATOR, CORE_PROFILE, COVER_PATH, EPC_OBJECT_TYPE_POSTCARD, EPC_VERSION_1_0,
    HASHES_PATH, HASH_ALGORITHM_SHA256, INTEGRITY_VERSION_1, MANIFEST_PATH, MARKDOWN_CORE_PROFILE,
    MARKDOWN_CORE_PROFILE_VERSION, MAX_ARCHIVE_SIZE, MAX_MARKDOWN_LINE_BYTES, MAX_MARKDOWN_LINKS,
    MAX_REGULAR_FILES, MAX_TOTAL_UNCOMPRESSED_SIZE, MAX_ZIP_ENTRIES, MESSAGE_PATH,
    SIGNATURE_DOMAIN_SEPARATOR, SIGNATURE_PATH, THUMBNAIL_PATH,
};
#[cfg(feature = "jxl")]
use epc_image::{EpcImageKind, JxlValidationError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipArchive};

const MAX_COMPRESSION_RATIO: u64 = 100;

/// Severity of a validation issue.
///
/// `error` and `fatal` issues make a report invalid. `warning` and `info`
/// issues preserve global validity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    /// Informational note with no validity impact.
    Info,

    /// Non-fatal concern; the capsule remains valid.
    Warning,

    /// Verifiable conformance failure; the capsule is invalid.
    Error,

    /// Safety or readability failure that may interrupt validation.
    Fatal,
}

/// One structured EPC validation issue.
///
/// The JSON shape follows ADR-010: stable English code, developer-facing title
/// and detail, plus optional file and JSON Pointer locations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// Issue severity.
    pub severity: IssueSeverity,

    /// Stable EPC issue code, for example `EPC_MANIFEST_INVALID_CARD_ID`.
    pub code: String,

    /// Short developer-facing title.
    pub title: String,

    /// Longer developer-facing detail.
    pub detail: String,

    /// Capsule file associated with the issue, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,

    /// JSON Pointer associated with the issue, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,

    /// Related capsule file, when an issue links two files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_file: Option<String>,
}

impl ValidationIssue {
    /// Creates a validation issue without location metadata.
    pub fn new(
        severity: IssueSeverity,
        code: impl Into<String>,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code: code.into(),
            title: title.into(),
            detail: detail.into(),
            file: None,
            pointer: None,
            related_file: None,
        }
    }

    /// Attaches a capsule file path to this issue.
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Attaches a JSON Pointer to this issue.
    pub fn with_pointer(mut self, pointer: impl Into<String>) -> Self {
        self.pointer = Some(pointer.into());
        self
    }

    /// Attaches a second capsule file path related to this issue.
    pub fn with_related_file(mut self, related_file: impl Into<String>) -> Self {
        self.related_file = Some(related_file.into());
        self
    }
}

/// Count of validation issues by severity.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationSummary {
    /// Number of fatal issues.
    pub fatal: usize,

    /// Number of error issues.
    pub error: usize,

    /// Number of warning issues.
    pub warning: usize,

    /// Number of informational issues.
    pub info: usize,
}

/// Positive proof-check results collected during validation.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofReport {
    /// Integrity proof status.
    pub integrity: IntegrityProofReport,

    /// Authenticity signature proof status.
    pub signature: SignatureProofReport,
}

/// `proof/hashes.json` verification status.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrityProofReport {
    /// Whether `proof/hashes.json` was present.
    pub present: bool,

    /// Whether digest and core-digest checks were attempted.
    pub checked: bool,

    /// Whether the integrity proof is valid.
    pub valid: bool,

    /// Hash algorithm declared by `proof/hashes.json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_algorithm: Option<String>,

    /// Verified core digest when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_digest: Option<String>,
}

/// `proof/signature.json` verification status.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureProofReport {
    /// Whether `proof/signature.json` was present.
    pub present: bool,

    /// Whether signature checks were attempted.
    pub checked: bool,

    /// Whether the signature proof is valid under its policy.
    pub valid: bool,

    /// Whether the signature policy was satisfied.
    pub policy_satisfied: bool,

    /// Policy mode, for example `all` or `any`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_mode: Option<String>,

    /// Signer's asserted display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer_display_name: Option<String>,

    /// Signer's asserted role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer_role: Option<String>,

    /// Signer's asserted signing time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_at: Option<String>,

    /// Signatures verified successfully.
    pub verified_signatures: Vec<VerifiedSignatureReport>,
}

/// One successfully verified signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedSignatureReport {
    /// Signature algorithm identifier.
    pub algorithm: String,

    /// Base64URL JWK thumbprint key identifier.
    pub key_id: String,
}

/// Complete structured EPC validation report.
///
/// Reports are serializable and intended to be stable enough for CLI output,
/// conformance tests, and future SDK bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    /// Global validity according to EPC severity rules.
    pub valid: bool,

    /// EPC profile evaluated or detected.
    pub profile: String,

    /// EPC version evaluated or detected.
    pub epc_version: String,

    /// Issue counts by severity.
    pub summary: ValidationSummary,

    /// Positive proof-check results.
    pub proofs: ProofReport,

    /// Ordered list of validation issues.
    pub issues: Vec<ValidationIssue>,
}

/// Options controlling expensive validation passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationOptions {
    /// Whether JPEG XL content should be decoded and checked against EPC image
    /// limits.
    pub validate_jxl_images: bool,
}

impl Default for ValidationOptions {
    fn default() -> Self {
        Self {
            validate_jxl_images: true,
        }
    }
}

impl ValidationOptions {
    /// Returns options that skip JPEG XL decoding.
    ///
    /// This is intended for trusted internal flows that have just performed a
    /// full image validation and only need to re-check manifest, hash, and
    /// signature consistency.
    pub fn without_jxl_images(mut self) -> Self {
        self.validate_jxl_images = false;
        self
    }
}

impl Default for ValidationReport {
    fn default() -> Self {
        Self::new(CORE_PROFILE, EPC_VERSION_1_0)
    }
}

impl ValidationReport {
    /// Creates an empty valid report for a profile and EPC version.
    pub fn new(profile: impl Into<String>, epc_version: impl Into<String>) -> Self {
        Self {
            valid: true,
            profile: profile.into(),
            epc_version: epc_version.into(),
            summary: ValidationSummary::default(),
            proofs: ProofReport::default(),
            issues: Vec::new(),
        }
    }

    /// Returns the global validity flag.
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    /// Adds an issue and updates summary counters plus global validity.
    pub fn push(&mut self, issue: ValidationIssue) {
        match issue.severity {
            IssueSeverity::Fatal => self.summary.fatal += 1,
            IssueSeverity::Error => self.summary.error += 1,
            IssueSeverity::Warning => self.summary.warning += 1,
            IssueSeverity::Info => self.summary.info += 1,
        }
        self.issues.push(issue);
        self.valid = self.summary.fatal == 0 && self.summary.error == 0;
    }

    /// Adds all issues from another report and updates this report metadata.
    pub fn extend(&mut self, other: ValidationReport) {
        self.profile = other.profile;
        self.epc_version = other.epc_version;
        self.proofs = other.proofs;
        for issue in other.issues {
            self.push(issue);
        }
    }

    /// Returns `true` when at least one fatal issue has been recorded.
    pub fn has_fatal(&self) -> bool {
        self.summary.fatal > 0
    }
}

/// Validator for an unpacked EPC `core-format` capsule directory.
///
/// The directory root is expected to contain `manifest.json`, `media/`,
/// `text/`, and `proof/` exactly as they would appear at the root of the EPC
/// ZIP archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreDirectoryValidator {
    root: PathBuf,
    observed_files: BTreeSet<String>,
}

impl CoreDirectoryValidator {
    /// Creates a validator rooted at an unpacked capsule directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            observed_files: BTreeSet::new(),
        }
    }

    fn with_observed_files(root: impl Into<PathBuf>, observed_files: BTreeSet<String>) -> Self {
        Self {
            root: root.into(),
            observed_files,
        }
    }

    /// Validates the directory and returns a structured EPC report.
    ///
    /// This method does not panic for invalid capsules. Filesystem errors are
    /// represented as fatal issues whenever they prevent safe validation.
    pub fn validate(&self) -> ValidationReport {
        self.validate_with_options(ValidationOptions::default())
    }

    /// Validates the directory with explicit validation options.
    pub fn validate_with_options(&self, options: ValidationOptions) -> ValidationReport {
        #[cfg(not(feature = "jxl"))]
        let _ = options;

        let mut report = ValidationReport::default();
        let mut files = BTreeSet::new();
        let mut total_size = 0_u64;

        collect_regular_files(
            &self.root,
            &self.root,
            &mut files,
            &mut total_size,
            &mut report,
        );

        if files.len() > MAX_REGULAR_FILES {
            report.push(ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_RESOURCE_TOO_MANY_FILES",
                "Too many files",
                "The core-format profile allows five required files plus optional proof files.",
            ));
        }

        if total_size > MAX_TOTAL_UNCOMPRESSED_SIZE {
            report.push(ValidationIssue::new(
                IssueSeverity::Fatal,
                "EPC_RESOURCE_UNCOMPRESSED_SIZE_EXCEEDED",
                "Uncompressed size exceeded",
                "The total uncompressed size exceeds the core-format limit.",
            ));
        }

        let observed_files = files
            .iter()
            .cloned()
            .chain(self.observed_files.iter().cloned())
            .collect::<BTreeSet<_>>();

        let cover_files = observed_files
            .iter()
            .filter(|path| is_supported_cover_path(path))
            .count();

        for expected in epc_core::EXPECTED_CORE_FILES {
            if expected == COVER_PATH {
                if cover_files == 0 {
                    report.push(
                        ValidationIssue::new(
                            IssueSeverity::Error,
                            "EPC_CONTENT_MISSING_FILE",
                            "Content file missing",
                            "The required cover image is missing.",
                        )
                        .with_file(COVER_PATH),
                    );
                } else if cover_files > 1 {
                    report.push(ValidationIssue::new(
                        IssueSeverity::Error,
                        "EPC_CONTENT_MULTIPLE_COVER_FILES",
                        "Multiple cover files",
                        "A core-format capsule must contain exactly one cover image.",
                    ));
                }
                continue;
            }
            if !observed_files.contains(expected) {
                let (code, title) = match expected {
                    MANIFEST_PATH => ("EPC_MANIFEST_MISSING", "Manifest missing"),
                    HASHES_PATH => ("EPC_INTEGRITY_HASHES_MISSING", "Hashes missing"),
                    _ => ("EPC_CONTENT_MISSING_FILE", "Content file missing"),
                };
                report.push(
                    ValidationIssue::new(
                        IssueSeverity::Error,
                        code,
                        title,
                        format!("The required file {expected} is missing."),
                    )
                    .with_file(expected),
                );
            }
        }

        for file in &files {
            if !is_allowed_regular_file(file) {
                report.push(
                    ValidationIssue::new(
                        IssueSeverity::Error,
                        "EPC_RESOURCE_UNEXPECTED_ENTRY",
                        "Unexpected entry",
                        "The core-format profile does not allow this file.",
                    )
                    .with_file(file),
                );
            }

            if let Some(limit) = expected_file_size_limit(file) {
                match fs::metadata(self.root.join(file)) {
                    Ok(metadata) if metadata.len() > limit => {
                        let code = if file == MESSAGE_PATH {
                            "EPC_RESOURCE_MARKDOWN_TOO_LARGE"
                        } else if file == MANIFEST_PATH || file == HASHES_PATH {
                            "EPC_RESOURCE_JSON_TOO_LARGE"
                        } else {
                            "EPC_RESOURCE_IMAGE_FILE_TOO_LARGE"
                        };
                        report.push(
                            ValidationIssue::new(
                                IssueSeverity::Error,
                                code,
                                "File size limit exceeded",
                                "The file exceeds its core-format size limit.",
                            )
                            .with_file(file),
                        );
                    }
                    Ok(metadata) if metadata.len() == 0 && is_warnable_empty_content_file(file) => {
                        report.push(
                            ValidationIssue::new(
                                IssueSeverity::Warning,
                                "EPC_CONTENT_EMPTY_FILE",
                                "Empty content file",
                                "The file is present but empty.",
                            )
                            .with_file(file),
                        );
                    }
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
        }

        let manifest = read_manifest(&self.root, &mut report);
        if let Some(manifest) = &manifest {
            report.epc_version = manifest.epc_version.clone();
            report.profile = manifest.profile.clone();
            validate_manifest(manifest, &mut report);
            if !observed_files.contains(&manifest.content.cover.path) {
                report.push(
                    ValidationIssue::new(
                        IssueSeverity::Error,
                        "EPC_CONTENT_MISSING_FILE",
                        "Declared cover missing",
                        "The cover file declared by manifest.json is missing.",
                    )
                    .with_file(&manifest.content.cover.path)
                    .with_pointer("#/content/cover/path"),
                );
            }
        }

        validate_markdown(&self.root, &mut report);
        #[cfg(feature = "jxl")]
        if options.validate_jxl_images {
            validate_jxl_images(&self.root, &files, &mut report);
        }

        let hashes = read_hashes(&self.root, &mut report);
        if let Some(hashes) = &hashes {
            report.proofs.integrity.present = true;
            report.proofs.integrity.hash_algorithm = Some(hashes.hash_algorithm.clone());
            report.proofs.integrity.core_digest = Some(hashes.core_digest.clone());
            validate_hashes_schema(hashes, manifest.as_ref(), &mut report);
        }

        if !report.has_fatal() {
            if let (Some(manifest), Some(hashes)) = (manifest.as_ref(), hashes.as_ref()) {
                validate_integrity(&self.root, manifest, hashes, &mut report);
                if let Some(signature) = read_signature(&self.root, &mut report) {
                    report.proofs.signature.present = true;
                    validate_signature(manifest, hashes, &signature, &mut report);
                }
            }
        }

        report
    }
}

/// Validates an unpacked EPC `core-format` capsule directory.
///
/// This is the convenience entry point used by the CLI.
pub fn validate_core_directory(root: impl Into<PathBuf>) -> ValidationReport {
    CoreDirectoryValidator::new(root).validate()
}

/// Validates an unpacked EPC directory with explicit validation options.
pub fn validate_core_directory_with_options(
    root: impl Into<PathBuf>,
    options: ValidationOptions,
) -> ValidationReport {
    CoreDirectoryValidator::new(root).validate_with_options(options)
}

/// Validates a `.epc` ZIP archive as an EPC 1.0 `core-format` capsule.
///
/// The validator reads ZIP metadata first, rejects unsupported paths and
/// compression methods, extracts expected files to a temporary bounded
/// directory, then delegates manifest and integrity checks to
/// [`validate_core_directory`].
pub fn validate_epc_file(path: impl AsRef<Path>) -> ValidationReport {
    let path = path.as_ref();
    let mut report = ValidationReport::default();

    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_ARCHIVE_SIZE => {
            report.push(ValidationIssue::new(
                IssueSeverity::Fatal,
                "EPC_RESOURCE_ARCHIVE_TOO_LARGE",
                "Archive too large",
                "The EPC archive exceeds the core-format archive size limit.",
            ));
            return report;
        }
        Ok(_) => {}
        Err(error) => {
            report.push(ValidationIssue::new(
                IssueSeverity::Fatal,
                "EPC_CONTAINER_INVALID_ZIP",
                "Cannot read archive",
                format!("The EPC archive cannot be read: {error}."),
            ));
            return report;
        }
    }

    if contains_zip64_marker(path) {
        report.push(ValidationIssue::new(
            IssueSeverity::Error,
            "EPC_CONTAINER_ZIP64_UNSUPPORTED",
            "ZIP64 unsupported",
            "The core-format profile does not allow ZIP64 structures.",
        ));
    }

    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            report.push(ValidationIssue::new(
                IssueSeverity::Fatal,
                "EPC_CONTAINER_INVALID_ZIP",
                "Cannot open archive",
                format!("The EPC archive cannot be opened: {error}."),
            ));
            return report;
        }
    };

    let mut archive = match ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(error) => {
            report.push(ValidationIssue::new(
                IssueSeverity::Fatal,
                "EPC_CONTAINER_INVALID_ZIP",
                "Invalid ZIP archive",
                format!("The EPC archive is not a valid ZIP file: {error}."),
            ));
            return report;
        }
    };

    if archive.len() > MAX_ZIP_ENTRIES {
        report.push(ValidationIssue::new(
            IssueSeverity::Error,
            "EPC_RESOURCE_TOO_MANY_ENTRIES",
            "Too many ZIP entries",
            "The core-format profile allows at most nine ZIP entries.",
        ));
    }

    let temp_dir = TempDir::new("epc-validate");
    let mut seen = BTreeSet::new();
    let mut observed_files = BTreeSet::new();
    let mut regular_files = 0_usize;
    let mut total_uncompressed = 0_u64;

    for index in 0..archive.len() {
        let mut file = match archive.by_index(index) {
            Ok(file) => file,
            Err(error) => {
                report.push(ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_CONTAINER_INVALID_ZIP",
                    "Cannot read ZIP entry",
                    format!("A ZIP entry cannot be read: {error}."),
                ));
                continue;
            }
        };

        let raw_name = file.name().to_string();
        let is_dir = file.is_dir();
        let normalized_name = raw_name.trim_end_matches('/').to_string();

        if normalized_name.is_empty() {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_CONTAINER_UNSAFE_PATH",
                    "Unsafe ZIP path",
                    "Empty ZIP entry names are not allowed.",
                )
                .with_file(raw_name),
            );
            continue;
        }

        if !seen.insert(normalized_name.clone()) {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_CONTAINER_DUPLICATE_PATH",
                    "Duplicate ZIP path",
                    "The archive contains a duplicate normalized path.",
                )
                .with_file(&normalized_name),
            );
            continue;
        }

        if !matches!(
            file.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_CONTAINER_UNSUPPORTED_COMPRESSION",
                    "Unsupported compression",
                    "core-format accepts only store and deflate ZIP methods.",
                )
                .with_file(&normalized_name),
            );
        }

        if is_zip_symlink(file.unix_mode()) {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_CONTAINER_UNSAFE_PATH",
                    "Unsafe ZIP path",
                    "Symbolic links are not allowed in EPC archives.",
                )
                .with_file(&normalized_name),
            );
            continue;
        }

        if is_dir {
            if !epc_core::ALLOWED_DIRECTORY_ENTRIES.contains(&normalized_name.as_str()) {
                report.push(
                    ValidationIssue::new(
                        IssueSeverity::Error,
                        "EPC_RESOURCE_UNEXPECTED_ENTRY",
                        "Unexpected ZIP directory",
                        "The core-format profile does not allow this directory.",
                    )
                    .with_file(&normalized_name),
                );
            }
            continue;
        }

        regular_files += 1;
        total_uncompressed = total_uncompressed.saturating_add(file.size());

        if !is_safe_core_path(&normalized_name) {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_CONTAINER_UNSAFE_PATH",
                    "Unsafe ZIP path",
                    "The ZIP entry path is not safe for an EPC core-format capsule.",
                )
                .with_file(&normalized_name),
            );
            continue;
        }

        if !is_allowed_regular_file(&normalized_name) {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_RESOURCE_UNEXPECTED_ENTRY",
                    "Unexpected ZIP file",
                    "The core-format profile does not allow this file.",
                )
                .with_file(&normalized_name),
            );
            continue;
        }

        observed_files.insert(normalized_name.clone());

        if let Some(limit) = expected_file_size_limit(&normalized_name) {
            if file.size() > limit {
                report.push(
                    ValidationIssue::new(
                        IssueSeverity::Error,
                        file_size_issue_code(&normalized_name),
                        "File size limit exceeded",
                        "The ZIP entry exceeds its core-format size limit.",
                    )
                    .with_file(&normalized_name),
                );
                continue;
            }
        }

        if file.compressed_size() > 0
            && file.size() / file.compressed_size() > MAX_COMPRESSION_RATIO
        {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_RESOURCE_COMPRESSION_RATIO_EXCEEDED",
                    "Compression ratio exceeded",
                    "The ZIP entry compression ratio exceeds the core-format limit.",
                )
                .with_file(&normalized_name),
            );
            continue;
        }

        let output_path = temp_dir.path().join(&normalized_name);
        if let Some(parent) = output_path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                report.push(ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_CONTAINER_INVALID_ZIP",
                    "Cannot extract ZIP entry",
                    format!("Cannot create extraction directory: {error}."),
                ));
                continue;
            }
        }

        let mut bytes = Vec::with_capacity(file.size().min(1024 * 1024) as usize);
        if let Err(error) = file.read_to_end(&mut bytes) {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_CONTAINER_INVALID_ZIP",
                    "Cannot extract ZIP entry",
                    format!("Cannot extract ZIP entry bytes: {error}."),
                )
                .with_file(&normalized_name),
            );
            continue;
        }

        if let Err(error) = fs::write(&output_path, bytes) {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_CONTAINER_INVALID_ZIP",
                    "Cannot write extracted ZIP entry",
                    format!("Cannot write extracted ZIP entry: {error}."),
                )
                .with_file(&normalized_name),
            );
        }
    }

    if regular_files > MAX_REGULAR_FILES {
        report.push(ValidationIssue::new(
            IssueSeverity::Error,
            "EPC_RESOURCE_TOO_MANY_FILES",
            "Too many files",
            "The core-format profile allows five required files plus optional proof files.",
        ));
    }

    if total_uncompressed > MAX_TOTAL_UNCOMPRESSED_SIZE {
        report.push(ValidationIssue::new(
            IssueSeverity::Fatal,
            "EPC_RESOURCE_UNCOMPRESSED_SIZE_EXCEEDED",
            "Uncompressed size exceeded",
            "The total uncompressed size exceeds the core-format limit.",
        ));
    }

    let content_report =
        CoreDirectoryValidator::with_observed_files(temp_dir.path(), observed_files).validate();
    report.extend(content_report);
    report
}

fn file_size_issue_code(path: &str) -> &'static str {
    if path == MESSAGE_PATH {
        "EPC_RESOURCE_MARKDOWN_TOO_LARGE"
    } else if path == MANIFEST_PATH || path == HASHES_PATH {
        "EPC_RESOURCE_JSON_TOO_LARGE"
    } else {
        "EPC_RESOURCE_IMAGE_FILE_TOO_LARGE"
    }
}

fn is_warnable_empty_content_file(path: &str) -> bool {
    is_supported_cover_path(path) || matches!(path, THUMBNAIL_PATH | MESSAGE_PATH)
}

fn is_zip_symlink(unix_mode: Option<u32>) -> bool {
    unix_mode
        .map(|mode| mode & 0o170000 == 0o120000)
        .unwrap_or(false)
}

fn contains_zip64_marker(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    bytes
        .windows(4)
        .any(|window| matches!(window, [0x50, 0x4b, 0x06, 0x06] | [0x50, 0x4b, 0x06, 0x07]))
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
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

fn collect_regular_files(
    root: &Path,
    dir: &Path,
    files: &mut BTreeSet<String>,
    total_size: &mut u64,
    report: &mut ValidationReport,
) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            report.push(ValidationIssue::new(
                IssueSeverity::Fatal,
                "EPC_CONTAINER_INVALID_ZIP",
                "Cannot read capsule directory",
                format!("The capsule directory cannot be read: {error}."),
            ));
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let relative = match path.strip_prefix(root) {
            Ok(relative) => relative,
            Err(_) => continue,
        };
        let relative = relative
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");

        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };

        if metadata.file_type().is_symlink() {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_CONTAINER_UNSAFE_PATH",
                    "Unsafe path",
                    "Symbolic links are not allowed in core-format capsules.",
                )
                .with_file(relative),
            );
            continue;
        }

        if metadata.is_dir() {
            if !is_allowed_directory(&relative) {
                report.push(
                    ValidationIssue::new(
                        IssueSeverity::Error,
                        "EPC_RESOURCE_UNEXPECTED_ENTRY",
                        "Unexpected entry",
                        "The core-format profile does not allow this directory.",
                    )
                    .with_file(relative),
                );
            }
            collect_regular_files(root, &path, files, total_size, report);
        } else if metadata.is_file() {
            if !is_safe_core_path(&relative) {
                report.push(
                    ValidationIssue::new(
                        IssueSeverity::Fatal,
                        "EPC_CONTAINER_UNSAFE_PATH",
                        "Unsafe path",
                        "The file path is not safe for an EPC core-format capsule.",
                    )
                    .with_file(&relative),
                );
            }
            *total_size = total_size.saturating_add(metadata.len());
            if !files.insert(relative.clone()) {
                report.push(
                    ValidationIssue::new(
                        IssueSeverity::Fatal,
                        "EPC_CONTAINER_DUPLICATE_PATH",
                        "Duplicate path",
                        "The capsule contains a duplicate normalized path.",
                    )
                    .with_file(relative),
                );
            }
        } else {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_CONTAINER_UNSAFE_PATH",
                    "Unsafe path",
                    "Only regular files and directories are allowed.",
                )
                .with_file(relative),
            );
        }
    }
}

fn is_allowed_directory(path: &str) -> bool {
    epc_core::ALLOWED_DIRECTORY_ENTRIES.contains(&path)
}

fn read_manifest(root: &Path, report: &mut ValidationReport) -> Option<Manifest> {
    let path = root.join(MANIFEST_PATH);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_MANIFEST_INVALID_JSON",
                    "Cannot read manifest",
                    format!("manifest.json cannot be read: {error}."),
                )
                .with_file(MANIFEST_PATH),
            );
            return None;
        }
    };

    let mut value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_MANIFEST_INVALID_JSON",
                    "Invalid manifest JSON",
                    format!("manifest.json is not valid EPC manifest JSON: {error}."),
                )
                .with_file(MANIFEST_PATH),
            );
            return None;
        }
    };
    complete_manifest_status_default(&mut value);

    match serde_json::from_value(value) {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_MANIFEST_INVALID_JSON",
                    "Invalid manifest JSON",
                    format!("manifest.json is not valid EPC manifest JSON: {error}."),
                )
                .with_file(MANIFEST_PATH),
            );
            None
        }
    }
}

fn complete_manifest_status_default(value: &mut Value) {
    let Value::Object(object) = value else {
        return;
    };
    if object.contains_key("status") {
        return;
    }
    let sealed_at = object
        .get("sealed_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    object.insert(
        "status".to_string(),
        Value::String(if sealed_at.is_empty() {
            "draft".to_string()
        } else {
            "sealed".to_string()
        }),
    );
}

fn read_hashes(root: &Path, report: &mut ValidationReport) -> Option<Hashes> {
    let path = root.join(HASHES_PATH);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_INTEGRITY_HASHES_MISSING",
                    "Cannot read hashes",
                    format!("proof/hashes.json cannot be read: {error}."),
                )
                .with_file(HASHES_PATH),
            );
            return None;
        }
    };

    match serde_json::from_slice(&bytes) {
        Ok(hashes) => Some(hashes),
        Err(error) => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_MANIFEST_INVALID_JSON",
                    "Invalid hashes JSON",
                    format!("proof/hashes.json is not valid JSON: {error}."),
                )
                .with_file(HASHES_PATH),
            );
            None
        }
    }
}

fn read_signature(root: &Path, report: &mut ValidationReport) -> Option<SignatureProof> {
    let path = root.join(SIGNATURE_PATH);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_SIGNATURE_POLICY_NOT_SATISFIED",
                    "Cannot read signature proof",
                    format!("proof/signature.json cannot be read: {error}."),
                )
                .with_file(SIGNATURE_PATH),
            );
            return None;
        }
    };

    match serde_json::from_slice(&bytes) {
        Ok(signature) => Some(signature),
        Err(error) => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_SIGNATURE_POLICY_NOT_SATISFIED",
                    "Invalid signature JSON",
                    format!("proof/signature.json is not valid JSON: {error}."),
                )
                .with_file(SIGNATURE_PATH),
            );
            None
        }
    }
}

fn validate_manifest(manifest: &Manifest, report: &mut ValidationReport) {
    expect_eq(
        &manifest.epc_version,
        EPC_VERSION_1_0,
        "EPC_MANIFEST_UNSUPPORTED_VERSION",
        "Unsupported EPC version",
        "Only EPC 1.0 is supported by core-format.",
        MANIFEST_PATH,
        "#/epc_version",
        report,
    );
    expect_eq(
        &manifest.profile,
        CORE_PROFILE,
        "EPC_MANIFEST_UNSUPPORTED_PROFILE",
        "Unsupported EPC profile",
        "Only the core-format profile is supported.",
        MANIFEST_PATH,
        "#/profile",
        report,
    );
    expect_eq(
        &manifest.object_type,
        EPC_OBJECT_TYPE_POSTCARD,
        "EPC_MANIFEST_REQUIRED_FIELD_MISSING",
        "Unsupported object type",
        "EPC 1.0 core-format capsules must declare type postcard.",
        MANIFEST_PATH,
        "#/type",
        report,
    );

    if !is_valid_card_id(&manifest.id) {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_MANIFEST_INVALID_CARD_ID",
                "Invalid card id",
                "The card id must use the canonical escale:<ULID> form.",
            )
            .with_file(MANIFEST_PATH)
            .with_pointer("#/id"),
        );
    }

    validate_manifest_status(manifest, report);
    validate_created_local_time(manifest, report);

    if manifest.author.display_name.trim().is_empty() {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_MANIFEST_REQUIRED_FIELD_MISSING",
                "Author display name missing",
                "author.display_name is required.",
            )
            .with_file(MANIFEST_PATH)
            .with_pointer("#/author/display_name"),
        );
    }

    match cover_mime_for_path(&manifest.content.cover.path) {
        Some(expected_mime) => validate_content_ref(
            &manifest.content.cover.path,
            &manifest.content.cover.mime,
            &manifest.content.cover.path,
            expected_mime,
            "#/content/cover",
            report,
        ),
        None => report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_MANIFEST_UNEXPECTED_CONTENT_PATH",
                "Unexpected content path",
                "content.cover.path must be media/cover.jpg, media/cover.jpeg, media/cover.png, media/cover.webp, or media/cover.jxl.",
            )
            .with_file(MANIFEST_PATH)
            .with_pointer("#/content/cover/path"),
        ),
    }
    validate_content_ref(
        &manifest.content.thumbnail.path,
        &manifest.content.thumbnail.mime,
        THUMBNAIL_PATH,
        "image/jxl",
        "#/content/thumbnail",
        report,
    );
    validate_content_ref(
        &manifest.content.message.path,
        &manifest.content.message.mime,
        MESSAGE_PATH,
        "text/markdown",
        "#/content/message",
        report,
    );
    expect_markdown_eq(
        &manifest.content.message.markdown_profile,
        MARKDOWN_CORE_PROFILE,
        "Unsupported Markdown profile",
        "#/content/message/markdown_profile",
        report,
    );
    expect_markdown_eq(
        &manifest.content.message.markdown_profile_version,
        MARKDOWN_CORE_PROFILE_VERSION,
        "Unsupported Markdown profile version",
        "#/content/message/markdown_profile_version",
        report,
    );
}

fn validate_manifest_status(manifest: &Manifest, report: &mut ValidationReport) {
    match manifest.status {
        ManifestStatus::Draft | ManifestStatus::Issued if !manifest.sealed_at.is_empty() => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_MANIFEST_INVALID_STATUS",
                    "Invalid manifest status",
                    "draft and issued manifests must not set sealed_at.",
                )
                .with_file(MANIFEST_PATH)
                .with_pointer("#/sealed_at"),
            );
        }
        ManifestStatus::Sealed if manifest.sealed_at.is_empty() => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_MANIFEST_INVALID_STATUS",
                    "Invalid manifest status",
                    "sealed manifests must set sealed_at.",
                )
                .with_file(MANIFEST_PATH)
                .with_pointer("#/sealed_at"),
            );
        }
        _ => {}
    }
}

fn validate_created_local_time(manifest: &Manifest, report: &mut ValidationReport) {
    if !is_valid_time_zone(&manifest.created_local_time.time_zone) {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_MANIFEST_INVALID_CREATED_LOCAL_TIME",
                "Invalid creation time zone",
                "created_local_time.time_zone must be a non-empty device time zone identifier.",
            )
            .with_file(MANIFEST_PATH)
            .with_pointer("#/created_local_time/time_zone"),
        );
    }

    if !is_valid_utc_offset(&manifest.created_local_time.utc_offset) {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_MANIFEST_INVALID_CREATED_LOCAL_TIME",
                "Invalid creation UTC offset",
                "created_local_time.utc_offset must use +HH:MM or -HH:MM.",
            )
            .with_file(MANIFEST_PATH)
            .with_pointer("#/created_local_time/utc_offset"),
        );
    }
}

fn is_valid_time_zone(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'\\')
}

fn is_valid_utc_offset(value: &str) -> bool {
    if value.len() != 6 || !matches!(value.as_bytes()[0], b'+' | b'-') || &value[3..4] != ":" {
        return false;
    }
    let Ok(hour) = value[1..3].parse::<u8>() else {
        return false;
    };
    let Ok(minute) = value[4..6].parse::<u8>() else {
        return false;
    };
    hour <= 23 && minute <= 59
}

fn expect_markdown_eq(
    actual: &str,
    expected: &str,
    title: &str,
    pointer: &str,
    report: &mut ValidationReport,
) {
    if actual != expected {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_TEXT_MARKDOWN_PROFILE_UNSUPPORTED",
                title,
                format!(
                    "The core-format profile requires {pointer} to be {expected:?}, but found {actual:?}."
                ),
            )
            .with_file(MANIFEST_PATH)
            .with_pointer(pointer),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn expect_eq(
    actual: &str,
    expected: &str,
    code: &str,
    title: &str,
    detail: &str,
    file: &str,
    pointer: &str,
    report: &mut ValidationReport,
) {
    if actual != expected {
        report.push(
            ValidationIssue::new(IssueSeverity::Error, code, title, detail)
                .with_file(file)
                .with_pointer(pointer),
        );
    }
}

fn validate_content_ref(
    path: &str,
    mime: &str,
    expected_path: &str,
    expected_mime: &str,
    pointer: &str,
    report: &mut ValidationReport,
) {
    if path != expected_path {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_CONTENT_MISSING_FILE",
                "Unexpected content path",
                "The manifest content path does not match core-format.",
            )
            .with_file(MANIFEST_PATH)
            .with_pointer(format!("{pointer}/path"))
            .with_related_file(expected_path),
        );
    }
    if mime != expected_mime {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_CONTENT_MIME_MISMATCH",
                "MIME mismatch",
                "The manifest content MIME type does not match core-format.",
            )
            .with_file(MANIFEST_PATH)
            .with_pointer(format!("{pointer}/mime"))
            .with_related_file(expected_path),
        );
    }
}

fn validate_markdown(root: &Path, report: &mut ValidationReport) {
    let path = root.join(MESSAGE_PATH);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };

    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_TEXT_MARKDOWN_INVALID_UTF8",
                    "Invalid Markdown UTF-8",
                    "text/message.md must be valid UTF-8.",
                )
                .with_file(MESSAGE_PATH),
            );
            return;
        }
    };

    let mut links = 0_usize;
    for line in text.lines() {
        if line.len() > MAX_MARKDOWN_LINE_BYTES {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_RESOURCE_MARKDOWN_LINE_TOO_LONG",
                    "Markdown line too long",
                    "A Markdown line exceeds the core-format limit.",
                )
                .with_file(MESSAGE_PATH),
            );
        }

        if line.trim_start().starts_with('<') {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Warning,
                    "EPC_TEXT_MARKDOWN_UNSUPPORTED_FEATURE",
                    "Unsupported Markdown feature",
                    "Raw HTML is not supported by epc-markdown-core.",
                )
                .with_file(MESSAGE_PATH),
            );
        }

        links += line.matches("](").count();
        for target in markdown_link_targets(line) {
            if !(target.starts_with("https://")
                || target.starts_with("http://")
                || target.starts_with("mailto:"))
            {
                report.push(
                    ValidationIssue::new(
                        IssueSeverity::Warning,
                        "EPC_TEXT_MARKDOWN_UNSAFE_LINK",
                        "Unsafe Markdown link",
                        "Only https, http, and mailto links are allowed.",
                    )
                    .with_file(MESSAGE_PATH),
                );
            }
        }
    }

    if links > MAX_MARKDOWN_LINKS {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_RESOURCE_MARKDOWN_TOO_MANY_LINKS",
                "Too many Markdown links",
                "The message exceeds the core-format Markdown link limit.",
            )
            .with_file(MESSAGE_PATH),
        );
    }
}

#[cfg(feature = "jxl")]
fn validate_jxl_images(root: &Path, files: &BTreeSet<String>, report: &mut ValidationReport) {
    for file in [COVER_PATH, THUMBNAIL_PATH] {
        if !files.contains(file) {
            continue;
        }

        let Some(kind) = EpcImageKind::from_core_path(file) else {
            continue;
        };

        match epc_image::validate_jxl_file(root.join(file), kind) {
            Ok(_) => {}
            Err(error) => push_jxl_issue(file, error, report),
        }
    }
}

#[cfg(feature = "jxl")]
fn push_jxl_issue(file: &str, error: JxlValidationError, report: &mut ValidationReport) {
    match error {
        JxlValidationError::Io(error) => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_IMAGE_JXL_UNREADABLE",
                    "Cannot read JPEG XL image",
                    format!("The JPEG XL image cannot be read: {error}."),
                )
                .with_file(file),
            );
        }
        JxlValidationError::InvalidBitstream(error) => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_IMAGE_JXL_INVALID",
                    "Invalid JPEG XL image",
                    format!("The image is not a valid JPEG XL bitstream: {error}."),
                )
                .with_file(file),
            );
        }
        JxlValidationError::DimensionsExceeded {
            width,
            height,
            max_dimension,
        } => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_RESOURCE_IMAGE_DIMENSIONS_EXCEEDED",
                    "Image dimensions exceeded",
                    format!(
                        "The decoded image is {width}x{height}, exceeding the {max_dimension}px per-side core-format limit."
                    ),
                )
                .with_file(file),
            );
        }
        JxlValidationError::PixelsExceeded { pixels, max_pixels } => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_RESOURCE_IMAGE_PIXELS_EXCEEDED",
                    "Image pixel limit exceeded",
                    format!(
                        "The decoded image has {pixels} pixels, exceeding the {max_pixels} pixel core-format limit."
                    ),
                )
                .with_file(file),
            );
        }
        JxlValidationError::DecodeFailed(error) => {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_IMAGE_JXL_DECODE_FAILED",
                    "JPEG XL decode failed",
                    format!("The JPEG XL image header was parsed but the first frame could not be decoded: {error}."),
                )
                .with_file(file),
            );
        }
    }
}

fn markdown_link_targets(line: &str) -> impl Iterator<Item = &str> {
    line.match_indices("](").filter_map(|(index, _)| {
        let start = index + 2;
        let rest = &line[start..];
        let end = rest.find(')')?;
        Some(&rest[..end])
    })
}

fn validate_hashes_schema(
    hashes: &Hashes,
    manifest: Option<&Manifest>,
    report: &mut ValidationReport,
) {
    if hashes.integrity_version != INTEGRITY_VERSION_1 {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_INTEGRITY_UNSUPPORTED_HASH_ALGORITHM",
                "Unsupported integrity version",
                "Only integrity version 1 is supported.",
            )
            .with_file(HASHES_PATH)
            .with_pointer("#/integrity_version"),
        );
    }
    if hashes.hash_algorithm != HASH_ALGORITHM_SHA256 {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_INTEGRITY_UNSUPPORTED_HASH_ALGORITHM",
                "Unsupported hash algorithm",
                "Only sha-256 is supported.",
            )
            .with_file(HASHES_PATH)
            .with_pointer("#/hash_algorithm"),
        );
    }

    let mut paths = BTreeSet::new();
    for entry in &hashes.entries {
        if !is_expected_hashed_core_file(&entry.path) {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_CONTENT_UNDECLARED_FILE",
                    "Unexpected hashed file",
                    "proof/hashes.json contains an entry outside the immutable core.",
                )
                .with_file(HASHES_PATH)
                .with_related_file(&entry.path),
            );
        }
        if !paths.insert(entry.path.as_str()) {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Fatal,
                    "EPC_CONTAINER_DUPLICATE_PATH",
                    "Duplicate hash entry",
                    "proof/hashes.json contains duplicate paths.",
                )
                .with_file(HASHES_PATH)
                .with_related_file(&entry.path),
            );
        }
        if URL_SAFE_NO_PAD.decode(&entry.digest).is_err() {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_INTEGRITY_DIGEST_MISMATCH",
                    "Invalid digest encoding",
                    "Digests must be Base64URL without padding.",
                )
                .with_file(HASHES_PATH)
                .with_related_file(&entry.path),
            );
        }
    }

    let cover_path = manifest
        .map(|manifest| manifest.content.cover.path.as_str())
        .unwrap_or(COVER_PATH);
    for expected in [MANIFEST_PATH, cover_path, THUMBNAIL_PATH, MESSAGE_PATH] {
        if !paths.contains(expected) {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_CONTENT_MISSING_FILE",
                    "Missing hash entry",
                    "proof/hashes.json must cover every immutable core file.",
                )
                .with_file(HASHES_PATH)
                .with_related_file(expected),
            );
        }
    }

    if URL_SAFE_NO_PAD.decode(&hashes.core_digest).is_err() {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_INTEGRITY_CORE_DIGEST_MISMATCH",
                "Invalid core digest encoding",
                "core_digest must be Base64URL without padding.",
            )
            .with_file(HASHES_PATH)
            .with_pointer("#/core_digest"),
        );
    }
}

fn validate_integrity(
    root: &Path,
    _manifest: &Manifest,
    hashes: &Hashes,
    report: &mut ValidationReport,
) {
    report.proofs.integrity.checked = true;
    let mut valid = true;

    for entry in &hashes.entries {
        let path = root.join(&entry.path);
        let digest = match digest_entry(&path, entry.transform) {
            Ok(digest) => digest,
            Err(_) => continue,
        };
        if digest != entry.digest {
            valid = false;
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_INTEGRITY_DIGEST_MISMATCH",
                    "Digest mismatch",
                    "The file digest does not match proof/hashes.json.",
                )
                .with_file(HASHES_PATH)
                .with_related_file(&entry.path),
            );
        }
    }

    let descriptor = integrity_descriptor_value(hashes);
    let canonical = canonical_json(&descriptor);
    let mut hasher = Sha256::new();
    hasher.update(CORE_DOMAIN_SEPARATOR.as_bytes());
    hasher.update(canonical.as_bytes());
    let digest = URL_SAFE_NO_PAD.encode(hasher.finalize());

    if digest != hashes.core_digest {
        valid = false;
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_INTEGRITY_CORE_DIGEST_MISMATCH",
                "Core digest mismatch",
                "The core digest does not match the integrity descriptor.",
            )
            .with_file(HASHES_PATH)
            .with_pointer("#/core_digest"),
        );
    }

    report.proofs.integrity.valid = valid;
}

fn validate_signature(
    manifest: &Manifest,
    hashes: &Hashes,
    proof: &SignatureProof,
    report: &mut ValidationReport,
) {
    report.proofs.signature.checked = true;
    report.proofs.signature.policy_mode = Some(proof.payload.policy.mode.clone());
    report.proofs.signature.signer_display_name = Some(proof.payload.signer.display_name.clone());
    report.proofs.signature.signer_role = Some(proof.payload.signer.role.clone());
    report.proofs.signature.signed_at = Some(proof.payload.signed_at.clone());
    let mut valid = true;

    if proof.signature_version != "1" {
        valid = false;
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_SIGNATURE_UNSUPPORTED_ALGORITHM",
                "Unsupported signature proof version",
                "Only signature_version 1 is supported.",
            )
            .with_file(SIGNATURE_PATH)
            .with_pointer("#/signature_version"),
        );
    }

    valid &= validate_signature_binding(
        proof.payload.context == "EPC-SIGNATURE-V1",
        "#/payload/context",
        "payload.context must be EPC-SIGNATURE-V1.",
        report,
    );
    valid &= validate_signature_binding(
        proof.payload.card_id == manifest.id,
        "#/payload/card_id",
        "payload.card_id must match manifest.json id.",
        report,
    );
    valid &= validate_signature_binding(
        proof.payload.epc_version == manifest.epc_version,
        "#/payload/epc_version",
        "payload.epc_version must match manifest.json epc_version.",
        report,
    );
    valid &= validate_signature_binding(
        proof.payload.core_digest == hashes.core_digest,
        "#/payload/core_digest",
        "payload.core_digest must match proof/hashes.json core_digest.",
        report,
    );
    valid &= validate_signature_binding(
        proof.payload.hash_algorithm == hashes.hash_algorithm,
        "#/payload/hash_algorithm",
        "payload.hash_algorithm must match proof/hashes.json hash_algorithm.",
        report,
    );

    if proof.payload.policy.mode != "all" && proof.payload.policy.mode != "any" {
        valid = false;
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_SIGNATURE_POLICY_NOT_SATISFIED",
                "Unsupported signature policy",
                "payload.policy.mode must be all or any.",
            )
            .with_file(SIGNATURE_PATH)
            .with_pointer("#/payload/policy/mode"),
        );
    }
    if proof.payload.policy.required_keys.is_empty() {
        valid = false;
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_SIGNATURE_POLICY_NOT_SATISFIED",
                "Empty signature policy",
                "payload.policy.required_keys must contain at least one key.",
            )
            .with_file(SIGNATURE_PATH)
            .with_pointer("#/payload/policy/required_keys"),
        );
    }

    let signature_input = signature_input(proof);
    let mut valid_required = 0_usize;
    for required in &proof.payload.policy.required_keys {
        if required.algorithm != "Ed25519" {
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_SIGNATURE_UNSUPPORTED_ALGORITHM",
                    "Unsupported signature algorithm",
                    "Only Ed25519 signatures are supported.",
                )
                .with_file(SIGNATURE_PATH),
            );
            continue;
        }

        let matching_signature = proof
            .signatures
            .iter()
            .filter(|signature| {
                signature.algorithm == required.algorithm && signature.key_id == required.key_id
            })
            .find(|signature| verify_ed25519_signature(signature, &signature_input));

        if let Some(signature) = matching_signature {
            valid_required += 1;
            report
                .proofs
                .signature
                .verified_signatures
                .push(VerifiedSignatureReport {
                    algorithm: signature.algorithm.clone(),
                    key_id: signature.key_id.clone(),
                });
        } else if proof.payload.policy.mode == "all" {
            valid = false;
            report.push(
                ValidationIssue::new(
                    IssueSeverity::Error,
                    "EPC_SIGNATURE_POLICY_NOT_SATISFIED",
                    "Required signature missing or invalid",
                    "A required signature is missing, invalid, or does not match its key id.",
                )
                .with_file(SIGNATURE_PATH),
            );
        }
    }

    if proof.payload.policy.mode == "any" && valid_required == 0 {
        valid = false;
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_SIGNATURE_POLICY_NOT_SATISFIED",
                "Signature policy not satisfied",
                "At least one required signature must be valid.",
            )
            .with_file(SIGNATURE_PATH),
        );
    }

    report.proofs.signature.policy_satisfied = match proof.payload.policy.mode.as_str() {
        "all" => {
            valid_required == proof.payload.policy.required_keys.len()
                && !proof.payload.policy.required_keys.is_empty()
        }
        "any" => valid_required > 0,
        _ => false,
    };
    report.proofs.signature.valid = valid && report.proofs.signature.policy_satisfied;
}

fn validate_signature_binding(
    ok: bool,
    pointer: &'static str,
    detail: &'static str,
    report: &mut ValidationReport,
) -> bool {
    if !ok {
        report.push(
            ValidationIssue::new(
                IssueSeverity::Error,
                "EPC_ANCHOR_BINDING_MISMATCH",
                "Signature binding mismatch",
                detail,
            )
            .with_file(SIGNATURE_PATH)
            .with_pointer(pointer),
        );
    }
    ok
}

fn signature_input(proof: &SignatureProof) -> Vec<u8> {
    let mut input = Vec::new();
    input.extend_from_slice(SIGNATURE_DOMAIN_SEPARATOR.as_bytes());
    let payload =
        serde_json::to_value(&proof.payload).expect("signature payload serialization cannot fail");
    input.extend_from_slice(canonical_json(&payload).as_bytes());
    input
}

fn verify_ed25519_signature(signature: &epc_core::SignatureEntry, input: &[u8]) -> bool {
    if signature.public_key.kty != "OKP" || signature.public_key.crv != "Ed25519" {
        return false;
    }

    let public_key_bytes = match URL_SAFE_NO_PAD.decode(&signature.public_key.x) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let public_key_bytes: [u8; 32] = match public_key_bytes.try_into() {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    if ed25519_jwk_thumbprint(&signature.public_key.x) != signature.key_id {
        return false;
    }

    let value = match URL_SAFE_NO_PAD.decode(&signature.value) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let signature = match Signature::from_slice(&value) {
        Ok(signature) => signature,
        Err(_) => return false,
    };
    let verifying_key = match VerifyingKey::from_bytes(&public_key_bytes) {
        Ok(verifying_key) => verifying_key,
        Err(_) => return false,
    };
    verifying_key.verify(input, &signature).is_ok()
}

fn digest_entry(path: &Path, transform: HashTransform) -> io::Result<String> {
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

fn ed25519_jwk_thumbprint(public_key_x: &str) -> String {
    let jwk = serde_json::json!({
        "crv": "Ed25519",
        "kty": "OKP",
        "x": public_key_x,
    });
    URL_SAFE_NO_PAD.encode(sha256(canonical_json(&jwk).as_bytes()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn validates_minimal_core_directory() {
        let root = TestDir::new();
        write_minimal_capsule(root.path(), "escale:00000000000000000000000000");

        let report = validate_core_directory(root.path());

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn validates_minimal_core_directory_with_jpeg_cover() {
        let root = TestDir::new();
        write_minimal_capsule_with_cover(
            root.path(),
            "escale:00000000000000000000000000",
            "media/cover.jpg",
            include_bytes!("../../../testcases/images/tour-eiffel.jpg"),
        );

        let report = validate_core_directory(root.path());

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn validates_minimal_core_directory_with_webp_cover() {
        let root = TestDir::new();
        write_minimal_capsule_with_cover(
            root.path(),
            "escale:00000000000000000000000000",
            "media/cover.webp",
            b"RIFF\x10\x00\x00\x00WEBPVP8 ",
        );

        let report = validate_core_directory(root.path());

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn validates_minimal_epc_zip() {
        let root = TestDir::new();
        write_minimal_capsule(root.path(), "escale:00000000000000000000000000");
        let archive = std::env::temp_dir().join("epc-validate-minimal.epc");
        let _ = fs::remove_file(&archive);
        write_zip_capsule(root.path(), &archive);

        let report = validate_epc_file(&archive);

        let _ = fs::remove_file(&archive);
        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn rejects_too_large_zip_file_without_reporting_it_missing() {
        let root = TestDir::new();
        write_minimal_capsule(root.path(), "escale:00000000000000000000000000");
        fs::write(
            root.path().join(COVER_PATH),
            vec![0_u8; (epc_core::MAX_COVER_SIZE + 1) as usize],
        )
        .unwrap();
        let archive = std::env::temp_dir().join("epc-validate-too-large-cover.epc");
        let _ = fs::remove_file(&archive);
        write_zip_capsule(root.path(), &archive);

        let report = validate_epc_file(&archive);

        let _ = fs::remove_file(&archive);
        assert!(!report.is_valid());
        assert!(report.issues.iter().any(|issue| {
            issue.code == "EPC_RESOURCE_IMAGE_FILE_TOO_LARGE"
                && issue.file.as_deref() == Some(COVER_PATH)
        }));
        assert!(!report.issues.iter().any(|issue| {
            issue.code == "EPC_CONTENT_MISSING_FILE" && issue.file.as_deref() == Some(COVER_PATH)
        }));
    }

    #[test]
    fn rejects_bad_card_id() {
        let root = TestDir::new();
        write_minimal_capsule(root.path(), "escale:not-a-ulid");

        let report = validate_core_directory(root.path());

        assert!(!report.is_valid());
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "EPC_MANIFEST_INVALID_CARD_ID"));
    }

    #[test]
    fn detects_digest_mismatch() {
        let root = TestDir::new();
        write_minimal_capsule(root.path(), "escale:00000000000000000000000000");
        fs::write(root.path().join(MESSAGE_PATH), "changed").unwrap();

        let report = validate_core_directory(root.path());

        assert!(!report.is_valid());
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "EPC_INTEGRITY_DIGEST_MISMATCH"));
    }

    #[test]
    fn rejects_invalid_jxl_content() {
        let root = TestDir::new();
        write_minimal_capsule(root.path(), "escale:00000000000000000000000000");
        fs::write(root.path().join(COVER_PATH), b"not jpeg xl").unwrap();

        let report = validate_core_directory(root.path());

        assert!(!report.is_valid());
        assert!(report.issues.iter().any(|issue| {
            issue.code == "EPC_IMAGE_JXL_INVALID" && issue.file.as_deref() == Some(COVER_PATH)
        }));
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let suffix = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("epc-validate-test-{suffix}"));
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

    fn write_minimal_capsule(root: &Path, card_id: &str) {
        write_minimal_capsule_with_cover(
            root,
            card_id,
            COVER_PATH,
            include_bytes!("../../../testcases/images/arc-de-triomphe-paris.jxl"),
        );
    }

    fn write_minimal_capsule_with_cover(
        root: &Path,
        card_id: &str,
        cover_path: &str,
        cover_bytes: &[u8],
    ) {
        fs::create_dir_all(root.join("media")).unwrap();
        fs::create_dir_all(root.join("text")).unwrap();
        fs::create_dir_all(root.join("proof")).unwrap();

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
                    "path": cover_path,
                    "mime": cover_mime_for_path(cover_path).unwrap()
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
        fs::write(root.join(cover_path), cover_bytes).unwrap();
        write_sample_thumbnail_jxl(root);
        fs::write(root.join(MESSAGE_PATH), "Hello **Escale**.\n").unwrap();

        let entries = [MANIFEST_PATH, cover_path, THUMBNAIL_PATH, MESSAGE_PATH]
            .into_iter()
            .map(|path| {
                let transform = if path == MANIFEST_PATH {
                    HashTransform::Jcs
                } else {
                    HashTransform::Identity
                };
                HashEntry {
                    path: path.to_string(),
                    transform,
                    digest: digest_entry(&root.join(path), transform).unwrap(),
                }
            })
            .collect::<Vec<_>>();

        let mut hashes = Hashes {
            integrity_version: INTEGRITY_VERSION_1.to_string(),
            hash_algorithm: HASH_ALGORITHM_SHA256.to_string(),
            entries,
            core_digest: String::new(),
        };
        let descriptor = integrity_descriptor_value(&hashes);
        let mut hasher = Sha256::new();
        hasher.update(CORE_DOMAIN_SEPARATOR.as_bytes());
        hasher.update(canonical_json(&descriptor).as_bytes());
        hashes.core_digest = URL_SAFE_NO_PAD.encode(hasher.finalize());

        fs::write(
            root.join(HASHES_PATH),
            serde_json::to_string_pretty(&hashes).unwrap(),
        )
        .unwrap();
    }

    fn write_sample_thumbnail_jxl(root: &Path) {
        fs::write(
            root.join(THUMBNAIL_PATH),
            include_bytes!("../../../testcases/images/thumbnail-256.jxl"),
        )
        .unwrap();
    }

    fn write_zip_capsule(root: &Path, output_file: &Path) {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let file = File::create(output_file).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let directory_options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o755);
        let file_options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o644);

        for directory in ["media/", "text/", "proof/"] {
            zip.add_directory(directory, directory_options).unwrap();
        }

        let manifest = read_manifest(root, &mut ValidationReport::default()).unwrap();
        for path in [
            MANIFEST_PATH,
            manifest.content.cover.path.as_str(),
            THUMBNAIL_PATH,
            MESSAGE_PATH,
            HASHES_PATH,
        ] {
            zip.start_file(path, file_options).unwrap();
            let bytes = fs::read(root.join(path)).unwrap();
            zip.write_all(&bytes).unwrap();
        }

        zip.finish().unwrap();
    }
}
