use std::io::Cursor;

use fast_image_resize::{images::Image as FIrImage, PixelType, Resizer};
use image::{
    codecs::jpeg::JpegEncoder, ExtendedColorType, GenericImageView, GrayImage, RgbImage,
};
use tracing::{info, warn};
use webp::Encoder;

/// Ошибка обработки изображения.
///
/// `InvalidDimensions` означает, что размеры изображения выходят за допустимые
/// пределы — в этом случае хендлер должен вернуть оригинал без изменений.
#[derive(Debug)]
pub enum ProcessImageError {
    /// Размеры вне диапазона [1, max_dimension] — вернуть оригинал
    InvalidDimensions(String),
    /// Байты не являются валидным изображением — вернуть оригинал
    DecodeFailed(String),
    /// Ошибка ресайза, энкодинга — 500
    Processing(anyhow::Error),
}

impl std::fmt::Display for ProcessImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessImageError::InvalidDimensions(msg) => write!(f, "{}", msg),
            ProcessImageError::DecodeFailed(msg) => write!(f, "{}", msg),
            ProcessImageError::Processing(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for ProcessImageError {}

impl From<anyhow::Error> for ProcessImageError {
    fn from(e: anyhow::Error) -> Self {
        ProcessImageError::Processing(e)
    }
}

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

/// Обработать изображение: валидация → ресайз → проверка целевых размеров → B&W → сжатие.
///
/// `max_dimension` — максимум по любой стороне исходного изображения (0 = без лимита).
/// `max_target_dimension` — максимум по длинной стороне целевого изображения (0 = без лимита).
/// Эффективный лимит — min(max_target_dimension, хард-лимит формата).
/// Хард-лимит: 16383 для WebP, 65535 для JPEG.
/// Если целевая длинная сторона превышает лимит — пересчитываем пропорционально.
/// Если после пересчёта короткая сторона < 1 — возвращаем оригинал.
pub fn process_image(
    raw: &[u8],
    to_jpeg: bool,
    quality: u8,
    to_bw: bool,
    resize_short_side: u32,
    max_dimension: u32,
    max_target_dimension: u32,
) -> Result<ProcessedImage, ProcessImageError> {
    // 1. Декодировать
    let img = image::load_from_memory(raw)
        .map_err(|e| ProcessImageError::DecodeFailed(format!("Failed to decode image: {}", e)))?;

    let (orig_w, orig_h) = img.dimensions();

    // 2. Валидация размеров
    if orig_w < 1 || orig_h < 1 {
        return Err(ProcessImageError::InvalidDimensions(format!(
            "Image dimensions {}x{} are invalid (minimum 1x1)",
            orig_w, orig_h
        )));
    }
    if max_dimension > 0 && (orig_w > max_dimension || orig_h > max_dimension) {
        return Err(ProcessImageError::InvalidDimensions(format!(
            "Image dimensions {}x{} exceed maximum {}x{}",
            orig_w, orig_h, max_dimension, max_dimension
        )));
    }

    let short_side = orig_w.min(orig_h);

    // 3. Вычислить целевые размеры (с учётом resize_short_side)
    let mut target_w = orig_w;
    let mut target_h = orig_h;

    if resize_short_side > 0 && short_side > resize_short_side {
        let scale = resize_short_side as f64 / short_side as f64;
        target_w = (orig_w as f64 * scale).round() as u32;
        target_h = (orig_h as f64 * scale).round() as u32;
    }

    // 4. Проверка целевых размеров по максимуму
    // Хард-лимит по длинной стороне: 16383 для WebP, 65535 для JPEG
    let hard_limit = if to_jpeg { 65535 } else { 16383 };
    let effective_max = if max_target_dimension > 0 {
        max_target_dimension.min(hard_limit)
    } else {
        hard_limit
    };

    let target_long = target_w.max(target_h);
    if target_long > effective_max {
        let scale = effective_max as f64 / target_long as f64;
        target_w = (target_w as f64 * scale).round() as u32;
        target_h = (target_h as f64 * scale).round() as u32;
        info!(
            hard_limit = hard_limit,
            max_target = max_target_dimension,
            effective_max = effective_max,
            "Target dimensions clamped by format limit"
        );
    }

    // 5. Проверка по минимуму: короткая сторона >= 1
    let target_short = target_w.min(target_h);
    if target_short < 1 {
        return Err(ProcessImageError::InvalidDimensions(format!(
            "Target dimensions {}x{} have short side < 1px after clamping",
            target_w, target_h
        )));
    }

    // 5. B&W — преобразовать в grayscale (до ресайза и определения типа пикселей)
    let img = if to_bw {
        info!("Converting to grayscale");
        img.grayscale().into()
    } else {
        img
    };

    // 6. Определить реальный тип пикселей после B&W-конвертации
    //    Issue #8: логировать потерю alpha-канала
    let is_grayscale = matches!(img, image::DynamicImage::ImageLuma8(_));
    let has_alpha = matches!(
        img,
        image::DynamicImage::ImageRgba8(_) | image::DynamicImage::ImageLumaA8(_)
    );
    if has_alpha {
        warn!("Image has alpha channel — transparency will be lost in output");
    }

    // Flatten RGBA → RGB (если нужно). Grayscale оставляем как есть.
    let img = if is_grayscale {
        img
    } else {
        image::DynamicImage::ImageRgb8(img.into_rgb8())
    };

    let pixel_type = if is_grayscale {
        PixelType::U8
    } else {
        PixelType::U8x3
    };

    // 7. Ресайз через fast_image_resize (SIMD-оптимизированный Lanczos3)
    let img = if target_w != orig_w || target_h != orig_h {
        info!(
            original = "{}x{}",
            orig_w, orig_h,
            resized = "{}x{}",
            target_w, target_h,
            filter = "Lanczos3 (fast_image_resize SIMD)",
            "Resizing"
        );

        let mut resizer = Resizer::new();
        let mut dst = FIrImage::new(target_w, target_h, pixel_type);
        resizer.resize(&img, &mut dst, None).map_err(|e| {
            anyhow::anyhow!("fast_image_resize error: {}", e)
        })?;

        let buf = dst.into_vec();
        if is_grayscale {
            let gray: GrayImage =
                GrayImage::from_raw(target_w, target_h, buf).ok_or_else(|| {
                    anyhow::anyhow!("Failed to reconstruct grayscale image")
                })?;
            image::DynamicImage::ImageLuma8(gray)
        } else {
            let rgb: RgbImage =
                RgbImage::from_raw(target_w, target_h, buf).ok_or_else(|| {
                    anyhow::anyhow!("Failed to reconstruct RGB image")
                })?;
            image::DynamicImage::ImageRgb8(rgb)
        }
    } else {
        img
    };

    // 8. Энкодинг
    //    Issue #7: grayscale кодируем напрямую (1 канал), без бессмысленной конвертации в RGB
    let (data, output_format) = if is_grayscale {
        let gray = match &img {
            image::DynamicImage::ImageLuma8(g) => g,
            _ => unreachable!("is_grayscale is true but image is not Luma8"),
        };
        if to_jpeg {
            let mut cursor = Cursor::new(Vec::new());
            JpegEncoder::new_with_quality(&mut cursor, quality)
                .encode(gray, gray.width(), gray.height(), ExtendedColorType::L8)
                .map_err(|e| anyhow::anyhow!("JPEG encoding error: {}", e))?;
            (cursor.into_inner(), OutputFormat::Jpeg)
        } else {
            // WebP: webp crate не поддерживает grayscale напрямую → конвертируем в RGB
            let rgb = img.to_rgb8();
            let bytes = rgb.as_raw();
            let webp_data = Encoder::from_rgb(&bytes, rgb.width(), rgb.height())
                .encode(quality as f32);
            ((*webp_data).to_vec(), OutputFormat::WebP)
        }
    } else {
        let rgb = match &img {
            image::DynamicImage::ImageRgb8(r) => r,
            _ => unreachable!("is_grayscale is false but image is not Rgb8"),
        };
        if to_jpeg {
            let mut cursor = Cursor::new(Vec::new());
            JpegEncoder::new_with_quality(&mut cursor, quality)
                .encode(rgb, rgb.width(), rgb.height(), ExtendedColorType::Rgb8)
                .map_err(|e| anyhow::anyhow!("JPEG encoding error: {}", e))?;
            (cursor.into_inner(), OutputFormat::Jpeg)
        } else {
            let bytes = rgb.as_raw();
            let webp_data = Encoder::from_rgb(&bytes, rgb.width(), rgb.height())
                .encode(quality as f32);
            ((*webp_data).to_vec(), OutputFormat::WebP)
        }
    };

    Ok(ProcessedImage { data, format: output_format })
}
