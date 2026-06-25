//! Shared EPC domain types, constants, and profile rules.
//!
//! `epc-core` intentionally contains no filesystem or ZIP processing. It is the
//! small common vocabulary used by validators, packers, CLIs, and future SDK
//! bindings when they need to agree on EPC 1.0 `core-format` names, paths,
//! resource limits, and JSON structures.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};

/// EPC format version implemented by this SDK.
pub const EPC_VERSION_1_0: &str = "1.0";

/// Minimal EPC profile supported by the first reference SDK implementation.
pub const CORE_PROFILE: &str = "core-format";

/// Logical object type used by EPC 1.0 postcard capsules.
pub const EPC_OBJECT_TYPE_POSTCARD: &str = "postcard";

/// Official MIME type for EPC ZIP-backed capsules.
pub const EPC_MIME_TYPE: &str = "application/vnd.escale.epc+zip";

/// Domain separator prepended before hashing the EPC integrity descriptor.
pub const CORE_DOMAIN_SEPARATOR: &str = "EPC-CORE-V1\n";

/// Domain separator prepended before signing an EPC signature payload.
pub const SIGNATURE_DOMAIN_SEPARATOR: &str = "EPC-SIGNATURE-V1\n";

/// Markdown profile required by `core-format` message files.
pub const MARKDOWN_CORE_PROFILE: &str = "epc-markdown-core";

/// Markdown profile version required by EPC 1.0 `core-format`.
pub const MARKDOWN_CORE_PROFILE_VERSION: &str = "1.0";

/// Required hash algorithm name for EPC 1.0 integrity proofs.
pub const HASH_ALGORITHM_SHA256: &str = "sha-256";

/// Integrity descriptor version used by `proof/hashes.json`.
pub const INTEGRITY_VERSION_1: &str = "1";

/// Required manifest path at the capsule root.
pub const MANIFEST_PATH: &str = "manifest.json";

/// Preferred JPEG XL cover image path.
pub const COVER_PATH: &str = "media/cover.jxl";

/// Supported cover image paths for EPC 1.0 `core-format`.
pub const SUPPORTED_COVER_PATHS: [&str; 5] = [
    "media/cover.jpg",
    "media/cover.jpeg",
    "media/cover.png",
    "media/cover.webp",
    "media/cover.jxl",
];

/// Required JPEG XL thumbnail image path.
pub const THUMBNAIL_PATH: &str = "media/thumbnail.jxl";

/// Required Markdown message path.
pub const MESSAGE_PATH: &str = "text/message.md";

/// Required integrity proof path.
pub const HASHES_PATH: &str = "proof/hashes.json";

/// Optional authenticity proof path.
pub const SIGNATURE_PATH: &str = "proof/signature.json";

/// All regular files required by a `core-format` capsule.
pub const EXPECTED_CORE_FILES: [&str; 5] = [
    MANIFEST_PATH,
    COVER_PATH,
    THUMBNAIL_PATH,
    MESSAGE_PATH,
    HASHES_PATH,
];

/// Immutable core files covered by `proof/hashes.json`.
///
/// `proof/hashes.json` is intentionally excluded to avoid recursive hashing.
pub const EXPECTED_HASHED_CORE_FILES: [&str; 4] =
    [MANIFEST_PATH, COVER_PATH, THUMBNAIL_PATH, MESSAGE_PATH];

/// Optional proof files recognized by the `core-format` profile.
pub const OPTIONAL_PROOF_FILES: [&str; 1] = [SIGNATURE_PATH];

/// Directory entries tolerated by the ZIP profile.
///
/// Directory entries are not part of the immutable core; they are allowed only
/// as container conveniences.
pub const ALLOWED_DIRECTORY_ENTRIES: [&str; 3] = ["media", "text", "proof"];

/// Maximum size of a complete `.epc` archive in bytes.
pub const MAX_ARCHIVE_SIZE: u64 = 32 * 1024 * 1024;

/// Maximum total uncompressed size of all regular files in bytes.
pub const MAX_TOTAL_UNCOMPRESSED_SIZE: u64 = 40 * 1024 * 1024;

/// Maximum number of ZIP entries, including tolerated directory entries.
pub const MAX_ZIP_ENTRIES: usize = 9;

/// Maximum number of regular files allowed by `core-format`.
pub const MAX_REGULAR_FILES: usize = 6;

/// Maximum UTF-8 byte length of a normalized capsule path.
pub const MAX_PATH_BYTES: usize = 128;

/// Maximum path depth for files in the capsule root.
pub const MAX_PATH_DEPTH: usize = 2;

/// Maximum `manifest.json` size in bytes.
pub const MAX_MANIFEST_SIZE: u64 = 64 * 1024;

/// Maximum `proof/hashes.json` size in bytes.
pub const MAX_HASHES_SIZE: u64 = 64 * 1024;

/// Maximum `proof/signature.json` size in bytes.
pub const MAX_SIGNATURE_SIZE: u64 = 64 * 1024;

/// Maximum `text/message.md` size in bytes.
pub const MAX_MESSAGE_SIZE: u64 = 64 * 1024;

/// Maximum cover image file size in bytes.
pub const MAX_COVER_SIZE: u64 = 24 * 1024 * 1024;

/// Maximum `media/thumbnail.jxl` file size in bytes.
pub const MAX_THUMBNAIL_SIZE: u64 = 1024 * 1024;

/// Maximum decoded cover image pixels.
pub const MAX_COVER_PIXELS: u64 = 24_000_000;

/// Maximum decoded cover image width or height in pixels.
pub const MAX_COVER_DIMENSION: u32 = 8192;

/// Maximum decoded thumbnail image pixels.
pub const MAX_THUMBNAIL_PIXELS: u64 = 256 * 256;

/// Maximum decoded thumbnail image width or height in pixels.
pub const MAX_THUMBNAIL_DIMENSION: u32 = 256;

/// Maximum number of Markdown links allowed by `epc-markdown-core`.
pub const MAX_MARKDOWN_LINKS: usize = 32;

/// Maximum UTF-8 byte length of a single Markdown line.
pub const MAX_MARKDOWN_LINE_BYTES: usize = 4096;

/// Minimal EPC `manifest.json` model for `core-format`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// EPC format version, currently `"1.0"`.
    pub epc_version: String,

    /// EPC profile, currently `"core-format"`.
    pub profile: String,

    /// Logical object type, serialized as JSON field `type`.
    #[serde(rename = "type")]
    pub object_type: String,

    /// Canonical card identifier in `escale:<ULID>` form.
    pub id: String,

    /// Draft or object creation timestamp in UTC RFC 3339 form.
    pub created_at: String,

    /// Device-local creation context captured when the draft was created.
    pub created_local_time: CreatedLocalTime,

    /// Capsule sealing timestamp in UTC RFC 3339 form.
    pub sealed_at: String,

    /// Display author metadata.
    pub author: Author,

    /// Required readable capsule content.
    pub content: Content,
}

/// Device-local creation time metadata declared by the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedLocalTime {
    /// Device time zone identifier at creation time, preferably IANA form.
    pub time_zone: String,

    /// Device UTC offset at creation time in `+HH:MM` or `-HH:MM` form.
    pub utc_offset: String,
}

/// Display author metadata declared by the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    /// Human-facing author name.
    pub display_name: String,
}

/// Required content section of a `core-format` manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    /// Main cover image declaration.
    pub cover: MediaContent,

    /// Thumbnail image declaration.
    pub thumbnail: MediaContent,

    /// Markdown message declaration.
    pub message: MessageContent,
}

/// Manifest declaration for a media file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaContent {
    /// Capsule-relative path.
    pub path: String,

    /// Explicit MIME type.
    pub mime: String,

    /// Optional still-image metadata captured from the referenced media file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageMetadata>,
}

/// Still-image metadata captured in the manifest for a media file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageMetadata {
    /// Image width in pixels.
    pub width: u32,

    /// Image height in pixels.
    pub height: u32,

    /// Total pixel count.
    pub pixels: u64,

    /// Human-readable image format name, such as `JPEG XL`, `JPEG`, or `PNG`.
    pub format: String,

    /// Technical image encoding name, such as `jpeg-xl`, `jpeg`, or `png`.
    pub encoding: String,

    /// Bits used for each color sample.
    pub bits_per_sample: u32,

    /// Number of color channels declared by the image.
    pub color_channels: u32,

    /// Number of alpha bits declared by the image.
    pub alpha_bits: u32,

    /// Total declared bits per pixel.
    pub bits_per_pixel: u32,
}

/// Manifest declaration for the Markdown message file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageContent {
    /// Capsule-relative path.
    pub path: String,

    /// Explicit MIME type, expected to be `text/markdown`.
    pub mime: String,

    /// Markdown profile name.
    pub markdown_profile: String,

    /// Markdown profile version.
    pub markdown_profile_version: String,
}

/// EPC integrity proof model stored in `proof/hashes.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hashes {
    /// Integrity descriptor version.
    pub integrity_version: String,

    /// Hash algorithm name.
    pub hash_algorithm: String,

    /// Per-file digest entries for immutable core files.
    pub entries: Vec<HashEntry>,

    /// Digest of the canonical integrity descriptor with the EPC core domain.
    pub core_digest: String,
}

/// Per-file digest entry in `proof/hashes.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HashEntry {
    /// Immutable core file path.
    pub path: String,

    /// Transform applied before hashing.
    pub transform: HashTransform,

    /// Base64URL, unpadded SHA-256 digest.
    pub digest: String,
}

/// Hash input transform for an integrity entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HashTransform {
    /// Canonical JSON Serialization transform for JSON documents.
    Jcs,

    /// Byte-for-byte hashing with no transform.
    Identity,
}

/// EPC authenticity proof model stored in `proof/signature.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureProof {
    /// Signature proof format version.
    pub signature_version: String,

    /// Canonical payload signed by every signature.
    pub payload: SignaturePayload,

    /// Signatures over the canonical payload.
    pub signatures: Vec<SignatureEntry>,
}

/// Payload covered by EPC signatures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignaturePayload {
    /// Signature domain context, expected to be `EPC-SIGNATURE-V1`.
    pub context: String,

    /// Manifest card identifier.
    pub card_id: String,

    /// EPC format version.
    pub epc_version: String,

    /// Bound core digest from `proof/hashes.json`.
    pub core_digest: String,

    /// Bound hash algorithm from `proof/hashes.json`.
    pub hash_algorithm: String,

    /// Signer's asserted signing timestamp.
    pub signed_at: String,

    /// Human-facing signer metadata.
    pub signer: SignatureSigner,

    /// Signature verification policy.
    pub policy: SignaturePolicy,
}

/// Human-facing signer metadata in an EPC signature payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureSigner {
    /// Display name asserted by the signer.
    pub display_name: String,

    /// Signer role, for example `author`.
    pub role: String,
}

/// Signature policy for an EPC authenticity proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignaturePolicy {
    /// Policy mode, expected to be `all` or `any`.
    pub mode: String,

    /// Required signing keys.
    pub required_keys: Vec<SignatureRequiredKey>,
}

/// Required signing key descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureRequiredKey {
    /// Signature algorithm identifier.
    pub algorithm: String,

    /// Base64URL JWK thumbprint key identifier.
    pub key_id: String,
}

/// One signature over an EPC signature payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureEntry {
    /// Signature algorithm identifier.
    pub algorithm: String,

    /// Base64URL JWK thumbprint key identifier.
    pub key_id: String,

    /// Public key used to verify the signature.
    pub public_key: SignaturePublicKey,

    /// Base64URL signature bytes.
    pub value: String,
}

/// JWK-style Ed25519 public key representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignaturePublicKey {
    /// Key type, expected to be `OKP`.
    pub kty: String,

    /// Curve, expected to be `Ed25519`.
    pub crv: String,

    /// Base64URL Ed25519 public key bytes.
    pub x: String,
}

/// Returns `true` when a value is a canonical EPC card id.
///
/// The accepted form is `escale:<ULID>` where the ULID is 26 uppercase
/// Crockford Base32 characters.
pub fn is_valid_card_id(value: &str) -> bool {
    let Some(ulid) = value.strip_prefix("escale:") else {
        return false;
    };

    ulid.len() == 26
        && ulid
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'A'..=b'H' | b'J'..=b'K' | b'M'..=b'N' | b'P'..=b'T' | b'V'..=b'Z'))
}

/// Returns `true` when a capsule-relative path satisfies `core-format` rules.
///
/// The check rejects absolute paths, backslashes, NUL bytes, empty segments,
/// `.` and `..` segments, overly long paths, and paths deeper than two levels.
pub fn is_safe_core_path(path: &str) -> bool {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.as_bytes().contains(&0)
        || path.len() > MAX_PATH_BYTES
    {
        return false;
    }

    let mut depth = 0;
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return false;
        }
        depth += 1;
    }

    depth <= MAX_PATH_DEPTH
}

/// Returns the file-size limit in bytes for a required `core-format` file.
///
/// Unknown paths return `None`.
pub fn expected_file_size_limit(path: &str) -> Option<u64> {
    match path {
        MANIFEST_PATH => Some(MAX_MANIFEST_SIZE),
        THUMBNAIL_PATH => Some(MAX_THUMBNAIL_SIZE),
        MESSAGE_PATH => Some(MAX_MESSAGE_SIZE),
        HASHES_PATH => Some(MAX_HASHES_SIZE),
        SIGNATURE_PATH => Some(MAX_SIGNATURE_SIZE),
        _ if is_supported_cover_path(path) => Some(MAX_COVER_SIZE),
        _ => None,
    }
}

/// Returns `true` when `path` is one of the five required regular files.
pub fn is_expected_core_file(path: &str) -> bool {
    EXPECTED_CORE_FILES.contains(&path) || is_supported_cover_path(path)
}

/// Returns `true` when `path` is a regular file allowed by `core-format`.
pub fn is_allowed_regular_file(path: &str) -> bool {
    is_expected_core_file(path) || OPTIONAL_PROOF_FILES.contains(&path)
}

/// Returns `true` when `path` must be covered by `proof/hashes.json`.
pub fn is_expected_hashed_core_file(path: &str) -> bool {
    EXPECTED_HASHED_CORE_FILES.contains(&path) || is_supported_cover_path(path)
}

/// Returns `true` when `path` is an accepted immutable cover image path.
pub fn is_supported_cover_path(path: &str) -> bool {
    SUPPORTED_COVER_PATHS.contains(&path)
}

/// Returns the MIME type expected for a supported cover image path.
pub fn cover_mime_for_path(path: &str) -> Option<&'static str> {
    match path {
        "media/cover.jpg" | "media/cover.jpeg" => Some("image/jpeg"),
        "media/cover.png" => Some("image/png"),
        "media/cover.webp" => Some("image/webp"),
        "media/cover.jxl" => Some("image/jxl"),
        _ => None,
    }
}
