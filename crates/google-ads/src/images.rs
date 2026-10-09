//! What an uploaded file is, read from its first bytes, and the shapes
//! Google Ads takes images in. Google Ads takes JPEG, PNG, and GIF images,
//! and judges them by their size in pixels.
//!
//! The port of `app/services/builtin/google_ads/images.ts`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Jpeg,
    Png,
    Gif,
}

impl ImageFormat {
    /// `jpeg`, `png` or `gif`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Gif => "gif",
        }
    }
}

/// What an uploaded file is, read from its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
}

const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// `bytes.readUInt16BE(at)`, for a place the caller knows the file reaches.
fn u16_be(bytes: &[u8], at: usize) -> Option<u32> {
    let pair: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(u32::from(u16::from_be_bytes(pair)))
}

fn u16_le(bytes: &[u8], at: usize) -> Option<u32> {
    let pair: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(u32::from(u16::from_le_bytes(pair)))
}

fn u32_be(bytes: &[u8], at: usize) -> Option<u32> {
    let four: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_be_bytes(four))
}

fn png_info(bytes: &[u8]) -> Option<ImageInfo> {
    if bytes.len() < 24 || bytes[..8] != PNG_SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    Some(ImageInfo {
        format: ImageFormat::Png,
        width: u32_be(bytes, 16)?,
        height: u32_be(bytes, 20)?,
    })
}

fn gif_info(bytes: &[u8]) -> Option<ImageInfo> {
    if bytes.len() < 10 || !(bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return None;
    }
    Some(ImageInfo {
        format: ImageFormat::Gif,
        width: u16_le(bytes, 6)?,
        height: u16_le(bytes, 8)?,
    })
}

/// The frame headers that carry the size: every start of frame but the tables among them.
fn is_start_of_frame(marker: u8) -> bool {
    (0xc0..=0xcf).contains(&marker) && marker != 0xc4 && marker != 0xc8 && marker != 0xcc
}

fn jpeg_info(bytes: &[u8]) -> Option<ImageInfo> {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return None;
    }

    let mut offset = 2;
    while offset + 9 <= bytes.len() {
        if bytes[offset] != 0xff {
            return None;
        }
        let marker = bytes[offset + 1];
        // Padding, and markers that stand alone.
        if marker == 0xff {
            offset += 1;
            continue;
        }
        if marker == 0x01 || (0xd0..=0xd8).contains(&marker) {
            offset += 2;
            continue;
        }

        let length = u16_be(bytes, offset + 2)?;
        if length < 2 {
            return None;
        }
        if is_start_of_frame(marker) {
            return Some(ImageInfo {
                format: ImageFormat::Jpeg,
                height: u16_be(bytes, offset + 5)?,
                width: u16_be(bytes, offset + 7)?,
            });
        }
        offset += 2 + length as usize;
    }
    None
}

/// `None` for anything but a JPEG, PNG, or GIF image with a size.
pub fn image_info(bytes: &[u8]) -> Option<ImageInfo> {
    png_info(bytes)
        .or_else(|| gif_info(bytes))
        .or_else(|| jpeg_info(bytes))
        .filter(|info| info.width > 0 && info.height > 0)
}

/// One of the shapes Google Ads takes images in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageShape {
    pub name: &'static str,
    pub label: &'static str,
    pub ratio: f64,
    pub min_width: u32,
    pub min_height: u32,
}

/// The shapes Google Ads takes images in, with the smallest size it accepts
/// for each. An image has a shape when its ratio is within 1% of it, as Google
/// measures it.
pub const IMAGE_SHAPES: [ImageShape; 4] = [
    ImageShape {
        name: "landscape",
        label: "landscape (1.91:1)",
        ratio: 1.91,
        min_width: 600,
        min_height: 314,
    },
    ImageShape {
        name: "square",
        label: "square (1:1)",
        ratio: 1.0,
        min_width: 300,
        min_height: 300,
    },
    ImageShape {
        name: "wide_logo",
        label: "wide logo (4:1)",
        ratio: 4.0,
        min_width: 512,
        min_height: 128,
    },
    ImageShape {
        name: "portrait",
        label: "portrait (4:5)",
        ratio: 0.8,
        min_width: 480,
        min_height: 600,
    },
];

/// A square image this small is still a logo, but not a marketing image.
pub const SQUARE_LOGO_MIN_PIXELS: u32 = 128;

/// The shape an image has, whatever its size. `None` when Google Ads has no
/// use for it. The sizes are numbers as JavaScript holds them: the ones of an
/// uploaded file, or what Google Ads says of an asset.
pub fn image_shape(width: f64, height: f64) -> Option<&'static ImageShape> {
    let ratio = width / height;
    IMAGE_SHAPES
        .iter()
        .find(|shape| (ratio / shape.ratio - 1.0).abs() <= 0.01)
}
