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
//! - optionally encode JPEG, PNG, or RGBA8 inputs to JPEG XL through libjxl.
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
//! Encoding is available only with the `jxl-encode-libjxl` Cargo feature. JPEG
//! and PNG inputs are decoded to RGBA8 pixels, then encoded as JPEG XL with the
//! libjxl C encoder. No external `cjxl` process is required.
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

#![warn(missing_docs)]

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use epc_core::{
    ImageMetadata, COVER_PATH, MAX_COVER_DIMENSION, MAX_COVER_PIXELS, MAX_THUMBNAIL_DIMENSION,
    MAX_THUMBNAIL_PIXELS, THUMBNAIL_PATH,
};
use jxl::api::{
    states, JxlColorType, JxlDataFormat, JxlDecoder, JxlDecoderOptions, JxlOutputBuffer,
    JxlPixelFormat, ProcessingResult,
};
use zip::ZipArchive;

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

/// Error returned while reading image metadata.
#[derive(Debug)]
pub enum ImageMetadataError {
    /// The source cannot be read.
    Io(io::Error),

    /// The image format is not supported by EPC media metadata.
    UnsupportedFormat,

    /// The image header is malformed or incomplete.
    InvalidHeader(String),

    /// JPEG XL header parsing failed.
    Jxl(JxlValidationError),
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

/// Options for JPEG XL encoding.
///
/// This type is available only with the `jxl-encode-libjxl` Cargo feature. The
/// default favors visually lossless output (`distance = 1`) for imported JPEG
/// and PNG files. Applications can choose either distance-oriented or
/// quality-oriented options depending on the UX they expose.
#[cfg(feature = "jxl-encode-libjxl")]
#[derive(Debug, Clone, PartialEq)]
pub struct EncodeOptions {
    /// Optional JPEG XL distance. `0.0` requests mathematically lossless pixel
    /// output; non-zero values use lossy VarDCT encoding.
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
    /// Builds visually lossless-oriented JPEG XL encoding options.
    fn default() -> Self {
        Self {
            distance: Some(1.0),
            quality: None,
            effort: Some(7),
        }
    }
}

#[cfg(feature = "jxl-encode-libjxl")]
impl EncodeOptions {
    /// Sets the JPEG XL distance.
    ///
    /// `0.0` requests mathematically lossless pixel output.
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

    /// Builds thumbnail-friendly options from the current encoder settings.
    ///
    /// The default cover preset is lossless-oriented. Thumbnails are derived
    /// from decoded RGBA pixels, so lossless re-encoding can be much larger than
    /// the source cover. When the caller did not choose a quality or non-zero
    /// distance, use a visually high-quality lossy thumbnail preset instead.
    pub fn for_thumbnail(&self) -> Self {
        let mut options = self.clone();
        let uses_defaultish_distance = match options.distance {
            None => true,
            Some(distance) => distance <= 1.0,
        };
        if options.quality.is_none() && uses_defaultish_distance {
            options.quality = Some(80.0);
            options.distance = None;
        }
        options
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

    /// The source image could not be decoded.
    DecodeImage(String),

    /// JPEG XL encoding failed.
    JxlEncode(String),

    /// PNG writing failed.
    Png(String),
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
    let bytes = std::fs::read(path).map_err(JxlValidationError::Io)?;
    decode_jxl_bytes_with_jxl_rs(&bytes, kind)
        .map(|(info, _)| info)
        .map_err(display_error_to_validation_error)
}

/// Reads still-image metadata from a supported EPC media file.
///
/// JPEG XL, JPEG, and PNG are supported because these are the image encodings
/// accepted by the `core-format` manifest for cover or thumbnail resources.
pub fn read_image_metadata_file(
    path: impl AsRef<Path>,
) -> Result<ImageMetadata, ImageMetadataError> {
    let bytes = std::fs::read(path).map_err(ImageMetadataError::Io)?;
    read_image_metadata(&bytes)
}

/// Reads still-image metadata from supported EPC media bytes.
pub fn read_image_metadata(bytes: &[u8]) -> Result<ImageMetadata, ImageMetadataError> {
    if is_jxl_bytes(bytes) {
        read_jxl_metadata(bytes)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        read_png_metadata(bytes)
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        read_jpeg_metadata(bytes)
    } else {
        Err(ImageMetadataError::UnsupportedFormat)
    }
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
    let (info, rgba) = decode_jxl_bytes_with_jxl_rs(bytes, kind)?;
    resize_decoded_rgba8(rgba, info.width, info.height, options)
}

/// Reads and decodes a JPEG XL file into RGBA8, optionally resizing for display.
///
/// This is the standalone-file equivalent of [`decode_jxl_rgba8`].
pub fn decode_jxl_file_rgba8(
    path: impl AsRef<Path>,
    kind: EpcImageKind,
    options: RenderOptions,
) -> Result<RgbaImage, DisplayError> {
    let bytes = std::fs::read(path).map_err(DisplayError::Io)?;
    decode_jxl_rgba8(&bytes, kind, options)
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
/// This applies the EPC thumbnail rule: fit within 256x256 pixels, preserve
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

/// Reads a supported cover source file and derives canonical EPC thumbnail pixels.
///
/// JPEG and PNG inputs are decoded through the `image` crate. JPEG XL inputs are
/// decoded through the EPC JPEG XL reader. The source file itself is not
/// modified or re-encoded by this helper.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn derive_thumbnail_rgba8_from_cover_file(
    cover_file: impl AsRef<Path>,
) -> Result<RgbaImage, ThumbnailError> {
    let cover_file = cover_file.as_ref();
    let cover = if is_jxl_path(cover_file) {
        derive_thumbnail_rgba8_from_cover_jxl_file(cover_file)?
    } else {
        let image = decode_source_image_rgba8(cover_file)?;
        derive_thumbnail_rgba8_from_cover(&image)?
    };
    Ok(cover)
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
    let thumbnail_options = options.for_thumbnail();
    let bytes = encode_thumbnail_rgba8_to_jxl_bytes(&thumbnail, &thumbnail_options)?;
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
    let thumbnail_options = options.for_thumbnail();
    encode_thumbnail_rgba8_to_jxl_file(&thumbnail, thumbnail_file.as_ref(), &thumbnail_options)?;
    validate_jxl_file(thumbnail_file, EpcImageKind::Thumbnail)?;
    Ok(())
}

/// Encodes a canonical EPC thumbnail file from a supported cover source file.
///
/// The cover source may be JPEG, PNG, or JPEG XL. Only the thumbnail output is
/// encoded as JPEG XL; the caller remains responsible for storing the cover
/// source bytes unchanged in the EPC capsule.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_thumbnail_from_cover_file(
    cover_file: impl AsRef<Path>,
    thumbnail_file: impl AsRef<Path>,
    options: &EncodeOptions,
) -> Result<(), ThumbnailError> {
    let thumbnail = derive_thumbnail_rgba8_from_cover_file(cover_file)?;
    let thumbnail_options = options.for_thumbnail();
    encode_thumbnail_rgba8_to_jxl_file(&thumbnail, thumbnail_file.as_ref(), &thumbnail_options)?;
    validate_jxl_file(thumbnail_file, EpcImageKind::Thumbnail)?;
    Ok(())
}

/// Encodes a JPEG file into JPEG XL.
///
/// The input is decoded to RGBA8 pixels before JPEG XL encoding.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_jpeg_file_to_jxl_file(
    input_file: impl AsRef<Path>,
    output_file: impl AsRef<Path>,
    options: &EncodeOptions,
) -> Result<(), EncodeError> {
    encode_file_to_jxl_file(input_file, output_file, options)
}

/// Encodes a PNG file into JPEG XL.
///
/// The input is decoded to RGBA8 pixels before JPEG XL encoding.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_png_file_to_jxl_file(
    input_file: impl AsRef<Path>,
    output_file: impl AsRef<Path>,
    options: &EncodeOptions,
) -> Result<(), EncodeError> {
    encode_file_to_jxl_file(input_file, output_file, options)
}

/// Encodes an input JPEG or PNG image file into JPEG XL.
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
    let image = decode_source_image_rgba8(input_file.as_ref())?;
    let bytes = encode_rgba8_to_jxl_bytes(&image, options)?;
    std::fs::write(output_file, bytes).map_err(EncodeError::Io)
}

/// Encodes an input JPEG or PNG image file into a thumbnail JPEG XL without alpha.
///
/// The source image is decoded as RGBA8, then encoded as RGB JPEG XL. This does
/// not resize the source; callers should still validate the output against
/// [`EpcImageKind::Thumbnail`] when accepting arbitrary inputs.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_file_to_thumbnail_jxl_file(
    input_file: impl AsRef<Path>,
    output_file: impl AsRef<Path>,
    options: &EncodeOptions,
) -> Result<(), EncodeError> {
    validate_encode_options(options)?;
    let image = decode_source_image_rgba8(input_file.as_ref())?;
    encode_thumbnail_rgba8_to_jxl_file(&image, output_file, options)
}

/// Encodes RGBA8 pixels into a JPEG XL file.
#[cfg(feature = "jxl-encode-libjxl")]
pub fn encode_rgba8_to_jxl_file(
    image: &RgbaImage,
    output_file: impl AsRef<Path>,
    options: &EncodeOptions,
) -> Result<(), EncodeError> {
    let bytes = encode_rgba8_to_jxl_bytes(image, options)?;
    std::fs::write(output_file, bytes).map_err(EncodeError::Io)
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
    validate_rgba_image(image)?;
    validate_encode_options(options)?;
    libjxl_encoder::encode_rgba8(image, options)
}

#[cfg(feature = "jxl-encode-libjxl")]
fn encode_thumbnail_rgba8_to_jxl_file(
    image: &RgbaImage,
    output_file: impl AsRef<Path>,
    options: &EncodeOptions,
) -> Result<(), EncodeError> {
    let bytes = encode_thumbnail_rgba8_to_jxl_bytes(image, options)?;
    std::fs::write(output_file, bytes).map_err(EncodeError::Io)
}

#[cfg(feature = "jxl-encode-libjxl")]
fn encode_thumbnail_rgba8_to_jxl_bytes(
    image: &RgbaImage,
    options: &EncodeOptions,
) -> Result<Vec<u8>, EncodeError> {
    validate_rgba_image(image)?;
    validate_encode_options(options)?;
    libjxl_encoder::encode_rgba8_as_rgb(image, options)
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

/// Resizes an already decoded RGBA8 image according to render options.
fn resize_decoded_rgba8(
    rgba: RgbaImage,
    width: u32,
    height: u32,
    options: RenderOptions,
) -> Result<RgbaImage, DisplayError> {
    validate_options(options)?;
    let (target_width, target_height) = target_dimensions(width, height, options)?;
    if target_width == width && target_height == height {
        Ok(rgba)
    } else {
        resize_rgba8(&rgba, target_width, target_height)
    }
}

/// Decodes JPEG XL bytes with `jxl-rs` into RGBA8 pixels.
fn decode_jxl_bytes_with_jxl_rs(
    bytes: &[u8],
    kind: EpcImageKind,
) -> Result<(JxlInfo, RgbaImage), DisplayError> {
    let mut input = bytes;
    let decoder = JxlDecoder::<states::Initialized>::new(JxlDecoderOptions::default());
    let mut decoder = advance_to_image_info(decoder, &mut input)?;
    let basic_info = decoder.basic_info().clone();
    let width = u32::try_from(basic_info.size.0).map_err(|_| DisplayError::ImageTooLarge)?;
    let height = u32::try_from(basic_info.size.1).map_err(|_| DisplayError::ImageTooLarge)?;
    let info = validate_jxl_dimensions(width, height, kind)?;

    decoder.set_pixel_format(JxlPixelFormat {
        color_type: JxlColorType::Rgba,
        color_data_format: Some(JxlDataFormat::U8 { bit_depth: 8 }),
        extra_channel_format: vec![None; basic_info.extra_channels.len()],
    });

    let decoder = advance_to_frame_info(decoder, &mut input)?;
    let pixel_len = rgba_len(width, height).ok_or(DisplayError::ImageTooLarge)?;
    let mut pixels = vec![0_u8; pixel_len];
    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or(DisplayError::ImageTooLarge)?;
    let mut buffers = [JxlOutputBuffer::new(
        &mut pixels,
        height as usize,
        row_bytes,
    )];
    advance_to_frame_done(decoder, &mut input, &mut buffers)?;

    Ok((
        info,
        RgbaImage {
            width,
            height,
            pixels,
        },
    ))
}

fn read_jxl_metadata(bytes: &[u8]) -> Result<ImageMetadata, ImageMetadataError> {
    let mut input = bytes;
    let decoder = JxlDecoder::<states::Initialized>::new(JxlDecoderOptions::default());
    let decoder = advance_to_image_info(decoder, &mut input).map_err(|error| match error {
        DisplayError::Jxl(error) => ImageMetadataError::Jxl(error),
        other => ImageMetadataError::InvalidHeader(format!("{other:?}")),
    })?;
    let basic_info = decoder.basic_info();
    let width = u32::try_from(basic_info.size.0)
        .map_err(|_| ImageMetadataError::InvalidHeader("image width is too large".to_string()))?;
    let height = u32::try_from(basic_info.size.1)
        .map_err(|_| ImageMetadataError::InvalidHeader("image height is too large".to_string()))?;
    let bits_per_sample = basic_info.bit_depth.bits_per_sample();
    let color_channels = 3;
    let alpha_bits = if basic_info.extra_channels.is_empty() {
        0
    } else {
        bits_per_sample
    };
    let bits_per_pixel = bits_per_sample
        .saturating_mul(color_channels)
        .saturating_add(alpha_bits);
    Ok(ImageMetadata {
        width,
        height,
        pixels: u64::from(width) * u64::from(height),
        format: "JPEG XL".to_string(),
        encoding: "jpeg-xl".to_string(),
        bits_per_sample,
        color_channels,
        alpha_bits,
        bits_per_pixel,
    })
}

fn read_png_metadata(bytes: &[u8]) -> Result<ImageMetadata, ImageMetadataError> {
    if bytes.len() < 33 || &bytes[12..16] != b"IHDR" {
        return Err(ImageMetadataError::InvalidHeader(
            "missing PNG IHDR chunk".to_string(),
        ));
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    let bit_depth = u32::from(bytes[24]);
    let color_type = bytes[25];
    let (color_channels, alpha_bits) = match color_type {
        0 => (1, 0),
        2 => (3, 0),
        3 => (1, 0),
        4 => (1, bit_depth),
        6 => (3, bit_depth),
        _ => {
            return Err(ImageMetadataError::InvalidHeader(format!(
                "unsupported PNG color type {color_type}"
            )));
        }
    };
    Ok(ImageMetadata {
        width,
        height,
        pixels: u64::from(width) * u64::from(height),
        format: "PNG".to_string(),
        encoding: "png".to_string(),
        bits_per_sample: bit_depth,
        color_channels,
        alpha_bits,
        bits_per_pixel: bit_depth
            .saturating_mul(color_channels)
            .saturating_add(alpha_bits),
    })
}

fn read_jpeg_metadata(bytes: &[u8]) -> Result<ImageMetadata, ImageMetadataError> {
    let mut offset = 2;
    while offset + 4 <= bytes.len() {
        while offset < bytes.len() && bytes[offset] == 0xff {
            offset += 1;
        }
        if offset >= bytes.len() {
            break;
        }
        let marker = bytes[offset];
        offset += 1;
        if marker == 0xd9 || marker == 0xda {
            break;
        }
        if offset + 2 > bytes.len() {
            return Err(ImageMetadataError::InvalidHeader(
                "truncated JPEG segment length".to_string(),
            ));
        }
        let segment_len = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        if segment_len < 2 || offset + segment_len > bytes.len() {
            return Err(ImageMetadataError::InvalidHeader(
                "invalid JPEG segment length".to_string(),
            ));
        }
        if is_jpeg_sof_marker(marker) {
            if segment_len < 8 {
                return Err(ImageMetadataError::InvalidHeader(
                    "truncated JPEG frame header".to_string(),
                ));
            }
            let precision = u32::from(bytes[offset + 2]);
            let height = u32::from(u16::from_be_bytes([bytes[offset + 3], bytes[offset + 4]]));
            let width = u32::from(u16::from_be_bytes([bytes[offset + 5], bytes[offset + 6]]));
            let color_channels = u32::from(bytes[offset + 7]);
            return Ok(ImageMetadata {
                width,
                height,
                pixels: u64::from(width) * u64::from(height),
                format: "JPEG".to_string(),
                encoding: jpeg_encoding_name(marker).to_string(),
                bits_per_sample: precision,
                color_channels,
                alpha_bits: 0,
                bits_per_pixel: precision.saturating_mul(color_channels),
            });
        }
        offset += segment_len;
    }

    Err(ImageMetadataError::InvalidHeader(
        "missing JPEG start-of-frame segment".to_string(),
    ))
}

fn is_jxl_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0x0a])
        || bytes.starts_with(&[0x00, 0x00, 0x00, 0x0c, b'J', b'X', b'L', b' '])
}

fn is_jpeg_sof_marker(marker: u8) -> bool {
    matches!(
        marker,
        0xc0 | 0xc1 | 0xc2 | 0xc3 | 0xc5 | 0xc6 | 0xc7 | 0xc9 | 0xca | 0xcb | 0xcd | 0xce | 0xcf
    )
}

fn jpeg_encoding_name(marker: u8) -> &'static str {
    match marker {
        0xc0 => "jpeg-baseline",
        0xc2 => "jpeg-progressive",
        _ => "jpeg",
    }
}

fn advance_to_image_info<'a>(
    mut decoder: JxlDecoder<states::Initialized>,
    input: &mut &'a [u8],
) -> Result<JxlDecoder<states::WithImageInfo>, DisplayError> {
    loop {
        match decoder
            .process(input)
            .map_err(|error| JxlValidationError::InvalidBitstream(error.to_string()))?
        {
            ProcessingResult::Complete { result } => return Ok(result),
            ProcessingResult::NeedsMoreInput { fallback, .. } => {
                if input.is_empty() {
                    return Err(DisplayError::Jxl(JxlValidationError::InvalidBitstream(
                        "unexpected end of JPEG XL input".to_string(),
                    )));
                }
                decoder = fallback;
            }
        }
    }
}

fn advance_to_frame_info<'a>(
    mut decoder: JxlDecoder<states::WithImageInfo>,
    input: &mut &'a [u8],
) -> Result<JxlDecoder<states::WithFrameInfo>, DisplayError> {
    loop {
        match decoder
            .process(input)
            .map_err(|error| JxlValidationError::InvalidBitstream(error.to_string()))?
        {
            ProcessingResult::Complete { result } => return Ok(result),
            ProcessingResult::NeedsMoreInput { fallback, .. } => {
                if input.is_empty() {
                    return Err(DisplayError::Jxl(JxlValidationError::InvalidBitstream(
                        "unexpected end of JPEG XL input".to_string(),
                    )));
                }
                decoder = fallback;
            }
        }
    }
}

fn advance_to_frame_done<'a>(
    mut decoder: JxlDecoder<states::WithFrameInfo>,
    input: &mut &'a [u8],
    buffers: &mut [JxlOutputBuffer<'_>],
) -> Result<JxlDecoder<states::WithImageInfo>, DisplayError> {
    loop {
        match decoder
            .process(input, buffers)
            .map_err(|error| JxlValidationError::DecodeFailed(error.to_string()))?
        {
            ProcessingResult::Complete { result } => return Ok(result),
            ProcessingResult::NeedsMoreInput { fallback, .. } => {
                if input.is_empty() {
                    return Err(DisplayError::Jxl(JxlValidationError::DecodeFailed(
                        "unexpected end of JPEG XL frame".to_string(),
                    )));
                }
                decoder = fallback;
            }
        }
    }
}

fn display_error_to_validation_error(error: DisplayError) -> JxlValidationError {
    match error {
        DisplayError::Io(error) => JxlValidationError::Io(error),
        DisplayError::Jxl(error) => error,
        other => JxlValidationError::DecodeFailed(format!("{other:?}")),
    }
}

/// Applies EPC resource limits to decoded JPEG XL dimensions.
fn validate_jxl_dimensions(
    width: u32,
    height: u32,
    kind: EpcImageKind,
) -> Result<JxlInfo, JxlValidationError> {
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

/// Validates user-controlled JPEG XL encoder options.
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

/// Decodes a supported source image to RGBA8 pixels.
#[cfg(feature = "jxl-encode-libjxl")]
fn decode_source_image_rgba8(path: &Path) -> Result<RgbaImage, EncodeError> {
    let image = image::ImageReader::open(path)
        .map_err(EncodeError::Io)?
        .decode()
        .map_err(|error| EncodeError::DecodeImage(error.to_string()))?
        .to_rgba8();
    Ok(RgbaImage {
        width: image.width(),
        height: image.height(),
        pixels: image.into_raw(),
    })
}

#[cfg(feature = "jxl-encode-libjxl")]
fn is_jxl_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("jxl"))
        .unwrap_or(false)
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

/// Writes an RGBA8 image to PNG for debugging or preview generation.
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

#[cfg(feature = "jxl-encode-libjxl")]
mod libjxl_encoder {
    use super::{EncodeError, EncodeOptions, RgbaImage};
    use std::ffi::{c_int, c_uint, c_void};
    use std::mem::MaybeUninit;
    use std::ptr;

    const JXL_FALSE: c_int = 0;
    const JXL_TRUE: c_int = 1;
    const JXL_ENC_SUCCESS: c_int = 0;
    const JXL_ENC_ERROR: c_int = 1;
    const JXL_ENC_NEED_MORE_OUTPUT: c_int = 2;
    const JXL_TYPE_UINT8: c_int = 2;
    const JXL_NATIVE_ENDIAN: c_int = 0;
    const JXL_ORIENT_IDENTITY: c_int = 1;
    const JXL_ENC_FRAME_SETTING_EFFORT: c_int = 0;

    enum JxlEncoder {}
    enum JxlEncoderFrameSettings {}

    type JxlParallelRetCode = c_int;
    type JxlParallelRunInit =
        Option<unsafe extern "C" fn(*mut c_void, usize) -> JxlParallelRetCode>;
    type JxlParallelRunFunction = Option<unsafe extern "C" fn(*mut c_void, u32)>;
    type JxlParallelRunner = Option<
        unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            JxlParallelRunInit,
            JxlParallelRunFunction,
            u32,
            u32,
        ) -> JxlParallelRetCode,
    >;

    #[repr(C)]
    struct JxlPixelFormat {
        num_channels: u32,
        data_type: c_int,
        endianness: c_int,
        align: usize,
    }

    #[repr(C)]
    struct JxlPreviewHeader {
        xsize: u32,
        ysize: u32,
    }

    #[repr(C)]
    struct JxlAnimationHeader {
        tps_numerator: u32,
        tps_denominator: u32,
        num_loops: u32,
        have_timecodes: c_int,
    }

    #[repr(C)]
    struct JxlBasicInfo {
        have_container: c_int,
        xsize: u32,
        ysize: u32,
        bits_per_sample: u32,
        exponent_bits_per_sample: u32,
        intensity_target: f32,
        min_nits: f32,
        relative_to_max_display: c_int,
        linear_below: f32,
        uses_original_profile: c_int,
        have_preview: c_int,
        have_animation: c_int,
        orientation: c_int,
        num_color_channels: u32,
        num_extra_channels: u32,
        alpha_bits: u32,
        alpha_exponent_bits: u32,
        alpha_premultiplied: c_int,
        preview: JxlPreviewHeader,
        animation: JxlAnimationHeader,
        intrinsic_xsize: u32,
        intrinsic_ysize: u32,
        padding: [u8; 100],
    }

    #[link(name = "jxl")]
    #[link(name = "jxl_threads")]
    unsafe extern "C" {
        fn JxlEncoderCreate(memory_manager: *const c_void) -> *mut JxlEncoder;
        fn JxlEncoderDestroy(enc: *mut JxlEncoder);
        fn JxlEncoderSetParallelRunner(
            enc: *mut JxlEncoder,
            parallel_runner: JxlParallelRunner,
            parallel_runner_opaque: *mut c_void,
        ) -> c_int;
        fn JxlEncoderGetError(enc: *mut JxlEncoder) -> c_int;
        fn JxlEncoderProcessOutput(
            enc: *mut JxlEncoder,
            next_out: *mut *mut u8,
            avail_out: *mut usize,
        ) -> c_int;
        fn JxlEncoderCloseInput(enc: *mut JxlEncoder);
        fn JxlEncoderInitBasicInfo(info: *mut JxlBasicInfo);
        fn JxlEncoderSetBasicInfo(enc: *mut JxlEncoder, info: *const JxlBasicInfo) -> c_int;
        fn JxlEncoderFrameSettingsCreate(
            enc: *mut JxlEncoder,
            source: *const JxlEncoderFrameSettings,
        ) -> *mut JxlEncoderFrameSettings;
        fn JxlEncoderFrameSettingsSetOption(
            frame_settings: *mut JxlEncoderFrameSettings,
            option: c_int,
            value: i64,
        ) -> c_int;
        fn JxlEncoderSetFrameLossless(
            frame_settings: *mut JxlEncoderFrameSettings,
            lossless: c_int,
        ) -> c_int;
        fn JxlEncoderSetFrameDistance(
            frame_settings: *mut JxlEncoderFrameSettings,
            distance: f32,
        ) -> c_int;
        fn JxlEncoderDistanceFromQuality(quality: f32) -> f32;
        fn JxlEncoderAddImageFrame(
            frame_settings: *const JxlEncoderFrameSettings,
            pixel_format: *const JxlPixelFormat,
            buffer: *const c_void,
            size: usize,
        ) -> c_int;
        fn JxlThreadParallelRunner(
            runner_opaque: *mut c_void,
            jpegxl_opaque: *mut c_void,
            init: JxlParallelRunInit,
            func: JxlParallelRunFunction,
            start_range: c_uint,
            end_range: c_uint,
        ) -> JxlParallelRetCode;
        fn JxlThreadParallelRunnerCreate(
            memory_manager: *const c_void,
            num_worker_threads: usize,
        ) -> *mut c_void;
        fn JxlThreadParallelRunnerDestroy(runner_opaque: *mut c_void);
        fn JxlThreadParallelRunnerDefaultNumWorkerThreads() -> usize;
    }

    struct Encoder(*mut JxlEncoder);

    impl Encoder {
        fn new() -> Result<Self, EncodeError> {
            let enc = unsafe { JxlEncoderCreate(ptr::null()) };
            if enc.is_null() {
                Err(EncodeError::JxlEncode(
                    "libjxl encoder allocation failed".to_string(),
                ))
            } else {
                Ok(Self(enc))
            }
        }

        fn last_error(&self) -> c_int {
            unsafe { JxlEncoderGetError(self.0) }
        }
    }

    impl Drop for Encoder {
        fn drop(&mut self) {
            unsafe { JxlEncoderDestroy(self.0) };
        }
    }

    struct ThreadRunner(*mut c_void);

    impl ThreadRunner {
        fn new() -> Option<Self> {
            let default_threads = unsafe { JxlThreadParallelRunnerDefaultNumWorkerThreads() };
            let threads = std::thread::available_parallelism()
                .map(|parallelism| parallelism.get())
                .unwrap_or(default_threads)
                .min(default_threads.max(1));
            let runner = unsafe { JxlThreadParallelRunnerCreate(ptr::null(), threads) };
            if runner.is_null() {
                None
            } else {
                Some(Self(runner))
            }
        }
    }

    impl Drop for ThreadRunner {
        fn drop(&mut self) {
            unsafe { JxlThreadParallelRunnerDestroy(self.0) };
        }
    }

    pub(super) fn encode_rgba8(
        image: &RgbaImage,
        options: &EncodeOptions,
    ) -> Result<Vec<u8>, EncodeError> {
        encode_image_frame(image, options, true)
    }

    pub(super) fn encode_rgba8_as_rgb(
        image: &RgbaImage,
        options: &EncodeOptions,
    ) -> Result<Vec<u8>, EncodeError> {
        encode_image_frame(image, options, false)
    }

    fn encode_image_frame(
        image: &RgbaImage,
        options: &EncodeOptions,
        include_alpha: bool,
    ) -> Result<Vec<u8>, EncodeError> {
        let encoder = Encoder::new()?;
        let _runner = configure_runner(&encoder)?;
        set_basic_info(&encoder, image, include_alpha)?;
        let frame_settings = create_frame_settings(&encoder)?;
        configure_frame(&encoder, frame_settings, options)?;

        let rgb_pixels;
        let (pixels, num_channels) = if include_alpha {
            (image.pixels.as_slice(), 4)
        } else {
            rgb_pixels = rgba_to_rgb_pixels(image)?;
            (rgb_pixels.as_slice(), 3)
        };
        let pixel_format = JxlPixelFormat {
            num_channels,
            data_type: JXL_TYPE_UINT8,
            endianness: JXL_NATIVE_ENDIAN,
            align: 0,
        };
        check_status(
            unsafe {
                JxlEncoderAddImageFrame(
                    frame_settings,
                    &pixel_format,
                    pixels.as_ptr().cast(),
                    pixels.len(),
                )
            },
            &encoder,
            "JxlEncoderAddImageFrame",
        )?;
        unsafe { JxlEncoderCloseInput(encoder.0) };
        collect_output(&encoder)
    }

    fn rgba_to_rgb_pixels(image: &RgbaImage) -> Result<Vec<u8>, EncodeError> {
        let pixel_count = usize::try_from(image.width)
            .ok()
            .and_then(|width| {
                usize::try_from(image.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or(EncodeError::InvalidOptions(
                "image dimensions are too large",
            ))?;
        let mut rgb = Vec::with_capacity(pixel_count.checked_mul(3).ok_or(
            EncodeError::InvalidOptions("image dimensions are too large"),
        )?);
        for pixel in image.pixels.chunks_exact(4) {
            rgb.extend_from_slice(&pixel[..3]);
        }
        Ok(rgb)
    }

    fn configure_runner(encoder: &Encoder) -> Result<Option<ThreadRunner>, EncodeError> {
        let Some(runner) = ThreadRunner::new() else {
            return Ok(None);
        };
        check_status(
            unsafe {
                JxlEncoderSetParallelRunner(encoder.0, Some(JxlThreadParallelRunner), runner.0)
            },
            encoder,
            "JxlEncoderSetParallelRunner",
        )?;
        Ok(Some(runner))
    }

    fn set_basic_info(
        encoder: &Encoder,
        image: &RgbaImage,
        include_alpha: bool,
    ) -> Result<(), EncodeError> {
        let mut info = MaybeUninit::<JxlBasicInfo>::uninit();
        unsafe { JxlEncoderInitBasicInfo(info.as_mut_ptr()) };
        let mut info = unsafe { info.assume_init() };
        info.xsize = image.width;
        info.ysize = image.height;
        info.bits_per_sample = 8;
        info.exponent_bits_per_sample = 0;
        info.orientation = JXL_ORIENT_IDENTITY;
        info.num_color_channels = 3;
        info.num_extra_channels = if include_alpha { 1 } else { 0 };
        info.alpha_bits = if include_alpha { 8 } else { 0 };
        info.alpha_exponent_bits = 0;
        info.alpha_premultiplied = JXL_FALSE;
        info.uses_original_profile = JXL_TRUE;
        check_status(
            unsafe { JxlEncoderSetBasicInfo(encoder.0, &info) },
            encoder,
            "JxlEncoderSetBasicInfo",
        )
    }

    fn create_frame_settings(
        encoder: &Encoder,
    ) -> Result<*mut JxlEncoderFrameSettings, EncodeError> {
        let frame_settings = unsafe { JxlEncoderFrameSettingsCreate(encoder.0, ptr::null()) };
        if frame_settings.is_null() {
            Err(EncodeError::JxlEncode(
                "JxlEncoderFrameSettingsCreate returned null".to_string(),
            ))
        } else {
            Ok(frame_settings)
        }
    }

    fn configure_frame(
        encoder: &Encoder,
        frame_settings: *mut JxlEncoderFrameSettings,
        options: &EncodeOptions,
    ) -> Result<(), EncodeError> {
        if let Some(effort) = options.effort {
            check_status(
                unsafe {
                    JxlEncoderFrameSettingsSetOption(
                        frame_settings,
                        JXL_ENC_FRAME_SETTING_EFFORT,
                        i64::from(effort),
                    )
                },
                encoder,
                "JxlEncoderFrameSettingsSetOption(EFFORT)",
            )?;
        }

        let distance = match (options.quality, options.distance) {
            (Some(quality), _) => unsafe { JxlEncoderDistanceFromQuality(quality) },
            (None, Some(distance)) => distance,
            (None, None) => 1.0,
        };
        if distance <= 0.0 {
            check_status(
                unsafe { JxlEncoderSetFrameLossless(frame_settings, JXL_TRUE) },
                encoder,
                "JxlEncoderSetFrameLossless",
            )
        } else {
            check_status(
                unsafe { JxlEncoderSetFrameDistance(frame_settings, distance) },
                encoder,
                "JxlEncoderSetFrameDistance",
            )
        }
    }

    fn collect_output(encoder: &Encoder) -> Result<Vec<u8>, EncodeError> {
        let mut output = Vec::new();
        loop {
            let start = output.len();
            output.resize(start + 16 * 1024, 0);
            let mut next_out = output[start..].as_mut_ptr();
            let mut avail_out = output.len() - start;
            let status =
                unsafe { JxlEncoderProcessOutput(encoder.0, &mut next_out, &mut avail_out) };
            let written = output.len() - start - avail_out;
            output.truncate(start + written);

            match status {
                JXL_ENC_SUCCESS => return Ok(output),
                JXL_ENC_NEED_MORE_OUTPUT => {}
                JXL_ENC_ERROR => {
                    return Err(EncodeError::JxlEncode(format!(
                        "JxlEncoderProcessOutput failed with error {}",
                        encoder.last_error()
                    )));
                }
                other => {
                    return Err(EncodeError::JxlEncode(format!(
                        "JxlEncoderProcessOutput returned unexpected status {other}"
                    )));
                }
            }
        }
    }

    fn check_status(status: c_int, encoder: &Encoder, operation: &str) -> Result<(), EncodeError> {
        if status == JXL_ENC_SUCCESS {
            Ok(())
        } else {
            Err(EncodeError::JxlEncode(format!(
                "{operation} failed with status {status} and error {}",
                encoder.last_error()
            )))
        }
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
                "{}/../../testcases/images/arc-de-triomphe-paris.jxl",
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
    fn reads_supported_image_metadata() {
        let jpeg = read_image_metadata_file(format!(
            "{}/../../testcases/images/arc-de-triomphe-paris.jpeg",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        assert_eq!(jpeg.width, 1100);
        assert_eq!(jpeg.height, 732);
        assert_eq!(jpeg.format, "JPEG");
        assert_eq!(jpeg.encoding, "jpeg-baseline");
        assert_eq!(jpeg.bits_per_pixel, 24);

        let jxl = read_image_metadata_file(format!(
            "{}/../../testcases/images/arc-de-triomphe-paris.jxl",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        assert_eq!(jxl.width, 1100);
        assert_eq!(jxl.height, 732);
        assert_eq!(jxl.format, "JPEG XL");
        assert_eq!(jxl.encoding, "jpeg-xl");
        assert_eq!(jxl.bits_per_sample, 8);
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
            "{}/../../testcases/images/arc-de-triomphe-paris.jxl",
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
        zip.write_all(include_bytes!(
            "../../../testcases/images/arc-de-triomphe-paris.jxl"
        ))
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
    fn encodes_rgba8_with_libjxl_encoder() {
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
    fn encodes_thumbnail_from_cover_bytes_with_libjxl_encoder() {
        let bytes = encode_thumbnail_from_cover_jxl_bytes(
            include_bytes!("../../../testcases/images/arc-de-triomphe-paris.jxl"),
            &EncodeOptions::default(),
        )
        .unwrap();
        let decoded =
            decode_jxl_rgba8(&bytes, EpcImageKind::Thumbnail, RenderOptions::original()).unwrap();
        let metadata = read_image_metadata(&bytes).unwrap();

        assert!(decoded.width <= MAX_THUMBNAIL_DIMENSION);
        assert!(decoded.height <= MAX_THUMBNAIL_DIMENSION);
        assert_eq!(decoded.pixels.len(), decoded.expected_len());
        assert_eq!(metadata.alpha_bits, 0);
    }
}
