use serde::Deserialize;

/// Главный конфиг сервера.
///
/// Читается из `config.toml` в текущей директории или из пути,
/// переданного через аргумент `--config`.
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    /// Серверная часть
    #[serde(default = "ServerConfig::default")]
    pub server: ServerConfig,

    /// Настройки обработки изображений по умолчанию
    #[serde(default = "ImageConfig::default")]
    pub image: ImageConfig,

    /// SOCKS5 прокси (опционально)
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Адрес для прослушивания
    #[serde(default = "default_host")]
    pub host: String,

    /// Порт
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageConfig {
    /// Ресайзить изображения, если меньшая сторона превышает это значение (в пикселях).
    /// 0 — не ресайзить.
    #[serde(default = "default_resize_short_side")]
    pub resize_short_side: u32,

    /// Качество сжатия по умолчанию (1–100), если не указано в запросе
    #[serde(default = "default_quality")]
    pub quality: u8,

    /// Конвертировать в JPEG по умолчанию (если не указано в запросе)
    #[serde(default)]
    pub default_jpeg: bool,

    /// Черно-белый режим по умолчанию (если не указано в запросе)
    #[serde(default)]
    pub default_bw: bool,

    /// Максимальный размер исходного изображения в байтах (0 = без лимита)
    #[serde(default)]
    pub max_download_bytes: u64,

    /// Если сжатая копия больше оригинала — вернуть оригинал без изменений.
    #[serde(default = "default_prefer_original_if_smaller")]
    pub prefer_original_if_smaller: bool,

    /// Максимально допустимый размер по любой из сторон (px). 0 = без лимита.
    /// Изображения вне диапазона [1px, max_image_dimension] передаются как есть.
    #[serde(default = "default_max_image_dimension")]
    pub max_image_dimension: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    /// Адрес SOCKS5 прокси (например, "127.0.0.1:1080")
    pub address: String,

    /// Логин (опционально)
    #[serde(default)]
    pub username: Option<String>,

    /// Пароль (опционально)
    #[serde(default)]
    pub password: Option<String>,
}

// ——— Defaults ———

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            resize_short_side: default_resize_short_side(),
            quality: default_quality(),
            default_jpeg: false,
            default_bw: false,
            max_download_bytes: 0,
            prefer_original_if_smaller: default_prefer_original_if_smaller(),
            max_image_dimension: default_max_image_dimension(),
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_resize_short_side() -> u32 {
    720
}

fn default_quality() -> u8 {
    80
}

fn default_prefer_original_if_smaller() -> bool {
    true
}

fn default_max_image_dimension() -> u32 {
    65535
}

// ——— Loading ———

/// Загрузить конфиг из файла `path`.
pub fn load_config(path: &str) -> anyhow::Result<AppConfig> {
    let builder = config::Config::builder()
        .add_source(config::File::from(std::path::Path::new(path)).required(false))
        .add_source(config::Environment::with_prefix("BH").prefix_separator("_").separator("__"));

    let cfg = builder.build()?;
    cfg.try_deserialize().map_err(Into::into)
}
