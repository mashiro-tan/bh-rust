//! Кастомный DNS-резолвер с защитой от SSRF.
//!
//! Реализует `reqwest::dns::Resolve` — резолвит домены через системный
//! резолвер и отфильтровывает приватные/опасные IP-адреса **до**
//! установления TCP-соединения. Это защищает от DNS rebinding атак.

use std::net::SocketAddr;
use std::pin::Pin;

use reqwest::dns::{Addrs, Name, Resolve};
use tracing::warn;

use crate::ssrf::is_ip_safe;

/// DNS-резолвер, который блокирует подключение к приватным IP.
///
/// Используется как кастомный резолвер для `reqwest::Client`.
/// Каждый резолв проходит через `ssrf::is_ip_safe` — если все IP
/// для домена заблокированы, возвращается ошибка.
#[derive(Clone)]
pub struct SsrfDnsResolver;

impl Resolve for SsrfDnsResolver {
    fn resolve(
        &self,
        name: Name,
    ) -> Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Addrs, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    > {
        Box::pin(async move {
            let domain = name.as_str();

            // Используем tokio::net::lookup_host для асинхронного резолва.
            // Порт 0 — reqwest подставит правильный порт по схеме (80/443).
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((domain, 0))
                .await?
                .collect();

            if addrs.is_empty() {
                return Err(format!("No IP addresses resolved for '{}'", domain).into());
            }

            // Фильтрация: оставляем только безопасные IP
            let mut safe_addrs = Vec::new();
            for addr in &addrs {
                if is_ip_safe(addr.ip()) {
                    safe_addrs.push(*addr);
                } else {
                    warn!(
                        domain = domain,
                        blocked_ip = %addr.ip(),
                        "SSRF protection: blocked connection to private/reserved IP"
                    );
                }
            }

            if safe_addrs.is_empty() {
                return Err(format!(
                    "SSRF protection: all resolved IPs for '{}' are blocked (private/reserved range)",
                    domain
                ).into());
            }

            Ok(Box::new(safe_addrs.into_iter()) as Addrs)
        })
    }
}
