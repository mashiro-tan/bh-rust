//! SSRF (Server-Side Request Forgery) защита.
//!
//! Валидирует IP-адреса и URL-схемы для предотвращения доступа
//! к внутренним ресурсам, cloud metadata и локальным сервисам.

use std::net::IpAddr;
use url::Url;

/// Проверить, что IP-адрес безопасен для внешних соединений.
///
/// Блокирует:
/// - Loopback (`127.0.0.0/8`, `::1`)
/// - Private (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `fc00::/7`)
/// - Link-local (`169.254.0.0/16`, `fe80::/10`) — включает cloud metadata!
/// - Unspecified (`0.0.0.0`, `::`)
/// - Documentation (`192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`)
pub fn is_ip_safe(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => {
            !(addr.is_private()
                || addr.is_loopback()
                || addr.is_link_local()
                || addr.is_documentation()
                || addr.is_unspecified())
        }
        IpAddr::V6(addr) => {
            !(addr.is_loopback()
                || addr.is_unique_local()
                || addr.is_unicast_link_local()
                || addr.is_unspecified())
        }
    }
}

/// Валидировать URL: разрешены только `http` и `https` схемы.
///
/// Возвращает `Ok(Url)` если схема допустима, иначе ошибку с описанием.
pub fn validate_url_scheme(url_str: &str) -> anyhow::Result<Url> {
    let url = Url::parse(url_str).map_err(|e| {
        anyhow::anyhow!("Invalid URL format: {}", e)
    })?;

    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(anyhow::anyhow!(
            "URL scheme '{}' is not allowed (only http/https are permitted)",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ip_safe_public() {
        assert!(is_ip_safe("8.8.8.8".parse().unwrap()));
        assert!(is_ip_safe("1.1.1.1".parse().unwrap()));
        assert!(is_ip_safe("8.8.4.4".parse().unwrap()));
    }

    #[test]
    fn test_is_ip_safe_blocks_loopback() {
        assert!(!is_ip_safe("127.0.0.1".parse().unwrap()));
        assert!(!is_ip_safe("127.255.255.255".parse().unwrap()));
        assert!(!is_ip_safe("::1".parse().unwrap()));
    }

    #[test]
    fn test_is_ip_safe_blocks_private() {
        assert!(!is_ip_safe("10.0.0.1".parse().unwrap()));
        assert!(!is_ip_safe("10.255.255.255".parse().unwrap()));
        assert!(!is_ip_safe("172.16.0.1".parse().unwrap()));
        assert!(!is_ip_safe("172.31.255.255".parse().unwrap()));
        assert!(!is_ip_safe("192.168.0.1".parse().unwrap()));
        assert!(!is_ip_safe("192.168.255.255".parse().unwrap()));
        assert!(!is_ip_safe("fd00::1".parse().unwrap()));
    }

    #[test]
    fn test_is_ip_safe_blocks_link_local() {
        // Cloud metadata — самая опасная!
        assert!(!is_ip_safe("169.254.169.254".parse().unwrap()));
        assert!(!is_ip_safe("169.254.0.1".parse().unwrap()));
        assert!(!is_ip_safe("fe80::1".parse().unwrap()));
    }

    #[test]
    fn test_is_ip_safe_blocks_unspecified() {
        assert!(!is_ip_safe("0.0.0.0".parse().unwrap()));
        assert!(!is_ip_safe("::".parse().unwrap()));
    }

    #[test]
    fn test_is_ip_safe_blocks_documentation() {
        assert!(!is_ip_safe("192.0.2.1".parse().unwrap()));
        assert!(!is_ip_safe("198.51.100.1".parse().unwrap()));
        assert!(!is_ip_safe("203.0.113.1".parse().unwrap()));
    }

    #[test]
    fn test_validate_url_scheme_allows_http() {
        assert!(validate_url_scheme("http://example.com/image.png").is_ok());
    }

    #[test]
    fn test_validate_url_scheme_allows_https() {
        assert!(validate_url_scheme("https://example.com/image.png").is_ok());
    }

    #[test]
    fn test_validate_url_scheme_blocks_file() {
        let result = validate_url_scheme("file:///etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("file"));
    }

    #[test]
    fn test_validate_url_scheme_blocks_gopher() {
        assert!(validate_url_scheme("gopher://example.com").is_err());
    }

    #[test]
    fn test_validate_url_scheme_blocks_invalid() {
        assert!(validate_url_scheme("not-a-url").is_err());
    }
}
