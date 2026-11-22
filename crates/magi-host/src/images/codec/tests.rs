use super::*;
use image::{GenericImageView, Rgb, RgbImage, Rgba, RgbaImage};
use serde_json::json;

fn image(pixels: DynamicImage, format: ImageFormat, media_type: &str) -> Image {
    let mut bytes = Cursor::new(Vec::new());
    pixels.write_to(&mut bytes, format).expect("encode fixture");
    Image {
        data: STANDARD.encode(bytes.into_inner()),
        media_type: media_type.into(),
    }
}

fn solid(width: u32, height: u32) -> Image {
    image(
        DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([220, 30, 50]))),
        ImageFormat::Png,
        "image/png",
    )
}

fn profile(value: serde_json::Value) -> ImageResize {
    serde_json::from_value(value).expect("valid profile")
}

fn pixels(image: &Image) -> DynamicImage {
    image::load_from_memory(&STANDARD.decode(&image.data).expect("base64")).expect("decoded result")
}

#[test]
fn fitting_images_keep_exact_bytes_and_never_upscale() {
    let original = solid(40, 20);
    for profile in [
        ImageResize::default(),
        profile(json!({"max_width":400,"max_height":400,"jpeg_quality":1})),
        profile(json!({"max_bytes":original.data.len(),"max_width":40,"max_height":20})),
    ] {
        let result = convert(original.clone(), &profile, Mode::Normalize).expect("fits");
        assert_eq!(result, original);
        assert_eq!(pixels(&result).dimensions(), (40, 20));
    }
}

#[test]
fn real_pixels_are_resized_with_aspect_ratio_and_colors() {
    let source = RgbImage::from_fn(400, 200, |x, _| {
        if x < 200 {
            Rgb([220, 20, 20])
        } else {
            Rgb([20, 20, 220])
        }
    });
    let original = image(source.into(), ImageFormat::Png, "image/png");
    let result = convert(
        original,
        &profile(json!({"max_width":20,"max_height":15})),
        Mode::Normalize,
    )
    .expect("resize");
    let result = pixels(&result);
    assert_eq!(result.dimensions(), (20, 10));
    assert!(result.get_pixel(2, 4).0[0] > 190);
    assert!(result.get_pixel(17, 4).0[2] > 190);
}

#[test]
fn transparency_survives_resizing_as_png() {
    let original = image(
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(100, 50, Rgba([80, 40, 20, 64]))),
        ImageFormat::Png,
        "image/png",
    );
    let result = convert(original, &profile(json!({"max_width":10})), Mode::Normalize)
        .expect("resize alpha");
    assert_eq!(result.media_type, "image/png");
    assert_eq!(pixels(&result).dimensions(), (10, 5));
    assert_eq!(pixels(&result).get_pixel(5, 2).0[3], 64);
}

#[test]
fn base64_byte_limit_reduces_real_noisy_pixels() {
    let source = RgbImage::from_fn(128, 128, |x, y| {
        let n = (x + y * 128).wrapping_mul(2654435761);
        Rgb([(n >> 24) as u8, (n >> 16) as u8, n as u8])
    });
    let original = image(source.into(), ImageFormat::Png, "image/png");
    assert!(original.data.len() > 1000);
    let result = convert(
        original,
        &profile(json!({"max_bytes":1000})),
        Mode::Normalize,
    )
    .expect("fit base64 budget");
    assert!(result.data.len() <= 1000);
    let result = pixels(&result);
    assert!(result.width() < 128 && result.height() < 128);
}

#[test]
fn small_lossless_candidate_is_not_refused_for_jpeg_overhead() {
    let original = solid(100, 50);
    let result = convert(
        original,
        &profile(json!({"max_width":10,"max_bytes":200})),
        Mode::Normalize,
    )
    .expect("small png fits");
    assert_eq!(result.media_type, "image/png");
    assert_eq!(pixels(&result).dimensions(), (10, 5));
    assert!(result.data.len() <= 200);
}

#[test]
fn impossible_profiles_refuse_without_partial_output() {
    assert!(matches!(
        convert(
            solid(10, 10),
            &profile(json!({"max_bytes":1})),
            Mode::Normalize
        ),
        Err(Failure::CannotFit)
    ));
    let good = solid(5, 5);
    let bad = Image {
        data: "AAAA".into(),
        media_type: "image/png".into(),
    };
    assert!(
        run(Request {
            version: 1,
            mode: Mode::Normalize,
            profile: ImageResize::default(),
            images: vec![good, bad]
        })
        .is_err()
    );
}

#[test]
fn historical_images_are_validated_never_recompressed() {
    let original = solid(100, 50);
    let held = original.clone();
    let limits = profile(json!({"max_width":10}));
    assert!(matches!(
        convert(original.clone(), &limits, Mode::Validate),
        Err(Failure::History)
    ));
    assert_eq!(original, held);
    let normalized = convert(original, &limits, Mode::Normalize).expect("new image");
    assert_eq!(
        convert(normalized.clone(), &limits, Mode::Validate).expect("frozen image"),
        normalized
    );
}

#[test]
fn format_base64_and_pixel_errors_are_rejected() {
    let mut wrong = solid(10, 10);
    wrong.media_type = "image/jpeg".into();
    assert!(matches!(
        convert(wrong, &ImageResize::default(), Mode::Normalize),
        Err(Failure::Unsupported)
    ));
    for data in ["!!!!", "iVBORw0KGgo="] {
        assert!(
            convert(
                Image {
                    data: data.into(),
                    media_type: "image/png".into()
                },
                &ImageResize::default(),
                Mode::Normalize
            )
            .is_err()
        );
    }
}

#[test]
fn strict_edges_and_decoded_pixel_budgets_refuse_small_compressed_sources() {
    for (width, height) in [(8193, 1), (3000, 3000)] {
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([0, 0, 0])))
            .write_with_encoder(image::codecs::png::PngEncoder::new_with_quality(
                &mut bytes,
                image::codecs::png::CompressionType::Best,
                image::codecs::png::FilterType::Adaptive,
            ))
            .expect("compressed oversized raster");
        let original = Image {
            data: STANDARD.encode(bytes),
            media_type: "image/png".into(),
        };
        assert!(STANDARD.decode(&original.data).expect("base64").len() < INLINE_BYTES);
        assert!(matches!(
            convert(original, &ImageResize::default(), Mode::Normalize),
            Err(Failure::Resource)
        ));
    }
}

#[test]
fn all_supported_static_formats_round_trip_without_reencoding() {
    for (format, mime) in [
        (ImageFormat::Png, "image/png"),
        (ImageFormat::Jpeg, "image/jpeg"),
        (ImageFormat::Gif, "image/gif"),
        (ImageFormat::WebP, "image/webp"),
        (ImageFormat::Bmp, "image/bmp"),
    ] {
        let original = image(
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(16, 8, Rgba([220, 30, 50, 255])))
                .to_rgb8()
                .into(),
            format,
            mime,
        );
        assert_eq!(
            convert(original.clone(), &ImageResize::default(), Mode::Normalize).expect(mime),
            original
        );
    }
}

#[test]
fn orientation_is_applied_before_dimension_limits() {
    let mut original = image(
        RgbImage::from_pixel(40, 20, Rgb([200, 30, 50])).into(),
        ImageFormat::Jpeg,
        "image/jpeg",
    );
    let jpeg = STANDARD.decode(&original.data).expect("jpeg");
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
    bytes.extend_from_slice(
        &u16::try_from(exif.len() + 2)
            .expect("APP1 size")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(exif);
    bytes.extend_from_slice(&jpeg[2..]);
    original.data = STANDARD.encode(bytes);
    let result = convert(
        original,
        &profile(json!({"max_width":10,"max_height":10})),
        Mode::Normalize,
    )
    .expect("oriented image");
    assert_eq!(pixels(&result).dimensions(), (5, 10));
}

#[test]
fn animated_gif_is_unchanged_when_fitting_and_resizes_its_first_frame() {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        for color in [Rgba([220, 20, 20, 255]), Rgba([20, 20, 220, 255])] {
            encoder
                .encode_frame(image::Frame::new(RgbaImage::from_pixel(20, 10, color)))
                .expect("frame");
        }
    }
    let original = Image {
        data: STANDARD.encode(bytes),
        media_type: "image/gif".into(),
    };
    assert_eq!(
        convert(original.clone(), &ImageResize::default(), Mode::Normalize).expect("fits"),
        original
    );
    let resized = convert(original, &profile(json!({"max_width":10})), Mode::Normalize)
        .expect("resize frame");
    assert_eq!(pixels(&resized).dimensions(), (10, 5));
    assert!(pixels(&resized).get_pixel(4, 2).0[0] > 190);
}

#[test]
fn worker_protocol_version_and_attachment_count_are_strict() {
    for (version, images) in [(2, vec![]), (1, vec![solid(1, 1); 5])] {
        assert!(matches!(
            run(Request {
                version,
                mode: Mode::Normalize,
                profile: ImageResize::default(),
                images
            }),
            Err(Failure::Invalid)
        ));
    }
}
