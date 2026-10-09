//! What an uploaded file is, and the shapes Google Ads takes: the port of the
//! "images" group of `tests/unit/builtin_google_ads.spec.ts`, then both
//! against what the TypeScript answers.
//!
//! `fixtures/images.json` holds what `imageInfo` and `imageShape` of
//! `app/services/builtin/google_ads/images.ts` answer, run by Node: for some
//! five hundred files, whole, cut short, or made at random to look like a
//! JPEG, and for sizes on either side of each shape. The script that wrote
//! it is not part of the repository, since it runs Node on the TypeScript
//! app.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mymcps_google_ads::images::{
    IMAGE_SHAPES, ImageFormat, ImageInfo, SQUARE_LOGO_MIN_PIXELS, image_info, image_shape,
};
use mymcps_vine as vine;
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("fixtures/images.json");

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = vec![0; 33];
    bytes[..8].copy_from_slice(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    bytes[8..12].copy_from_slice(&13_u32.to_be_bytes());
    bytes[12..16].copy_from_slice(b"IHDR");
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    bytes
}

/// The start of a JPEG file: an application segment, then the frame header that carries the size.
fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x4a, 0x46];
    let mut frame = [0; 11];
    frame[..2].copy_from_slice(&0xffc2_u16.to_be_bytes());
    frame[2..4].copy_from_slice(&9_u16.to_be_bytes());
    frame[4] = 8;
    frame[5..7].copy_from_slice(&height.to_be_bytes());
    frame[7..9].copy_from_slice(&width.to_be_bytes());
    bytes.extend(frame);
    bytes
}

fn gif(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0; 13];
    bytes[..6].copy_from_slice(b"GIF89a");
    bytes[6..8].copy_from_slice(&width.to_le_bytes());
    bytes[8..10].copy_from_slice(&height.to_le_bytes());
    bytes
}

fn shape(width: f64, height: f64) -> Option<&'static str> {
    image_shape(width, height).map(|shape| shape.name)
}

#[test]
fn reads_the_size_of_png_jpeg_and_gif_files_from_their_first_bytes() {
    assert_eq!(
        image_info(&png(1200, 628)),
        Some(ImageInfo {
            format: ImageFormat::Png,
            width: 1200,
            height: 628
        })
    );
    assert_eq!(
        image_info(&jpeg(600, 600)),
        Some(ImageInfo {
            format: ImageFormat::Jpeg,
            width: 600,
            height: 600
        })
    );
    assert_eq!(
        image_info(&gif(512, 128)),
        Some(ImageInfo {
            format: ImageFormat::Gif,
            width: 512,
            height: 128
        })
    );
}

#[test]
fn takes_nothing_else_for_an_image() {
    assert_eq!(
        image_info(br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#),
        None
    );
    assert_eq!(image_info(b"%PDF-1.4"), None);
    assert_eq!(image_info(&[]), None);
    assert_eq!(image_info(&png(0, 628)), None);
    // A JPEG cut before its frame header has no size to read.
    assert_eq!(image_info(&jpeg(600, 600)[..10]), None);
}

#[test]
fn names_the_shapes_google_ads_uses_within_1_percent_of_their_ratio() {
    assert_eq!(shape(1200.0, 628.0), Some("landscape"));
    assert_eq!(shape(600.0, 314.0), Some("landscape"));
    assert_eq!(shape(300.0, 300.0), Some("square"));
    assert_eq!(shape(512.0, 128.0), Some("wide_logo"));
    assert_eq!(shape(960.0, 1200.0), Some("portrait"));
    assert_eq!(shape(800.0, 600.0), None);
    assert_eq!(shape(1200.0, 600.0), None);
}

#[test]
fn reads_files_as_the_typescript_does() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let files = fixture["files"].as_array().unwrap();
    assert_eq!(files.len(), 543);

    let mut images = 0;
    for (index, file) in files.iter().enumerate() {
        let bytes = STANDARD.decode(file[0].as_str().unwrap()).unwrap();
        let read = image_info(&bytes).map_or(Value::Null, |info| {
            json!({ "format": info.format.as_str(), "width": info.width, "height": info.height })
        });
        assert_eq!(read, file[1], "file {index}: {}", file[0]);
        images += usize::from(!read.is_null());
    }
    // Enough of the files are images for the comparison to say something of both.
    assert!(images > 100 && images < files.len() - 100, "{images}");
}

#[test]
fn names_shapes_as_the_typescript_does() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let sizes = fixture["shapes"].as_array().unwrap();
    assert_eq!(sizes.len(), 60);
    for size in sizes {
        let (width, height) = (size[0].as_f64().unwrap(), size[1].as_f64().unwrap());
        assert_eq!(json!(shape(width, height)), size[2], "{width}×{height}");
    }
    // A size Google Ads says nothing of is no shape, whatever it is not.
    assert_eq!(shape(f64::NAN, 628.0), None);
    assert_eq!(shape(0.0, 0.0), None);

    let known: Vec<Value> = IMAGE_SHAPES
        .iter()
        .map(|shape| {
            json!({
                "name": shape.name,
                "label": shape.label,
                "ratio": vine::js::number(shape.ratio),
                "minWidth": shape.min_width,
                "minHeight": shape.min_height,
            })
        })
        .collect();
    assert_eq!(json!(known), fixture["known"]);
    assert_eq!(
        json!(SQUARE_LOGO_MIN_PIXELS),
        fixture["squareLogoMinPixels"]
    );
}
