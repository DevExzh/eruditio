//! Bitflags filter controlling which categories of EPUB manifest resources
//! the reader should inflate.
//!
//! When converting to text-only formats (TXT, Markdown, etc.) there is no need
//! to decompress images, fonts, or audio/video resources.  `LoadFilter` lets the
//! pipeline communicate this to the reader so it can skip unnecessary work.

use crate::domain::format::Format;

/// Media types that belong to the TEXT category.
const TEXT_MEDIA_TYPES: &[&str] = &[
    "application/xhtml+xml",
    "text/html",
    "text/css",
    "application/x-dtbncx+xml",
    "application/xml",
    "text/xml",
    "application/smil+xml",
    "application/javascript",
    "text/javascript",
    "text/plain",
    "application/json",
];

/// A lightweight bitflags type that describes which categories of manifest
/// resources should be loaded by the EPUB reader.
///
/// Combine categories with `|` and test membership with [`contains`](Self::contains).
///
/// ```
/// # use eruditio::pipeline::load_filter::LoadFilter;
/// let filter = LoadFilter::TEXT | LoadFilter::IMAGES;
/// assert!(filter.contains(LoadFilter::TEXT));
/// assert!(!filter.contains(LoadFilter::FONTS));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadFilter(u8);

impl LoadFilter {
    /// XHTML, CSS, NCX, XML, and other textual resources.
    pub const TEXT: Self = Self(0b0001);
    /// All `image/*` types.
    pub const IMAGES: Self = Self(0b0010);
    /// Font resources (`font/*`, `application/font-*`, etc.).
    pub const FONTS: Self = Self(0b0100);
    /// Audio and video resources (`audio/*`, `video/*`).
    pub const MEDIA: Self = Self(0b1000);
    /// Load everything.
    pub const ALL: Self = Self(0b1111);
    /// Load nothing (useful as a starting point for building a custom filter).
    pub const NONE: Self = Self(0b0000);

    /// Returns `true` if `self` contains all bits set in `other`.
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Returns the appropriate filter for a given output format.
    ///
    /// Text-only targets get `TEXT`; formats that embed images get
    /// `TEXT | IMAGES`; container formats that preserve everything get `ALL`.
    pub fn for_output_format(format: Format) -> Self {
        match format {
            // Text-only outputs — no embedded resources.
            Format::Txt | Format::Md | Format::Pml | Format::Pdb | Format::Tcr | Format::Txtz => {
                Self::TEXT
            }

            // Formats that embed images alongside text.
            Format::Rtf
            | Format::Fb2
            | Format::Mobi
            | Format::Html
            | Format::Lrf
            | Format::Snb
            | Format::Rb
            | Format::Azw
            | Format::Azw3
            | Format::Prc
            | Format::Pdf
            | Format::Htmlz => Self(Self::TEXT.0 | Self::IMAGES.0),

            // Container / archive formats — keep everything.
            Format::Epub
            | Format::Lit
            | Format::Oeb
            | Format::Kepub
            | Format::Fbz
            | Format::Pmlz
            | Format::Cbz
            | Format::Cbr
            | Format::Cb7
            | Format::Cbc => Self::ALL,

            // Safe fallback for complex/unknown targets.
            Format::Azw4 | Format::Docx | Format::Odt | Format::Djvu | Format::Chm
            | Format::Zip => Self::ALL,
        }
    }

    /// Returns `true` if the filter would accept a resource with the given
    /// MIME media type.
    ///
    /// The matching rules are:
    /// - **TEXT**: any type listed in [`TEXT_MEDIA_TYPES`], or any `text/*`.
    /// - **IMAGES**: any `image/*`.
    /// - **FONTS**: `font/*`, `application/font-*`, `application/x-font-*`,
    ///   or `application/vnd.ms-opentype`.
    /// - **MEDIA**: `audio/*` or `video/*`.
    ///
    /// Unknown media types that do not match any category return `false`.
    pub fn matches_media_type(&self, media_type: &str) -> bool {
        if self.contains(Self::TEXT) && is_text(media_type) {
            return true;
        }
        if self.contains(Self::IMAGES) && is_image(media_type) {
            return true;
        }
        if self.contains(Self::FONTS) && is_font(media_type) {
            return true;
        }
        if self.contains(Self::MEDIA) && is_media(media_type) {
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Bitwise operator impls
// ---------------------------------------------------------------------------

impl core::ops::BitOr for LoadFilter {
    type Output = Self;

    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitAnd for LoadFilter {
    type Output = Self;

    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl core::ops::BitOrAssign for LoadFilter {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl core::ops::BitAndAssign for LoadFilter {
    #[inline]
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl core::ops::Not for LoadFilter {
    type Output = Self;

    #[inline]
    fn not(self) -> Self {
        // Only invert within the valid 4-bit range.
        Self(!self.0 & 0b1111)
    }
}

// ---------------------------------------------------------------------------
// Category helpers
// ---------------------------------------------------------------------------

#[inline]
fn is_text(media_type: &str) -> bool {
    // Exact matches first (the common case).
    if TEXT_MEDIA_TYPES.contains(&media_type) {
        return true;
    }
    // Fallback: any text/* subtype.
    media_type.starts_with("text/")
}

#[inline]
fn is_image(media_type: &str) -> bool {
    media_type.starts_with("image/")
}

#[inline]
fn is_font(media_type: &str) -> bool {
    media_type.starts_with("font/")
        || media_type.starts_with("application/font-")
        || media_type.starts_with("application/x-font-")
        || media_type == "application/vnd.ms-opentype"
}

#[inline]
fn is_media(media_type: &str) -> bool {
    media_type.starts_with("audio/") || media_type.starts_with("video/")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- matches_media_type: TEXT ----

    #[test]
    fn text_matches_xhtml() {
        assert!(LoadFilter::TEXT.matches_media_type("application/xhtml+xml"));
    }

    #[test]
    fn text_matches_css() {
        assert!(LoadFilter::TEXT.matches_media_type("text/css"));
    }

    #[test]
    fn text_matches_ncx() {
        assert!(LoadFilter::TEXT.matches_media_type("application/x-dtbncx+xml"));
    }

    #[test]
    fn text_matches_xml() {
        assert!(LoadFilter::TEXT.matches_media_type("application/xml"));
        assert!(LoadFilter::TEXT.matches_media_type("text/xml"));
    }

    #[test]
    fn text_matches_smil() {
        assert!(LoadFilter::TEXT.matches_media_type("application/smil+xml"));
    }

    #[test]
    fn text_matches_javascript() {
        assert!(LoadFilter::TEXT.matches_media_type("application/javascript"));
        assert!(LoadFilter::TEXT.matches_media_type("text/javascript"));
    }

    #[test]
    fn text_matches_plain() {
        assert!(LoadFilter::TEXT.matches_media_type("text/plain"));
    }

    #[test]
    fn text_matches_json() {
        assert!(LoadFilter::TEXT.matches_media_type("application/json"));
    }

    #[test]
    fn text_matches_any_text_subtype() {
        assert!(LoadFilter::TEXT.matches_media_type("text/markdown"));
        assert!(LoadFilter::TEXT.matches_media_type("text/x-custom"));
    }

    #[test]
    fn text_rejects_image() {
        assert!(!LoadFilter::TEXT.matches_media_type("image/png"));
    }

    // ---- matches_media_type: IMAGES ----

    #[test]
    fn images_matches_png() {
        assert!(LoadFilter::IMAGES.matches_media_type("image/png"));
    }

    #[test]
    fn images_matches_jpeg() {
        assert!(LoadFilter::IMAGES.matches_media_type("image/jpeg"));
    }

    #[test]
    fn images_matches_svg() {
        assert!(LoadFilter::IMAGES.matches_media_type("image/svg+xml"));
    }

    #[test]
    fn images_matches_gif() {
        assert!(LoadFilter::IMAGES.matches_media_type("image/gif"));
    }

    #[test]
    fn images_matches_webp() {
        assert!(LoadFilter::IMAGES.matches_media_type("image/webp"));
    }

    #[test]
    fn images_rejects_text() {
        assert!(!LoadFilter::IMAGES.matches_media_type("text/html"));
    }

    // ---- matches_media_type: FONTS ----

    #[test]
    fn fonts_matches_woff() {
        assert!(LoadFilter::FONTS.matches_media_type("font/woff"));
        assert!(LoadFilter::FONTS.matches_media_type("font/woff2"));
    }

    #[test]
    fn fonts_matches_opentype() {
        assert!(LoadFilter::FONTS.matches_media_type("font/otf"));
        assert!(LoadFilter::FONTS.matches_media_type("application/font-sfnt"));
    }

    #[test]
    fn fonts_matches_application_font_prefix() {
        assert!(LoadFilter::FONTS.matches_media_type("application/font-woff"));
    }

    #[test]
    fn fonts_matches_x_font_prefix() {
        assert!(LoadFilter::FONTS.matches_media_type("application/x-font-ttf"));
        assert!(LoadFilter::FONTS.matches_media_type("application/x-font-opentype"));
    }

    #[test]
    fn fonts_matches_ms_opentype() {
        assert!(LoadFilter::FONTS.matches_media_type("application/vnd.ms-opentype"));
    }

    #[test]
    fn fonts_rejects_image() {
        assert!(!LoadFilter::FONTS.matches_media_type("image/png"));
    }

    // ---- matches_media_type: MEDIA ----

    #[test]
    fn media_matches_audio() {
        assert!(LoadFilter::MEDIA.matches_media_type("audio/mpeg"));
        assert!(LoadFilter::MEDIA.matches_media_type("audio/ogg"));
    }

    #[test]
    fn media_matches_video() {
        assert!(LoadFilter::MEDIA.matches_media_type("video/mp4"));
        assert!(LoadFilter::MEDIA.matches_media_type("video/webm"));
    }

    #[test]
    fn media_rejects_text() {
        assert!(!LoadFilter::MEDIA.matches_media_type("text/html"));
    }

    // ---- matches_media_type: combined filters ----

    #[test]
    fn text_images_matches_both() {
        let filter = LoadFilter::TEXT | LoadFilter::IMAGES;
        assert!(filter.matches_media_type("text/html"));
        assert!(filter.matches_media_type("image/png"));
        assert!(!filter.matches_media_type("font/woff"));
        assert!(!filter.matches_media_type("audio/mpeg"));
    }

    #[test]
    fn all_matches_everything() {
        assert!(LoadFilter::ALL.matches_media_type("text/html"));
        assert!(LoadFilter::ALL.matches_media_type("image/jpeg"));
        assert!(LoadFilter::ALL.matches_media_type("font/woff2"));
        assert!(LoadFilter::ALL.matches_media_type("audio/mpeg"));
        assert!(LoadFilter::ALL.matches_media_type("video/mp4"));
    }

    #[test]
    fn none_matches_nothing() {
        assert!(!LoadFilter::NONE.matches_media_type("text/html"));
        assert!(!LoadFilter::NONE.matches_media_type("image/png"));
        assert!(!LoadFilter::NONE.matches_media_type("font/woff"));
        assert!(!LoadFilter::NONE.matches_media_type("audio/mpeg"));
    }

    // ---- Edge cases ----

    #[test]
    fn unknown_media_type_matches_nothing() {
        assert!(!LoadFilter::ALL.matches_media_type("application/octet-stream"));
        assert!(!LoadFilter::ALL.matches_media_type("application/zip"));
        assert!(!LoadFilter::ALL.matches_media_type(""));
        assert!(!LoadFilter::ALL.matches_media_type("something/weird"));
    }

    #[test]
    fn application_json_only_matched_by_text() {
        assert!(LoadFilter::TEXT.matches_media_type("application/json"));
        assert!(!LoadFilter::IMAGES.matches_media_type("application/json"));
        assert!(!LoadFilter::FONTS.matches_media_type("application/json"));
        assert!(!LoadFilter::MEDIA.matches_media_type("application/json"));
    }

    // ---- for_output_format ----

    #[test]
    fn txt_gets_text_only() {
        assert_eq!(LoadFilter::for_output_format(Format::Txt), LoadFilter::TEXT);
    }

    #[test]
    fn md_gets_text_only() {
        assert_eq!(LoadFilter::for_output_format(Format::Md), LoadFilter::TEXT);
    }

    #[test]
    fn tcr_gets_text_only() {
        assert_eq!(LoadFilter::for_output_format(Format::Tcr), LoadFilter::TEXT);
    }

    #[test]
    fn pml_gets_text_only() {
        assert_eq!(LoadFilter::for_output_format(Format::Pml), LoadFilter::TEXT);
    }

    #[test]
    fn pdb_gets_text_only() {
        assert_eq!(LoadFilter::for_output_format(Format::Pdb), LoadFilter::TEXT);
    }

    #[test]
    fn txtz_gets_text_only() {
        assert_eq!(
            LoadFilter::for_output_format(Format::Txtz),
            LoadFilter::TEXT
        );
    }

    #[test]
    fn rtf_gets_text_images() {
        let expected = LoadFilter::TEXT | LoadFilter::IMAGES;
        assert_eq!(LoadFilter::for_output_format(Format::Rtf), expected);
    }

    #[test]
    fn mobi_gets_text_images() {
        let expected = LoadFilter::TEXT | LoadFilter::IMAGES;
        assert_eq!(LoadFilter::for_output_format(Format::Mobi), expected);
    }

    #[test]
    fn html_gets_text_images() {
        let expected = LoadFilter::TEXT | LoadFilter::IMAGES;
        assert_eq!(LoadFilter::for_output_format(Format::Html), expected);
    }

    #[test]
    fn fb2_gets_text_images() {
        let expected = LoadFilter::TEXT | LoadFilter::IMAGES;
        assert_eq!(LoadFilter::for_output_format(Format::Fb2), expected);
    }

    #[test]
    fn pdf_gets_text_images() {
        let expected = LoadFilter::TEXT | LoadFilter::IMAGES;
        assert_eq!(LoadFilter::for_output_format(Format::Pdf), expected);
    }

    #[test]
    fn htmlz_gets_text_images() {
        let expected = LoadFilter::TEXT | LoadFilter::IMAGES;
        assert_eq!(LoadFilter::for_output_format(Format::Htmlz), expected);
    }

    #[test]
    fn epub_gets_all() {
        assert_eq!(LoadFilter::for_output_format(Format::Epub), LoadFilter::ALL);
    }

    #[test]
    fn kepub_gets_all() {
        assert_eq!(
            LoadFilter::for_output_format(Format::Kepub),
            LoadFilter::ALL
        );
    }

    #[test]
    fn cbz_gets_all() {
        assert_eq!(LoadFilter::for_output_format(Format::Cbz), LoadFilter::ALL);
    }

    #[test]
    fn docx_gets_all() {
        assert_eq!(LoadFilter::for_output_format(Format::Docx), LoadFilter::ALL);
    }

    #[test]
    fn zip_gets_all() {
        assert_eq!(LoadFilter::for_output_format(Format::Zip), LoadFilter::ALL);
    }

    // ---- Bitwise operations ----

    #[test]
    fn bitor_combines_flags() {
        let filter = LoadFilter::TEXT | LoadFilter::FONTS;
        assert!(filter.contains(LoadFilter::TEXT));
        assert!(filter.contains(LoadFilter::FONTS));
        assert!(!filter.contains(LoadFilter::IMAGES));
    }

    #[test]
    fn bitand_intersects_flags() {
        let a = LoadFilter::TEXT | LoadFilter::IMAGES;
        let b = LoadFilter::IMAGES | LoadFilter::FONTS;
        let intersection = a & b;
        assert_eq!(intersection, LoadFilter::IMAGES);
    }

    #[test]
    fn bitor_assign_works() {
        let mut filter = LoadFilter::TEXT;
        filter |= LoadFilter::IMAGES;
        assert!(filter.contains(LoadFilter::TEXT));
        assert!(filter.contains(LoadFilter::IMAGES));
    }

    #[test]
    fn bitand_assign_works() {
        let mut filter = LoadFilter::ALL;
        filter &= LoadFilter::TEXT;
        assert_eq!(filter, LoadFilter::TEXT);
    }

    #[test]
    fn not_inverts_flags() {
        let filter = !LoadFilter::TEXT;
        assert!(!filter.contains(LoadFilter::TEXT));
        assert!(filter.contains(LoadFilter::IMAGES));
        assert!(filter.contains(LoadFilter::FONTS));
        assert!(filter.contains(LoadFilter::MEDIA));
    }

    #[test]
    fn contains_self() {
        assert!(LoadFilter::ALL.contains(LoadFilter::ALL));
        assert!(LoadFilter::TEXT.contains(LoadFilter::TEXT));
        assert!(LoadFilter::NONE.contains(LoadFilter::NONE));
    }

    #[test]
    fn none_is_contained_in_everything() {
        assert!(LoadFilter::TEXT.contains(LoadFilter::NONE));
        assert!(LoadFilter::ALL.contains(LoadFilter::NONE));
        assert!(LoadFilter::NONE.contains(LoadFilter::NONE));
    }

    #[test]
    fn all_contains_every_flag() {
        assert!(LoadFilter::ALL.contains(LoadFilter::TEXT));
        assert!(LoadFilter::ALL.contains(LoadFilter::IMAGES));
        assert!(LoadFilter::ALL.contains(LoadFilter::FONTS));
        assert!(LoadFilter::ALL.contains(LoadFilter::MEDIA));
    }
}
