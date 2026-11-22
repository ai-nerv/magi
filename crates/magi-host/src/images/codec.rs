use super::{Failure, INLINE_BYTES, Mode, Request};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use magi_model::{Image, ImageResize};
use std::io::{Cursor, Write};

const EDGE: u32 = 8192;
const PIXELS: u64 = 8 * 1024 * 1024;

pub(super) fn run(request: Request) -> Result<Vec<Image>, Failure> {
    if request.version != 1 {
        return Err(Failure::Invalid);
    }
    magi_model::validate_images(&request.images).map_err(|_| Failure::Invalid)?;
    request
        .images
        .into_iter()
        .map(|image| convert(image, &request.profile, request.mode))
        .collect()
}

fn convert(image: Image, profile: &ImageResize, mode: Mode) -> Result<Image, Failure> {
    let bytes = STANDARD.decode(&image.data).map_err(|_| Failure::Invalid)?;
    if bytes.len() > INLINE_BYTES {
        return Err(Failure::Resource);
    }
    let format = image::guess_format(&bytes).map_err(|_| Failure::Unsupported)?;
    let expected = match image.media_type.as_str() {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        "image/bmp" => ImageFormat::Bmp,
        _ => return Err(Failure::Unsupported),
    };
    if format != expected {
        return Err(Failure::Unsupported);
    }
    let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(EDGE);
    limits.max_image_height = Some(EDGE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(decode_error)?;
    let (width, height) = decoder.dimensions();
    if width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > PIXELS
        || decoder.total_bytes() > 64 * 1024 * 1024
    {
        return Err(Failure::Resource);
    }
    let orientation = decoder.orientation().map_err(|_| Failure::Invalid)?;
    let mut pixels = DynamicImage::from_decoder(decoder).map_err(decode_error)?;
    pixels.apply_orientation(orientation);
    let width = dimension(profile.max_width, pixels.width());
    let height = dimension(profile.max_height, pixels.height());
    let encoded_cap = profile
        .max_bytes
        .map_or(u64::MAX, std::num::NonZeroU64::get);
    if pixels.width() <= width
        && pixels.height() <= height
        && image.data.len() as u64 <= encoded_cap
    {
        return Ok(image);
    }
    if matches!(mode, Mode::Validate) {
        return Err(Failure::History);
    }
    let raw_cap = INLINE_BYTES.min(usize::try_from(encoded_cap / 4 * 3).unwrap_or(usize::MAX));
    if raw_cap == 0 {
        return Err(Failure::CannotFit);
    }
    let alpha = pixels.color().has_alpha();
    let mut resized = pixels.thumbnail(width, height);
    for _ in 0..32 {
        if let Some((bytes, media_type)) = encode(
            &resized,
            alpha,
            profile.jpeg_quality.map_or(80, u8::from),
            raw_cap,
        ) {
            let data = STANDARD.encode(bytes);
            if data.len() as u64 <= encoded_cap {
                return Ok(Image {
                    data,
                    media_type: media_type.into(),
                });
            }
        }
        let (width, height) = (resized.width(), resized.height());
        if width == 1 && height == 1 {
            break;
        }
        resized = pixels.thumbnail(
            width.saturating_mul(3).checked_div(4).unwrap_or(1).max(1),
            height.saturating_mul(3).checked_div(4).unwrap_or(1).max(1),
        );
    }
    Err(Failure::CannotFit)
}

fn decode_error(error: image::ImageError) -> Failure {
    match error {
        image::ImageError::Limits(_) => Failure::Resource,
        _ => Failure::Invalid,
    }
}

fn dimension(maximum: Option<std::num::NonZeroU64>, actual: u32) -> u32 {
    maximum.map_or(actual, |value| {
        u32::try_from(value.get()).unwrap_or(u32::MAX).min(actual)
    })
}

struct Bounded {
    bytes: Vec<u8>,
    maximum: usize,
}

impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encoded(
    pixels: &DynamicImage,
    png: bool,
    quality: u8,
    maximum: usize,
) -> image::ImageResult<Vec<u8>> {
    let mut out = Bounded {
        bytes: Vec::new(),
        maximum,
    };
    if png {
        pixels.write_with_encoder(image::codecs::png::PngEncoder::new(&mut out))?;
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .encode_image(&pixels.to_rgb8())?;
    }
    Ok(out.bytes)
}

fn encode(
    pixels: &DynamicImage,
    alpha: bool,
    quality: u8,
    maximum: usize,
) -> Option<(Vec<u8>, &'static str)> {
    let png = encoded(pixels, true, quality, maximum).ok();
    if alpha {
        return png.map(|bytes| (bytes, "image/png"));
    }
    let jpeg = encoded(pixels, false, quality, maximum).ok();
    match (png, jpeg) {
        (Some(png), Some(jpeg)) if jpeg.len() < png.len() => return Some((jpeg, "image/jpeg")),
        (Some(png), _) => return Some((png, "image/png")),
        (_, Some(jpeg)) => return Some((jpeg, "image/jpeg")),
        _ => {}
    }
    for lower in [70, 55, 40].into_iter().filter(|lower| *lower < quality) {
        if let Ok(bytes) = encoded(pixels, false, lower, maximum) {
            return Some((bytes, "image/jpeg"));
        }
    }
    None
}

#[cfg(test)]
mod tests;
