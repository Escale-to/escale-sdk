//! JPEG XL validation, display, and optional encoding helpers for EPC media.
//!
//! `epc-image` is the SDK layer dedicated to EPC still images. It deliberately
//! exposes a small, portable contract so that native apps, Flutter bindings,
//! web adapters, CLIs, and viewers can share the same image rules:
//!
//! - validate JPEG XL bitstreams used by EPC media resources;
//! - decode `media/cover.jxl` or `media/thumbnail.jxl` into RGBA8 pixels;
//! - resize decoded images for display while preserving aspect ratio;
//! - derive the canonical EPC thumbnail from a cover image;
//! - optionally encode JPEG, PNG, or RGBA8 inputs to JPEG XL through `cjxl`.
//!
//! The public display format is [`RgbaImage`]: interleaved 8-bit RGBA pixels in
//! row-major order, with dimensions already adjusted for JPEG XL orientation.
//! This is intentionally lower-level than platform image objects. Bindings can
//! wrap the returned bytes as a Flutter `ui.Image`, a browser `ImageData`, a
//! native bitmap, or a temporary PNG without changing the core SDK behavior.
//!
//! # EPC paths and limits
//!
//! Image kind selection is explicit through [`EpcImageKind`]. The kind controls
//! both the canonical EPC path and the resource limits imported from
//! `epc-core`:
//!
//! - [`EpcImageKind::Cover`] maps to `media/cover.jxl`;
//! - [`EpcImageKind::Thumbnail`] maps to `media/thumbnail.jxl`.
//!
//! Validation and display decoding both enforce the configured maximum side and
//! maximum decoded-pixel limits for the selected kind before returning image
//! data.
//!
//! # Display examples
//!
//! Validate a standalone cover:
//!
//! ```no_run
//! use epc_image::{validate_jxl_file, EpcImageKind};
//!
//! let info = validate_jxl_file("media/cover.jxl", EpcImageKind::Cover)?;
//! println!("{}x{} pixels={}", info.width, info.height, info.pixels);
//! # Ok::<(), epc_image::JxlValidationError>(())
//! ```
//!
//! Render a cover from an unpacked EPC directory:
//!
//! ```no_run
//! use epc_image::{render_cover_from_directory_rgba8, RenderOptions};
//!
//! let image = render_cover_from_directory_rgba8(
//!     "album.epc.unpacked",
//!     RenderOptions::fit(1024, 1024),
//! )?;
//! assert_eq!(image.pixels.len(), image.expected_len());
//! # Ok::<(), epc_image::DisplayError>(())
//! ```
//!
//! Render a thumbnail directly from a `.epc` archive:
//!
//! ```no_run
//! use epc_image::{render_thumbnail_from_epc_rgba8, RenderOptions};
//!
//! let image = render_thumbnail_from_epc_rgba8("album.epc", RenderOptions::fit(256, 256))?;
//! # let _ = image;
//! # Ok::<(), epc_image::DisplayError>(())
//! ```
//!
//! # JPEG XL encoding
//!
//! Encoding is available only with the `jxl-encode-libjxl` Cargo feature. The
//! feature keeps the Rust core small by delegating actual JPEG XL encoding to
//! the reference `cjxl` executable from libjxl. Applications that need writing
//! support should ensure `cjxl` is installed or pass an explicit executable path
//! in `EncodeOptions`.
//!
//! # Generating docs
//!
//! Public API documentation is generated with:
//!
//! ```text
//! cargo doc -p epc-image --no-deps
//! ```
//!
//! Internal helpers are also documented in this file. Include them in the
//! generated output when auditing the implementation pipeline:
//!
//! ```text
//! cargo doc -p epc-image --no-deps --document-private-items
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::fs::File;
use std::io::{self, BufReader, Cursor, Read};
use std::path::Path;

use epc_core::{
    COVER_PATH, MAX_COVER_DIMENSION, MAX_COVER_PIXELS, MAX_THUMBNAIL_DIMENSION,
    MAX_THUMBNAIL_PIXELS, THUMBNAIL_PATH,
};
use jxl_oxide::JxlImage;
use zip::ZipArchive;

#[cfg(feature = "jxl-encode-libjxl")]
use std::ffi::OsString;
#[cfg(feature = "jxl-encode-libjxl")]
use std::process::Command;
#[cfg(feature = "jxl-encode-libjxl")]
use std::time::{SystemTime, UNIX_EPOCH};

/// Decoded JPEG XL image metadata used by EPC validators.
///
/// The dimensions are read after JPEG XL orientation is applied, which means
/// callers can compare these values directly with the rendered display image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JxlInfo {
    /// Image width after orientation is applied.
    pub width: u32,

    /// Image height after orientation is applied.
    pub height: u32,

    /// Number of decoded pixels.
    pub pixels: u64,
}

/// RGBA8 image returned by EPC display APIs.
///
/// Pixels are always interleaved as `R, G, B, A` bytes in row-major order. A
/// valid image has exactly `width * height * 4` bytes, which can be checked
/// with [`RgbaImage::expected_len`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    /// Image width in pixels.
    pub width: u32,

    /// Image height in pixels.
    pub height: u32,

    /// Interleaved RGBA8 pixels in `R, G, B, A` channel order.
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    /// Returns the expected byte length for this image.
    ///
    /// The value is `width * height * 4`, or `0` if the dimensions overflow
    /// `usize` on the current platform.
    pub fn expected_len(&self) -> usize {
        rgba_len(self.width, self.height).unwrap_or(0)
    }
}

/// Resize policy used by display rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeMode {
    /// Keep decoded source dimensions.
    ///
    /// EPC size limits are still enforced before the image is returned.
    Original,

    /// Fit within optional maximum dimensions while preserving aspect ratio.
    ///
    /// Rendering never upscales. If the decoded image already fits inside the
    /// requested box, the original decoded dimensions are kept.
    Fit,
}

/// Display rendering options.
///
/// The default uses [`ResizeMode::Fit`] without explicit maximum dimensions,
/// which preserves the decoded size while still validating that the image fits
/// EPC limits. Use [`RenderOptions::fit`] for UI preview surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    /// Optional maximum output width used by [`ResizeMode::Fit`].
    pub max_width: Option<u32>,

    /// Optional maximum output height used by [`ResizeMode::Fit`].
    pub max_height: Option<u32>,

    /// Resize mode.
    pub resize: ResizeMode,
}

impl Default for RenderOptions {
    /// Builds display options that preserve decoded dimensions by default.
    fn default() -> Self {
        Self {
            max_width: None,
            max_height: None,
            resize: ResizeMode::Fit,
        }
    }
}

impl RenderOptions {
    /// Creates options that keep the decoded image at its original dimensions.
    ///
    /// This is useful when a caller wants to build its own mipmaps, thumbnails,
    /// or native image objects after decoding.
    pub fn original() -> Self {
        Self {
            resize: ResizeMode::Original,
            ..Self::default()
        }
    }

    /// Creates options that fit the decoded image within the given box.
    ///
    /// Both dimensions must be greater than zero. The aspect ratio is preserved
    /// and the image is not upscaled.
    pub fn fit(max_width: u32, max_height: u32) -> Self {
        Self {
            max_width: Some(max_width),
            max_height: Some(max_height),
            resize: ResizeMode::Fit,
        }
    }
}

/// EPC image class used to select the canonical media path and resource limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpcImageKind {
    /// `media/cover.jxl`.
    Cover,

    /// `media/thumbnail.jxl`.
    Thumbnail,
}

impl EpcImageKind {
    /// Returns the EPC image kind for a canonical capsule path.
    ///
    /// Non-image paths and non-canonical aliases return `None`.
    pub fn from_core_path(path: &str) -> Option<Self> {
        match path {
            COVER_PATH => Some(Self::Cover),
            THUMBNAIL_PATH => Some(Self::Thumbnail),
            _ => None,
        }
    }

    /// Canonical capsule path for this image kind.
    pub fn core_path(self) -> &'static str {
        match self {
            Self::Cover => COVER_PATH,
            Self::Thumbnail => THUMBNAIL_PATH,
        }
    }

    /// Maximum allowed decoded pixels for this image kind.
    pub fn max_pixels(self) -> u64 {
        match self {
            Self::Cover => MAX_COVER_PIXELS,
            Self::Thumbnail => MAX_THUMBNAIL_PIXELS,
        }
    }

    /// Maximum allowed decoded width or height for this image kind.
    pub fn max_dimension(self) -> u32 {
        match self {
            Self::Cover => MAX_COVER_DIMENSION,
            Self::Thumbnail => MAX_THUMBNAIL_DIMENSION,
        }
    }
}

/// Error returned when JPEG XL validation fails.
#[derive(Debug)]
pub enum JxlValidationError {
    /// The file cannot be opened.
    Io(io::Error),

    /// The JPEG XL header cannot be parsed.
    ///
    /// This usually means the file is not JPEG XL, is truncated, or uses a
    /// bitstream feature unsupported by the current decoder.
    InvalidBitstream(String),

    /// The decoded dimensions exceed the per-side image limit.
    DimensionsExceeded {
        /// Decoded image width.
        width: u32,

        /// Decoded image height.
        height: u32,

        /// Maximum allowed width or height.
        max_dimension: u32,
    },

    /// The decoded pixel count exceeds the image limit.
    PixelsExceeded {
        /// Decoded pixel count.
        pixels: u64,

        /// Maximum allowed decoded pixel count.
        max_pixels: u64,
    },

    /// The first frame could not be rendered.
    ///
    /// EPC Phase 1 validation intentionally renders the first frame, rather
    /// than only checking the container signature, so corrupt payloads fail
    /// before an application tries to display them.
    DecodeFailed(String),
}

/// Error returned by EPC display rendering helpers.
#[derive(Debug)]
pub enum DisplayError {
    /// The source cannot be read.
    Io(io::Error),

    /// The EPC archive is not a valid ZIP file.
    InvalidZip(String),

    /// A required image entry is missing from the EPC archive.
    MissingImage {
        /// Missing capsule path.
        path: &'static str,
    },

    /// JPEG XL validation or decoding failed.
    Jxl(JxlValidationError),

    /// The decoded or resized image is too large to allocate safely.
    ImageTooLarge,

    /// The decoder returned an unsupported channel layout.
    UnsupportedChannelCount {
        /// Number of channels returned by the decoder stream.
        channels: u32,
    },

    /// Invalid render options were supplied.
    InvalidOptions(&'static str),
}

/// Options for JPEG XL encoding through the reference `cjxl` tool.
///
/// This type is available only with the `jxl-encode-libjxl` Cargo feature. The
/// default favors lossless output (`distance = 0`) with a moderate encoder
/// effort. Applications can choose either distance-oriented or quality-oriented
/// options depending on the UX they expose.
#[cfg(feature = "jxl-encode-libjxl")]
#[derive(Debug, Clone, PartialEq)]
pub struct EncodeOptions {
    /// Path or executable name for `cjxl`.
    ///
    /// The default is `cjxl`, resolved through the process `PATH`.
    pub cjxl_path: OsString,

    /// Optional JPEG XL distance. `0.0` requests mathematically lossless output
    /// for suitable sources.
    pub distance: Option<f32>,

    /// Optional JPEG XL quality value.
    ///
    /// Use [`EncodeOptions::with_quality`] to set this in preference to
    /// distance for simple UI sliders.
    pub quality: Option<f32>,

    /// Optional encoder effort.
    ///
    /// Higher values generally improve compression at the cost of CPU time.
    pub effort: Option<u8>,
}

#[cfg(feature = "jxl-encode-libjxl")]
impl Default for EncodeOptions {
    /// Builds lossless-oriented `cjxl` encoding options.
    fn default() -> Self {
        Self {
            cjxl_path: OsString::from("cjxl"),
            distance: Some(0.0),
            quality: None,
            effort: Some(7),
        }
    }
}

#[cfg(feature = "jxl-encode-libjxl")]
impl EncodeOptions {
    /// Sets the `cjxl` executable path.
    pub fn with_cjxl_path(mut self, path: impl Into<OsString>) -> Self {
        self.cjxl_path = path.into();
        self
    }

    /// Sets the JPEG XL distance.
    ///
    /// `0.0` requests mathematically lossless output for suitable sources.
    pub fn with_distance(mut self, distance: f32) -> Self {
        self.distance = Some(distance);
        self
    }

    /// Sets the JPEG XL quality value and clears distance.
    pub fn with_quality(mut self, quality: f32) -> Self {
        self.quality = Some(quality);
        self.distance = None;
        self
    }

    /// Sets the encoder effort.
    pub fn with_effort(mut self, effort: u8) -> Self {
        self.effort = Some(effort);
        self
    }
}

/// Error returned by JPEG XL encoding helpers.
#[cfg(feature = "jxl-encode-libjxl")]
#[derive(Debug)]
pub enum EncodeError {
    /// Filesystem or process I/O failed.
    Io(io::Error),

    /// Invalid RGBA image data was supplied.
    InvalidRgba {
        /// Expected RGBA byte length.
        expected: usize,

        /// Actual RGBA byte length.
        actual: usize,
    },

    /// Invalid encoder options were supplied.
    InvalidOptions(&'static str),

    /// PNG staging failed before invoking `cjxl`.
    Png(String),

    /// `cjxl` exited unsuccessfully.
    CjxlFailed {
        /// Process exit status rendered for diagnostics.
        status: String,

        /// Captured stderr text.
        stderr: String,
    },

    /// The configured `cjxl` executable could not be started.
    CjxlUnavailable {
        /// Configured executable path.
        path: OsString,

        /// Underlying I/O error.
        source: io::Error,
    },
}

/// Error returned while deriving and encoding a thumbnail from a cover.
#[cfg(feature = "jxl-encode-libjxl")]
#[derive(Debug)]
pub enum ThumbnailError {
    /// The cover could not be decoded or resized.
    Display(DisplayError),

    /// The thumbnail could not be encoded.
    Encode(EncodeError),

    /// The encoded thumbnail does not satisfy EPC thumbnail limits.
    Validation(JxlValidationError),
}

impl From<JxlValidationError> for DisplayError {
    /// Wraps JPEG XL validation errors in the display error type.
    fn from(error: JxlValidationError) -> Self {
        Self::Jxl(error)
    }
}

#[cfg(feature = "jxl-encode-libjxl")]
impl From<DisplayError> for ThumbnailError {
    /// Wraps display errors in the thumbnail generation error type.
    fn from(error: DisplayError) -> Self {
        Self::Display(error)
    }
}

#[cfg(feature = "jxl-encode-libjxl")]
impl From<EncodeError> for ThumbnailError {
    /// Wraps encoding errors in the thumbnail generation error type.
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

#[cfg(feature = "jxl-encode-libjxl")]
impl From<JxlValidationError> for ThumbnailError {
    /// Wraps validation errors in the thumbnail generation error type.
    fn from(error: JxlValidationError) -> Self {
        Self::Validation(error)
    }
}

/// Validates that a file is a decodable JPEG XL still image within EPC limits.
///
/// Validation parses the JPEG XL header, checks the selected [`EpcImageKind`]
/// limits, and renders the first frame to prove the payload is actually
/// decodable. It does not inspect EPC manifests or proof metadata.
pub fn validate_jxl_file(
    path: impl AsRef<Path>,
    kind: EpcImageKind,
) -> Result<JxlInfo, JxlValidationError> {
    let file = File::open(path).map_err(JxlValidationError::Io)?;
    let reader = BufReader::new(file);
    let image = JxlImage::builder()
        .read(reader)
        .map_err(|error| JxlValidationError::InvalidBitstream(error.to_string()))?;
    validate_jxl_image(image, kind)
}

/// Decodes JPEG XL bytes into RGBA8, optionally resizing for display.
///
/// Use this when the caller already loaded `media/cover.jxl` or
/// `media/thumbnail.jxl` from another storage layer. The selected
/// [`EpcImageKind`] still controls validation limits.
pub fn decode_jxl_rgba8(
    bytes: &[u8],
    kind: EpcImageKind,
    options: RenderOptions,
) -> Result<RgbaImage, DisplayError> {
    let image = JxlImage::builder()
        .read(Cursor::new(bytes))
        .map_err(|error| JxlValidationError::InvalidBitstream(error.to_string()))?;
    decode_jxl_image(image, kind, options)
}

/// Reads and decodes a JPEG XL file into RGBA8, optionally resizing for display.
///
/// This is the standalone-file equivalent of [`decode_jxl_rgba8`].
pub fn decode_jxl_file_rgba8(
    path: impl AsRef<Path>,
    kind: EpcImageKind,
    options: RenderOptions,
) -> Result<RgbaImage, DisplayError> {
    let file = File::open(path).map_err(DisplayError::Io)?;
    let reader = BufReader::new(file);
    let image = JxlImage::builder()
        .read(reader)
        .map_err(|error| JxlValidationError::InvalidBitstream(error.to_string()))?;
    decode_jxl_image(image, kind, options)
}

/// Renders `media/cover.jxl` from an unpacked EPC directory as RGBA8.
pub fn render_cover_from_directory_rgba8(
    root: impl AsRef<Path>,
    options: RenderOptions,
) -> Result<RgbaImage, DisplayError> {
    render_image_from_directory_rgba8(root, EpcImageKind::Cover, options)
}

/// Renders `media/thumbnail.jxl` from an unpacked EPC directory as RGBA8.
pub fn render_thumbnail_from_directory_rgba8(
    root: impl AsRef<Path>,
    options: RenderOptions,
) -> Result<RgbaImage, DisplayError> {
    render_image_from_directory_rgba8(root, EpcImageKind::Thumbnail, options)
}

/// Renders an EPC image from an unpacked directory as RGBA8.
///
/// `root` must be the directory that contains canonical EPC entries such as
/// `media/cover.jxl`. The function reads only the image selected by `kind`.
pub fn render_image_from_directory_rgba8(
    root: impl AsRef<Path>,
    kind: EpcImageKind,
    options: RenderOptions,
) -> Result<RgbaImage, DisplayError> {
    decode_jxl_file_rgba8(root.as_ref().join(kind.core_path()), kind, options)
}

/// Renders `media/cover.jxl` from a `.epc` archive as RGBA8.
pub fn render_cover_from_epc_rgba8(
    epc_file: impl AsRef<Path>,
    options: RenderOptions,
) -> Result<RgbaImage, DisplayError> {
    render_image_from_epc_rgba8(epc_file, EpcImageKind::Cover, options)
}

/// Renders `media/thumbnail.jxl` from a `.epc` archive as RGBA8.
pub fn render_thumbnail_from_epc_rgba8(
    epc_file: impl AsRef<Path>,
    options: RenderOptions,
) -> Result<RgbaImage, DisplayError> {
    render_image_from_epc_rgba8(epc_file, EpcImageKind::Thumbnail, options)
}

/// Renders an EPC image from a `.epc` archive as RGBA8.
///
/// The archive is opened as ZIP and only the selected image entry is extracted.
/// This helper is for display; full EPC validation remains the responsibility
/// of the validation layer.
pub fn render_image_from_epc_rgba8(
    epc_file: impl AsRef<Path>,
    kind: EpcImageKind,
    options: RenderOptions,
) -> Result<RgbaImage, DisplayError> {
    let file = File::open(epc_file).map_err(DisplayError::Io)?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| DisplayError::InvalidZip(error.to_string()))?;
    let mut entry = archive
        .by_name(kind.core_path())
        .map_err(|_| DisplayError::MissingImage {
            path: kind.core_path(),
        })?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).map_err(DisplayError::Io)?;
    decode_jxl_rgba8(&bytes, kind, options)
}

/// Resizes an RGBA8 image to fit within a maximum box.
///
/// The aspect ratio is preserved, the image is not cropped, and images already
/// fitting inside the requested box are returned unchanged.
pub fn resize_rgba8_to_fit(
    image: &RgbaImage,
    max_width: u32,
    max_height: u32,
) -> Result<RgbaImage, DisplayError> {
    validate_rgba_display_image(image)?;
    let (target_width, target_height) = target_dimensions(
        image.width,
        image.height,
        RenderOptions::fit(max_width, max_height),
    )?;
    if target_width == image.width && target_height == image.height {
        Ok(image.clone())
    } else {
        resize_rgba8(image, target_width, target_height)
    }
}

/// Derives the canonical EPC thumbnail pixels from a decoded cover image.
///
/// This applies the EPC thumbnail rule: fit within 1024x1024 pixels, preserve
/// the cover aspect ratio, do not crop, and do not upscale.
pub fn derive_thumbnail_rgba8_from_cover(cover: &RgbaImage) -> Result<RgbaImage, DisplayError> {
    resize_rgba8_to_fit(cover, MAX_THUMBNAIL_DIMENSION, MAX_THUMBNAIL_DIMENSION)
}

/// Decodes cover JPEG XL bytes and derives canonical EPC thumbnail pixels.
///
/// The input is validated with cover limits before it is resized to the
/// thumbnail bounds.
pub fn derive_thumbnail_rgba8_from_cover_jxl(bytes: &[u8]) -> Result<RgbaImage, DisplayError> {
    decode_jxl_rgba8(
        bytes,
        EpcImageKind::Cover,
        RenderOptions::fit(MAX_THUMBNAIL_DIMENSION, MAX_THUMBNAIL_DIMENSION),
    )
}

/// Reads a cover JPEG XL file and derives canonical EPC thumbnail pixels.
///
/// Use [`encode_rgba8_to_jxl_file`] afterwards when the `jxl-encode-libjxl`
/// feature is enabled to write the thumbnail to `media/thumbnail.jxl`.
pub fn derive_thumbnail_rgba8_from_cover_jxl_file(
    cover_file: impl AsRef<Path>,
) -> Result<RgbaImage, DisplayError> {
    decode_jxl_file_rgba8(
        cover_file,
        EpcImageKind::Cover,
        RenderOptions::fit(MAX_THUMBNAIL_DIMENSION, MAX_THUMBNAIL_DIMENSION),
    )
}

/// Encodes canonical EPC thumbnail bytes from cover JPEG XL bytes.
///
/// This is the in-memory equivalent of
/// [`encode_thumbnail_from_cover_jxl_file`].
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_thumbnail_from_cover_jxl_bytes(
    cover_jxl: &[u8],
    options: &EncodeOptions,
) -> Result<Vec<u8>, ThumbnailError> {
    let thumbnail = derive_thumbnail_rgba8_from_cover_jxl(cover_jxl)?;
    let bytes = encode_rgba8_to_jxl_bytes(&thumbnail, options)?;
    decode_jxl_rgba8(&bytes, EpcImageKind::Thumbnail, RenderOptions::original())?;
    Ok(bytes)
}

/// Encodes a canonical EPC thumbnail file from a cover JPEG XL file.
///
/// The cover is decoded with cover limits, resized with the EPC thumbnail rule,
/// encoded as JPEG XL, and validated with thumbnail limits.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_thumbnail_from_cover_jxl_file(
    cover_file: impl AsRef<Path>,
    thumbnail_file: impl AsRef<Path>,
    options: &EncodeOptions,
) -> Result<(), ThumbnailError> {
    let thumbnail = derive_thumbnail_rgba8_from_cover_jxl_file(cover_file)?;
    encode_rgba8_to_jxl_file(&thumbnail, thumbnail_file.as_ref(), options)?;
    validate_jxl_file(thumbnail_file, EpcImageKind::Thumbnail)?;
    Ok(())
}

/// Encodes a JPEG file into JPEG XL using `cjxl`.
///
/// The input is passed directly to `cjxl`; no Rust-side color conversion or
/// resizing is performed.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_jpeg_file_to_jxl_file(
    input_file: impl AsRef<Path>,
    output_file: impl AsRef<Path>,
    options: &EncodeOptions,
) -> Result<(), EncodeError> {
    encode_file_to_jxl_file(input_file, output_file, options)
}

/// Encodes a PNG file into JPEG XL using `cjxl`.
///
/// The input is passed directly to `cjxl`; no Rust-side color conversion or
/// resizing is performed.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_png_file_to_jxl_file(
    input_file: impl AsRef<Path>,
    output_file: impl AsRef<Path>,
    options: &EncodeOptions,
) -> Result<(), EncodeError> {
    encode_file_to_jxl_file(input_file, output_file, options)
}

/// Encodes an input image file supported by `cjxl` into JPEG XL.
///
/// This is the generic file-based encoder entry point. Prefer the typed helper
/// names in CLI and binding APIs when they make user intent clearer.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_file_to_jxl_file(
    input_file: impl AsRef<Path>,
    output_file: impl AsRef<Path>,
    options: &EncodeOptions,
) -> Result<(), EncodeError> {
    validate_encode_options(options)?;
    run_cjxl(input_file.as_ref(), output_file.as_ref(), options)
}

/// Encodes RGBA8 pixels into a JPEG XL file using a temporary PNG and `cjxl`.
///
/// The temporary PNG is an implementation detail used to feed `cjxl` a
/// lossless, broadly supported input format.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_rgba8_to_jxl_file(
    image: &RgbaImage,
    output_file: impl AsRef<Path>,
    options: &EncodeOptions,
) -> Result<(), EncodeError> {
    validate_rgba_image(image)?;
    validate_encode_options(options)?;

    let temp_png = TempFile::new("epc-image-rgba", "png")?;
    write_rgba_png(temp_png.path(), image)?;
    run_cjxl(temp_png.path(), output_file.as_ref(), options)
}

/// Encodes RGBA8 pixels into JPEG XL bytes.
///
/// This is convenient for bindings that want to write the resulting bytes into
/// an EPC package without managing an intermediate output file themselves.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_rgba8_to_jxl_bytes(
    image: &RgbaImage,
    options: &EncodeOptions,
) -> Result<Vec<u8>, EncodeError> {
    let temp_jxl = TempFile::new("epc-image-rgba", "jxl")?;
    encode_rgba8_to_jxl_file(image, temp_jxl.path(), options)?;
    std::fs::read(temp_jxl.path()).map_err(EncodeError::Io)
}

/// Writes RGBA8 pixels to a PNG file.
///
/// This helper is primarily useful for debugging, preview generation, or the
/// RGBA-to-JXL staging path. PNG is not the canonical EPC still-image format.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn write_rgba_png_file(
    image: &RgbaImage,
    output_file: impl AsRef<Path>,
) -> Result<(), EncodeError> {
    validate_rgba_image(image)?;
    write_rgba_png(output_file.as_ref(), image)
}

/// Validates an already parsed JPEG XL image.
///
/// This shared helper is used by file validation. It checks EPC image limits
/// and renders the first frame so that malformed payloads fail during
/// validation rather than later in a viewer.
fn validate_jxl_image(image: JxlImage, kind: EpcImageKind) -> Result<JxlInfo, JxlValidationError> {
    let info = image_info(&image, kind)?;
    image
        .render_frame(0)
        .map_err(|error| JxlValidationError::DecodeFailed(error.to_string()))?;
    Ok(info)
}

/// Decodes an already parsed JPEG XL image into the SDK RGBA8 display format.
///
/// The function centralizes the display pipeline: render first frame, normalize
/// channels to RGBA8, compute target dimensions, and resize only when needed.
fn decode_jxl_image(
    image: JxlImage,
    kind: EpcImageKind,
    options: RenderOptions,
) -> Result<RgbaImage, DisplayError> {
    validate_options(options)?;
    image_info(&image, kind)?;

    let render = image
        .render_frame(0)
        .map_err(|error| JxlValidationError::DecodeFailed(error.to_string()))?;
    let mut stream = render.stream();
    let width = stream.width();
    let height = stream.height();
    let channels = stream.channels();
    let samples_len = samples_len(width, height, channels).ok_or(DisplayError::ImageTooLarge)?;
    let mut samples = vec![0_u8; samples_len];
    let written = stream.write_to_buffer(&mut samples);
    samples.truncate(written);

    let rgba = samples_to_rgba8(width, height, channels, &samples)?;
    let (target_width, target_height) = target_dimensions(width, height, options)?;
    if target_width == width && target_height == height {
        Ok(rgba)
    } else {
        resize_rgba8(&rgba, target_width, target_height)
    }
}

/// Extracts oriented JPEG XL dimensions and applies EPC resource limits.
///
/// This function performs metadata-only checks. Callers that need proof of full
/// decodability must render a frame after calling it.
fn image_info(image: &JxlImage, kind: EpcImageKind) -> Result<JxlInfo, JxlValidationError> {
    let header = image.image_header();
    let width = header.width_with_orientation();
    let height = header.height_with_orientation();
    let pixels = u64::from(width) * u64::from(height);

    if width > kind.max_dimension() || height > kind.max_dimension() {
        return Err(JxlValidationError::DimensionsExceeded {
            width,
            height,
            max_dimension: kind.max_dimension(),
        });
    }

    if pixels > kind.max_pixels() {
        return Err(JxlValidationError::PixelsExceeded {
            pixels,
            max_pixels: kind.max_pixels(),
        });
    }

    Ok(JxlInfo {
        width,
        height,
        pixels,
    })
}

/// Converts decoder samples into interleaved RGBA8 pixels.
///
/// The JPEG XL decoder may return grayscale, grayscale plus alpha, RGB, or
/// RGBA streams. This helper expands all supported layouts into the stable
/// [`RgbaImage`] contract.
fn samples_to_rgba8(
    width: u32,
    height: u32,
    channels: u32,
    samples: &[u8],
) -> Result<RgbaImage, DisplayError> {
    let pixel_count = pixel_count(width, height).ok_or(DisplayError::ImageTooLarge)?;
    let mut pixels =
        Vec::with_capacity(rgba_len(width, height).ok_or(DisplayError::ImageTooLarge)?);

    match channels {
        1 => {
            if samples.len() < pixel_count {
                return Err(DisplayError::ImageTooLarge);
            }
            for &gray in &samples[..pixel_count] {
                pixels.extend_from_slice(&[gray, gray, gray, 255]);
            }
        }
        2 => {
            if samples.len() < pixel_count * 2 {
                return Err(DisplayError::ImageTooLarge);
            }
            for sample in samples[..pixel_count * 2].chunks_exact(2) {
                pixels.extend_from_slice(&[sample[0], sample[0], sample[0], sample[1]]);
            }
        }
        3 => {
            if samples.len() < pixel_count * 3 {
                return Err(DisplayError::ImageTooLarge);
            }
            for sample in samples[..pixel_count * 3].chunks_exact(3) {
                pixels.extend_from_slice(&[sample[0], sample[1], sample[2], 255]);
            }
        }
        4 => {
            if samples.len() < pixel_count * 4 {
                return Err(DisplayError::ImageTooLarge);
            }
            pixels.extend_from_slice(&samples[..pixel_count * 4]);
        }
        _ => return Err(DisplayError::UnsupportedChannelCount { channels }),
    }

    Ok(RgbaImage {
        width,
        height,
        pixels,
    })
}

/// Computes display dimensions for the requested resize policy.
///
/// The calculation preserves aspect ratio, rejects zero-sized targets, and
/// never upscales an image that already fits within the requested box.
fn target_dimensions(
    width: u32,
    height: u32,
    options: RenderOptions,
) -> Result<(u32, u32), DisplayError> {
    if width == 0 || height == 0 {
        return Err(DisplayError::ImageTooLarge);
    }

    if options.resize == ResizeMode::Original {
        return Ok((width, height));
    }

    let max_width = options.max_width.unwrap_or(width);
    let max_height = options.max_height.unwrap_or(height);
    if max_width == 0 || max_height == 0 {
        return Err(DisplayError::InvalidOptions(
            "maximum display dimensions must be greater than zero",
        ));
    }

    if width <= max_width && height <= max_height {
        return Ok((width, height));
    }

    let width_ratio = u64::from(max_width) * 1_000_000 / u64::from(width);
    let height_ratio = u64::from(max_height) * 1_000_000 / u64::from(height);
    let ratio = width_ratio.min(height_ratio).max(1);
    let target_width = ((u64::from(width) * ratio) / 1_000_000).max(1) as u32;
    let target_height = ((u64::from(height) * ratio) / 1_000_000).max(1) as u32;
    Ok((target_width, target_height))
}

/// Resizes an RGBA8 image with bilinear interpolation.
///
/// This lightweight scaler is intended for display previews. It keeps the SDK
/// independent of platform image APIs while producing stable RGBA8 output for
/// bindings.
fn resize_rgba8(
    image: &RgbaImage,
    target_width: u32,
    target_height: u32,
) -> Result<RgbaImage, DisplayError> {
    let target_len = rgba_len(target_width, target_height).ok_or(DisplayError::ImageTooLarge)?;
    let mut output = vec![0_u8; target_len];

    if target_width == 0 || target_height == 0 {
        return Err(DisplayError::ImageTooLarge);
    }

    let src_width = image.width as usize;
    let src_height = image.height as usize;
    let dst_width = target_width as usize;
    let dst_height = target_height as usize;

    for dst_y in 0..dst_height {
        let src_y = if dst_height == 1 {
            0.0
        } else {
            dst_y as f32 * (src_height - 1) as f32 / (dst_height - 1) as f32
        };
        let y0 = src_y.floor() as usize;
        let y1 = (y0 + 1).min(src_height - 1);
        let wy = src_y - y0 as f32;

        for dst_x in 0..dst_width {
            let src_x = if dst_width == 1 {
                0.0
            } else {
                dst_x as f32 * (src_width - 1) as f32 / (dst_width - 1) as f32
            };
            let x0 = src_x.floor() as usize;
            let x1 = (x0 + 1).min(src_width - 1);
            let wx = src_x - x0 as f32;

            let dst_idx = (dst_y * dst_width + dst_x) * 4;
            for channel in 0..4 {
                let p00 = image.pixels[(y0 * src_width + x0) * 4 + channel] as f32;
                let p10 = image.pixels[(y0 * src_width + x1) * 4 + channel] as f32;
                let p01 = image.pixels[(y1 * src_width + x0) * 4 + channel] as f32;
                let p11 = image.pixels[(y1 * src_width + x1) * 4 + channel] as f32;
                let top = p00 * (1.0 - wx) + p10 * wx;
                let bottom = p01 * (1.0 - wx) + p11 * wx;
                output[dst_idx + channel] = (top * (1.0 - wy) + bottom * wy).round() as u8;
            }
        }
    }

    Ok(RgbaImage {
        width: target_width,
        height: target_height,
        pixels: output,
    })
}

/// Validates render options before decoding allocates image buffers.
fn validate_options(options: RenderOptions) -> Result<(), DisplayError> {
    if matches!(options.max_width, Some(0)) || matches!(options.max_height, Some(0)) {
        return Err(DisplayError::InvalidOptions(
            "maximum display dimensions must be greater than zero",
        ));
    }
    Ok(())
}

/// Validates that an [`RgbaImage`] has exactly one RGBA byte tuple per pixel.
fn validate_rgba_display_image(image: &RgbaImage) -> Result<(), DisplayError> {
    let expected = rgba_len(image.width, image.height).ok_or(DisplayError::ImageTooLarge)?;
    if image.pixels.len() != expected {
        return Err(DisplayError::ImageTooLarge);
    }
    Ok(())
}

/// Computes the sample buffer length for a decoded stream.
///
/// Returns `None` if the multiplication does not fit in `usize`.
fn samples_len(width: u32, height: u32, channels: u32) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(usize::try_from(channels).ok()?)
}

/// Computes the pixel count for an image.
///
/// Returns `None` if the multiplication does not fit in `usize`.
fn pixel_count(width: u32, height: u32) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)
}

/// Computes the byte length of an RGBA8 image.
///
/// Returns `None` if the multiplication does not fit in `usize`.
fn rgba_len(width: u32, height: u32) -> Option<usize> {
    pixel_count(width, height)?.checked_mul(4)
}

/// Validates user-controlled `cjxl` encoder options.
#[cfg(feature = "jxl-encode-libjxl")]
fn validate_encode_options(options: &EncodeOptions) -> Result<(), EncodeError> {
    if let Some(distance) = options.distance {
        if !distance.is_finite() || distance < 0.0 {
            return Err(EncodeError::InvalidOptions(
                "distance must be finite and non-negative",
            ));
        }
    }

    if let Some(quality) = options.quality {
        if !quality.is_finite() || !(0.0..=100.0).contains(&quality) {
            return Err(EncodeError::InvalidOptions(
                "quality must be finite and between 0 and 100",
            ));
        }
    }

    if matches!(options.effort, Some(0)) {
        return Err(EncodeError::InvalidOptions(
            "effort must be greater than zero",
        ));
    }

    Ok(())
}

/// Validates that an [`RgbaImage`] has exactly one RGBA byte tuple per pixel.
#[cfg(feature = "jxl-encode-libjxl")]
fn validate_rgba_image(image: &RgbaImage) -> Result<(), EncodeError> {
    let expected = rgba_len(image.width, image.height).ok_or(EncodeError::InvalidOptions(
        "image dimensions are too large",
    ))?;
    if image.pixels.len() != expected {
        return Err(EncodeError::InvalidRgba {
            expected,
            actual: image.pixels.len(),
        });
    }
    Ok(())
}

/// Writes an RGBA8 image to PNG for debugging or `cjxl` staging.
#[cfg(feature = "jxl-encode-libjxl")]
fn write_rgba_png(path: &Path, image: &RgbaImage) -> Result<(), EncodeError> {
    let file = File::create(path).map_err(EncodeError::Io)?;
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| EncodeError::Png(error.to_string()))?;
    writer
        .write_image_data(&image.pixels)
        .map_err(|error| EncodeError::Png(error.to_string()))
}

/// Invokes the configured `cjxl` executable with validated options.
///
/// The function captures stderr on encoder failure so CLI and binding layers
/// can surface useful diagnostics to users.
#[cfg(feature = "jxl-encode-libjxl")]
fn run_cjxl(
    input_file: &Path,
    output_file: &Path,
    options: &EncodeOptions,
) -> Result<(), EncodeError> {
    let mut command = Command::new(&options.cjxl_path);
    command.arg(input_file).arg(output_file);

    let distance;
    if let Some(value) = options.distance {
        distance = value.to_string();
        command.arg("-d").arg(&distance);
    }

    let quality;
    if let Some(value) = options.quality {
        quality = value.to_string();
        command.arg("-q").arg(&quality);
    }

    let effort;
    if let Some(value) = options.effort {
        effort = value.to_string();
        command.arg("-e").arg(&effort);
    }

    let output = command
        .output()
        .map_err(|source| EncodeError::CjxlUnavailable {
            path: options.cjxl_path.clone(),
            source,
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(EncodeError::CjxlFailed {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Temporary file removed when dropped.
///
/// The encoder uses this for PNG staging and byte-returning JXL output without
/// pulling in an additional temporary-file dependency.
#[cfg(feature = "jxl-encode-libjxl")]
struct TempFile {
    /// Filesystem path of the temporary file.
    path: std::path::PathBuf,
}

#[cfg(feature = "jxl-encode-libjxl")]
impl TempFile {
    /// Creates a best-effort unique temporary path.
    ///
    /// The file is not opened immediately; callers create it through the writer
    /// or encoder that needs the path.
    fn new(prefix: &str, extension: &str) -> Result<Self, EncodeError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{now}-{}.{}",
            std::process::id(),
            extension
        ));
        let _ = std::fs::remove_file(&path);
        Ok(Self { path })
    }

    /// Returns the temporary file path.
    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(feature = "jxl-encode-libjxl")]
impl Drop for TempFile {
    /// Removes the temporary file if it still exists.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_jxl_bytes() {
        let path = std::env::temp_dir().join("epc-image-invalid.jxl");
        std::fs::write(&path, b"not jpeg xl").unwrap();

        let error = validate_jxl_file(&path, EpcImageKind::Cover).unwrap_err();
        assert!(matches!(error, JxlValidationError::InvalidBitstream(_)));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn renders_cover_rgba8_with_resize() {
        let image = decode_jxl_file_rgba8(
            format!(
                "{}/../../testcases/image/cover.jxl",
                env!("CARGO_MANIFEST_DIR")
            ),
            EpcImageKind::Cover,
            RenderOptions::fit(320, 320),
        )
        .unwrap();

        assert!(image.width <= 320);
        assert!(image.height <= 320);
        assert_eq!(image.pixels.len(), image.expected_len());
    }

    #[test]
    fn renders_thumbnail_from_directory() {
        let image = render_thumbnail_from_directory_rgba8(
            format!("{}/../../testcases/pc_0", env!("CARGO_MANIFEST_DIR")),
            RenderOptions::fit(128, 128),
        )
        .unwrap();

        assert!(image.width <= 128);
        assert!(image.height <= 128);
        assert_eq!(image.pixels.len(), image.expected_len());
    }

    #[test]
    fn resize_rgba8_to_fit_preserves_aspect_ratio() {
        let image = RgbaImage {
            width: 4,
            height: 2,
            pixels: vec![
                255, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0,
                0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
            ],
        };

        let resized = resize_rgba8_to_fit(&image, 2, 2).unwrap();

        assert_eq!(resized.width, 2);
        assert_eq!(resized.height, 1);
        assert_eq!(resized.pixels.len(), resized.expected_len());
    }

    #[test]
    fn derives_thumbnail_from_cover_file() {
        let image = derive_thumbnail_rgba8_from_cover_jxl_file(format!(
            "{}/../../testcases/image/cover.jxl",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();

        assert!(image.width <= MAX_THUMBNAIL_DIMENSION);
        assert!(image.height <= MAX_THUMBNAIL_DIMENSION);
        assert_eq!(image.pixels.len(), image.expected_len());
    }

    #[test]
    fn renders_cover_from_epc_archive() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let archive_path = std::env::temp_dir().join("epc-image-render-test.epc");
        let _ = std::fs::remove_file(&archive_path);
        let file = File::create(&archive_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        zip.add_directory("media/", options).unwrap();
        zip.start_file(COVER_PATH, options).unwrap();
        zip.write_all(include_bytes!("../../../testcases/image/cover.jxl"))
            .unwrap();
        zip.finish().unwrap();

        let image = render_cover_from_epc_rgba8(&archive_path, RenderOptions::fit(96, 96)).unwrap();

        let _ = std::fs::remove_file(&archive_path);
        assert!(image.width <= 96);
        assert!(image.height <= 96);
        assert_eq!(image.pixels.len(), image.expected_len());
    }

    #[cfg(feature = "jxl-encode-libjxl")]
    #[test]
    fn encodes_rgba8_with_cjxl_when_available() {
        if Command::new("cjxl").arg("--version").output().is_err() {
            return;
        }

        let image = RgbaImage {
            width: 2,
            height: 2,
            pixels: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        };

        let bytes = encode_rgba8_to_jxl_bytes(&image, &EncodeOptions::default()).unwrap();
        let decoded =
            decode_jxl_rgba8(&bytes, EpcImageKind::Thumbnail, RenderOptions::original()).unwrap();

        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.pixels.len(), 16);
    }

    #[cfg(feature = "jxl-encode-libjxl")]
    #[test]
    fn encodes_thumbnail_from_cover_bytes_with_cjxl_when_available() {
        if Command::new("cjxl").arg("--version").output().is_err() {
            return;
        }

        let bytes = encode_thumbnail_from_cover_jxl_bytes(
            include_bytes!("../../../testcases/image/cover.jxl"),
            &EncodeOptions::default(),
        )
        .unwrap();
        let decoded =
            decode_jxl_rgba8(&bytes, EpcImageKind::Thumbnail, RenderOptions::original()).unwrap();

        assert!(decoded.width <= MAX_THUMBNAIL_DIMENSION);
        assert!(decoded.height <= MAX_THUMBNAIL_DIMENSION);
        assert_eq!(decoded.pixels.len(), decoded.expected_len());
    }
}
