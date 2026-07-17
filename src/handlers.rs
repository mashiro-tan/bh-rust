use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use base64::Engine;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::{config::AppConfig, image::ProcessedImage};

/// Глобальное состояние, передаваемое в хендлеры.
#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    /// Готовый HTTP-клиент (с прокси, если настроен)
    pub client: reqwest::Client,
    /// Таймштамп UTC старта сервера (секунды с эпохи)
    pub started_on: u64,
    /// Счётчики статистики
    pub stats: Arc<Stats>,
}

/// Атомарные счётчики для /stats.
pub struct Stats {
    pub source_total: AtomicU64,      // суммарный вес оригиналов
    pub compressed_total: AtomicU64,   // суммарный вес сжатых (отданных)
    pub dropped_total: AtomicU64,      // суммарный вес сжатых, которые были отброшены (больше оригинала)
    pub dest_total: AtomicU64,         // суммарный вес отданных (compressed + original fallback)
}

impl Stats {
    pub fn new() -> Self {
        Self {
            source_total: AtomicU64::new(0),
            compressed_total: AtomicU64::new(0),
            dropped_total: AtomicU64::new(0),
            dest_total: AtomicU64::new(0),
        }
    }
}

/// Запрос от Komikku Bandwidth Hero:
/// `GET /?url=ORIGINAL&jpg=0&l=80&bw=0&resize=720&headers=BASE64_JSON`
#[derive(Debug)]
pub struct BhRequest {
    /// URL исходного изображения
    pub url: String,
    /// Конвертировать в JPEG: "1" = да, "0" = нет, absent = из конфига
    pub jpg: Option<bool>,
    /// Качество (1–100), absent = из конфига
    pub quality: Option<u8>,
    /// Черно-белый: "1" = да, "0" = нет, absent = из конфига
    pub bw: Option<bool>,
    /// Ресайз по короткой стороне (0 = не ресайзить), absent = из конфига
    pub resize: Option<u32>,
    /// Заголовки для проксирования (Cookie, User-Agent, Referer и т.д.)
    pub headers: Option<HashMap<String, String>>,
}

impl BhRequest {
    /// Разобрать из query params (`url`, `jpg`, `l`, `bw`, `headers`).
    pub fn from_query(params: &axum::extract::Query<std::collections::HashMap<String, String>>) -> Option<Self> {
        let map = &params.0;
        let url = map.get("url")?.clone();

        let jpg = map.get("jpg").and_then(|v| match v.as_str() {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        });

        let quality = map.get("l").and_then(|v| v.parse::<u8>().ok()).filter(|q| *q >= 1 && *q <= 100);

        let bw = map.get("bw").and_then(|v| match v.as_str() {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        });

        let resize = map.get("resize").and_then(|v| v.parse::<u32>().ok());

        // Decode base64-encoded JSON headers: {"User-Agent":"...","Cookie":"..."}
        let headers = map.get("headers").and_then(|h| {
            // base64 URL-safe decode (with or without padding)
            let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(h).ok()?;
            serde_json::from_slice::<HashMap<String, String>>(&decoded).ok()
        });

        Some(Self { url, jpg, quality, bw, resize, headers })
    }
}

/// GET / — основной обработчик Bandwidth Hero.
pub async fn handle_compress(
    State(state): State<AppState>,
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let req = match BhRequest::from_query(&query) {
        Some(r) => r,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Missing required parameter: url",
            )
                .into_response();
        }
    };

    info!(
        url = %req.url,
        jpg = ?req.jpg,
        quality = ?req.quality,
        bw = ?req.bw,
        resize = ?req.resize,
        has_headers = req.headers.is_some(),
        "Processing request"
    );

    // Разрешить параметры из конфига
    let cfg = &state.config.image;
    let to_jpeg = req.jpg.unwrap_or(cfg.default_jpeg);
    let quality = req.quality.unwrap_or(cfg.quality);
    let to_bw = req.bw.unwrap_or(cfg.default_bw);
    let resize_short = req.resize.unwrap_or(cfg.resize_short_side);

    // Валидация quality
    if quality < 1 || quality > 100 {
        return (
            StatusCode::BAD_REQUEST,
            "Quality must be between 1 and 100",
        )
            .into_response();
    }

    // Скачать исходное изображение
    let (bytes, content_type) = match download_image(&state.client, &req.url, req.headers.as_ref()).await {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Failed to download image");
            return (
                StatusCode::BAD_GATEWAY,
                format!("Failed to download image: {}", e),
            )
                .into_response();
        }
    };

    // Проверить лимит размера
    if cfg.max_download_bytes > 0 && (bytes.len() as u64) > cfg.max_download_bytes {
        warn!(
            size = bytes.len(),
            max = cfg.max_download_bytes,
            "Image exceeds max download size"
        );
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "Image exceeds maximum allowed size",
        )
            .into_response();
    }

    // Обработать изображение
    let result = match crate::image::process_image(&bytes, to_jpeg, quality, to_bw, resize_short) {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Failed to process image");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to process image: {}", e),
            )
                .into_response();
        }
    };

    // Если сжатая копия больше оригинала — вернуть оригинал (если включено в конфиге)
    let original_size = bytes.len() as u64;
    let compressed_size = result.data.len() as u64;
    let return_original = cfg.prefer_original_if_smaller && compressed_size >= original_size;

    let (response_data, response_content_type) = if return_original {
        warn!(
            original_size,
            compressed_size,
            "Compressed image is larger than original — returning original"
        );
        (bytes, content_type)
    } else {
        let content_type = resolve_content_type(&result, &content_type);
        (result.data, content_type)
    };

    // Обновить статистику
    let dest_size = response_data.len() as u64;
    state.stats.source_total.fetch_add(original_size, Ordering::Relaxed);
    if return_original {
        state.stats.dropped_total.fetch_add(compressed_size, Ordering::Relaxed);
    } else {
        state.stats.compressed_total.fetch_add(compressed_size, Ordering::Relaxed);
    }
    state.stats.dest_total.fetch_add(dest_size, Ordering::Relaxed);

    let size_ratio = original_size as f64 / dest_size as f64;

    info!(
        original_size,
        result_size = dest_size,
        ratio = size_ratio,
        "Completed"
    );

    (
        StatusCode::OK,
        [
            ("Content-Type", response_content_type.as_str()),
            ("Content-Length", response_data.len().to_string().as_str()),
        ],
        response_data,
    )
        .into_response()
}

/// GET /health — проверка работоспособности.
pub async fn handle_health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "bh-rust",
    }))
}

/// GET /stats — статистика работы сервера.
pub async fn handle_stats(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "startedOn": state.started_on,
        "sourceTotal": state.stats.source_total.load(Ordering::Relaxed),
        "compressedTotal": state.stats.compressed_total.load(Ordering::Relaxed),
        "droppedTotal": state.stats.dropped_total.load(Ordering::Relaxed),
        "destTotal": state.stats.dest_total.load(Ordering::Relaxed),
    }))
}

// ——— Helpers ———

/// Скачать изображение по URL, опционально добавляя заголовки.
async fn download_image(
    client: &reqwest::Client,
    url: &str,
    headers: Option<&HashMap<String, String>>,
) -> anyhow::Result<(Vec<u8>, String)> {
    // Валидация URL-схемы (защита в глубину: только http/https)
    crate::ssrf::validate_url_scheme(url)?;

    // Hop-by-hop заголовки, которые не нужно проксировать
    let hop_by_hop = [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "host",
    ];

    let mut request = client.get(url);

    if let Some(headers) = headers {
        for (key, value) in headers {
            // Пропускаем hop-by-hop заголовки (case-insensitive)
            if hop_by_hop.contains(&key.to_lowercase().as_str()) {
                continue;
            }
            request = request.header(key.as_str(), value.as_str());
        }
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        anyhow::bail!("Source returned status {}", response.status());
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| {
            // Извлекаем только MIME-тип, отбрасывая параметры (charset, boundary и т.д.).
            // Для image/* параметры вроде charset не определены в RFC 2046.
            ct.split(';').next().unwrap_or(ct).trim().to_string()
        })
        .unwrap_or_else(|| "application/octet-stream".to_string());

    // Проверяем, что сервер вернул изображение, а не HTML-страницу или другой контент.
    if content_type.as_str() != "application/octet-stream" && !content_type.starts_with("image/") {
        anyhow::bail!(
            "Source returned non-image content type: '{}'",
            content_type
        );
    }

    let bytes = response.bytes().await?.to_vec();
    Ok((bytes, content_type))
}

/// Определить Content-Type для результата.
fn resolve_content_type(result: &ProcessedImage, _original: &str) -> String {
    match result.format {
        crate::image::OutputFormat::Jpeg => "image/jpeg".to_string(),
        crate::image::OutputFormat::WebP => "image/webp".to_string(),
        crate::image::OutputFormat::Png => "image/png".to_string(),
    }
}
