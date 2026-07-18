use std::io::Cursor;

use fast_image_resize::{images::Image as FIrImage, PixelType, Resizer};
use image::{
    codecs::jpeg::JpegEncoder, ExtendedColorType, GenericImageView, GrayImage, RgbImage,
};
use tracing::info;
use webp::Encoder;

/// Формат выходного изображения.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    Jpeg,
    /// Lossy WebP via the `webp` crate (wraps libwebp)
    WebP,
    #[allow(dead_code)]
    Png,
}

/// Результат обработки.
pub struct ProcessedImage {
    pub data: Vec<u8>,
    pub format: OutputFormat,
}

/// Обработать изображение: ресайз → B&W → сжатие/конвертация.
pub fn process_image(
    raw: &[u8],
    to_jpeg: bool,
    quality: u8,
    to_bw: bool,
    resize_short_side: u32,
) -> anyhow::Result<ProcessedImage> {
    // 1. Декодировать
    let img = image::load_from_memory(raw)
        .map_err(|e| anyhow::anyhow!("Failed to decode image: {}", e))?;

    let (orig_w, orig_h) = img.dimensions();
    let short_side = orig_w.min(orig_h);

    // Determine pixel format: grayscale or RGB (strip alpha — JPEG doesn't support it)
    let is_grayscale =
        matches!(img, image::DynamicImage::ImageLuma8(_) | image::DynamicImage::ImageLumaA8(_));
    let pixel_type = if is_grayscale {
        PixelType::U8
    } else {
        PixelType::U8x3
    };

    // 2. Ресайз через fast_image_resize (SIMD-оптимизированный Lanczos3)
    let img = if resize_short_side > 0 && short_side > resize_short_side {
        let scale = resize_short_side as f64 / short_side as f64;
        let new_w = (orig_w as f64 * scale).round() as u32;
        let new_h = (orig_h as f64 * scale).round() as u32;
        info!(
            original = "{}x{}",
            orig_w, orig_h,
            resized = "{}x{}",
            new_w, new_h,
            filter = "Lanczos3 (fast_image_resize SIMD)",
            "Resizing"
        );

        // fast_image_resize::Resizer uses Lanczos3 by default (ResizeAlg::Convolution(Lanczos3))
        let mut resizer = Resizer::new();

        let mut dst = FIrImage::new(new_w, new_h, pixel_type);
        // DynamicImage implements IntoImageView via the "image" feature
        resizer.resize(&img, &mut dst, None).map_err(|e| {
            anyhow::anyhow!("fast_image_resize error: {}", e)
        })?;

        // Convert back to image::ImageBuffer
        let buf = dst.into_vec();
        if is_grayscale {
            let gray: GrayImage =
                GrayImage::from_raw(new_w, new_h, buf).ok_or_else(|| {
                    anyhow::anyhow!("Failed to reconstruct grayscale image")
                })?;
            image::DynamicImage::ImageLuma8(gray)
        } else {
            let rgb: RgbImage =
                RgbImage::from_raw(new_w, new_h, buf).ok_or_else(|| {
                    anyhow::anyhow!("Failed to reconstruct RGB image")
                })?;
            image::DynamicImage::ImageRgb8(rgb)
        }
    } else {
        img
    };

    // 3. Черно-белый
    let img = if to_bw {
        info!("Converting to grayscale");
        img.grayscale().into()
    } else {
        img
    };

    // 4. Конвертировать в RGB8 для энкодинга
    let rgb = img.into_rgb8();

    // 5. Энкодить
    let (data, output_format) = if to_jpeg {
        let mut cursor = Cursor::new(Vec::new());
        JpegEncoder::new_with_quality(&mut cursor, quality)
            .encode(&rgb, rgb.width(), rgb.height(), ExtendedColorType::Rgb8)
            .map_err(|e| anyhow::anyhow!("JPEG encoding error: {}", e))?;
        (cursor.into_inner(), OutputFormat::Jpeg)
    } else {
        // Lossy WebP via libwebp
        let bytes = rgb.as_raw();
        let webp_data = Encoder::from_rgb(&bytes, rgb.width(), rgb.height())
            .encode(quality as f32);
        ((*webp_data).to_vec(), OutputFormat::WebP)
    };

    Ok(ProcessedImage { data, format: output_format })
}
