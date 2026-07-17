use image::GenericImageView;

/// Создать минимальное PNG (1x1 пиксель, красный) в памяти
fn minimal_png() -> Vec<u8> {
    use std::io::Cursor;

    // Создаём 1x1 RGB изображение
    let img = image::RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0]));
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

#[test]
fn test_process_image_basic() {
    let raw = minimal_png();
    let result = bh_rust::image::process_image(&raw, true, 70, false, 720).unwrap();

    assert!(!result.data.is_empty());
    assert_eq!(result.format, bh_rust::image::OutputFormat::Jpeg);
}

#[test]
fn test_process_image_bw() {
    let raw = minimal_png();
    let result = bh_rust::image::process_image(&raw, true, 80, true, 0).unwrap();

    assert!(!result.data.is_empty());
}

#[test]
fn test_process_image_quality() {
    // Создаём побольше для сравнения качества
    let img = image::RgbImage::from_pixel(100, 100, image::Rgb([128, 64, 32]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    let raw = buf.into_inner();

    let result_high = bh_rust::image::process_image(&raw, true, 95, false, 0).unwrap();
    let result_low = bh_rust::image::process_image(&raw, true, 10, false, 0).unwrap();

    // Низкое качество = меньше файл
    assert!(
        result_low.data.len() < result_high.data.len(),
        "low({}) should be < high({})",
        result_low.data.len(),
        result_high.data.len()
    );
}

#[test]
fn test_process_image_resize() {
    // Создаём большое изображение
    let img = image::RgbImage::from_pixel(1920, 1080, image::Rgb([100, 150, 200]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    let raw = buf.into_inner();

    // Ресайз до 720 по меньшей стороне → 1280x720
    let result = bh_rust::image::process_image(&raw, true, 80, false, 720).unwrap();
    let decoded = image::load_from_memory(&result.data).unwrap();
    let (w, h) = decoded.dimensions();

    assert_eq!(w, 1280, "Expected width 1280, got {}", w);
    assert_eq!(h, 720, "Expected height 720, got {}", h);
}

#[test]
fn test_no_resize_when_small() {
    let img = image::RgbImage::from_pixel(400, 300, image::Rgb([50, 50, 50]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    let raw = buf.into_inner();

    let result = bh_rust::image::process_image(&raw, true, 80, false, 720).unwrap();
    let decoded = image::load_from_memory(&result.data).unwrap();
    let (w, h) = decoded.dimensions();

    assert_eq!(w, 400);
    assert_eq!(h, 300);
}

#[test]
fn test_prefer_original_if_smaller_config_default() {
    // Проверка, что по умолчанию prefer_original_if_smaller = true
    let cfg = bh_rust::config::ImageConfig::default();
    assert!(
        cfg.prefer_original_if_smaller,
        "prefer_original_if_smaller should be true by default"
    );
}
