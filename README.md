# bh-rust

High-performance image compression proxy for [Bandwidth Hero](https://github.com/ayastreb/bandwidth-hero) and [Komikku](https://github.com/komikku-app/komikku). Written in Rust with SIMD-accelerated Lanczos-3 resampling.

This is **not** an anonymizing proxy — it downloads images on the user's behalf, passing cookies and headers through to the origin host.

## Дисклеймер

Это **слопный** код. Из хорошего - он работает и выглядит более-менее прилично, насколько мне хватает понимания Rust кода. Из плохого - сесурити отсутствует, желающим использовать передачу заголовков следует иметь инстанс на собственной впске. Эффективность перегонки жпег в жпег под вопросом. Так же нужно учитывать, что с presigned urls с указанием оригинального ip клиента данное решение не работает (как не работает и с POST-запросами). Патч для Komikku расположен в соседней репе, такой же слопный. Ридми тут тоже слопный :^)
Возможно, в следующую итерацию я осилю собрать билд под mipsel с упаковкой для openwrt.

## Features

- **SIMD-accelerated resizing** — `fast_image_resize` with auto-detected SSE4.1/AVX2/NEON
- **Lanczos-3 resampling** — high-quality separable Lanczos filter by default
- **JPEG and WebP compression** with configurable quality (1–100)
- **Grayscale conversion** for additional savings
- **Header forwarding** — passes `Cookie`, `User-Agent`, `Referer`, and other headers to the origin (required for authenticated manga sources)
- **SOCKS5 proxy** support for outbound requests (only non-local ips for now due to draft ssrf guarding)
- **SSRF protection** — custom DNS resolver blocks private/reserved IPs, URL scheme validation (http/https only)
- **Statistics endpoint** — real-time counters for bandwidth saved
- **Single binary** — no runtime dependencies, ~14 MB release build

## Requirements

- **Rust 1.70+** (edition 2021)
- No other runtime dependencies

## Build

```bash
cargo build --release
```

The binary is at `target/release/bh-rust`.

### Cross-compilation for musl targets

For musl targets (e.g. `aarch64-unknown-linux-musl`, `x86_64-unknown-linux-musl`, `mipsel-unknown-linux-musl`),
set `LIBWEBP_SYS_STATIC=1` to statically link `libwebp`:

```bash
LIBWEBP_SYS_STATIC=1 cargo build --release --target <TARGET>
```

## Usage

```bash
# With default config.toml in the current directory
./target/release/bh-rust

# With a custom config file (--config or -c)
./target/release/bh-rust --config /etc/bh-rust/config.toml
./target/release/bh-rust -c /etc/bh-rust/config.toml

# With custom log level
RUST_LOG=info ./target/release/bh-rust
```

## Configuration

Copy `config.toml` to your desired location and edit:

```toml
[server]
host = "0.0.0.0"
port = 8080

[image]
resize_short_side = 720          # Resize if shorter side exceeds this (0 = off)
quality = 80                     # Compression quality (1-100)
default_jpeg = false             # Convert to JPEG by default (false = WebP)
default_bw = false               # Grayscale by default
max_download_bytes = 0           # Max source image size (0 = unlimited)
prefer_original_if_smaller = true # Return original if compressed is larger
max_image_dimension = 65535      # Max source dimension by any side (0 = unlimited)
max_target_dimension = 0         # Max target long side (0 = unlimited)
                                # Effective limit: min(max_target_dimension, format hard limit)
                                # Hard limits: 16383 for WebP, 65535 for JPEG

# Optional SOCKS5 proxy for outbound requests
# [proxy]
# address = "127.0.0.1:1080"
# username = "user"
# password = "pass"
```

All config values can also be set via environment variables with the `BH` prefix.
Nested keys use `_` as prefix separator and `__` as key separator:

```bash
BH_image_quality=90 BH_server_port=9090 ./target/release/bh-rust
```

### CLI Arguments

```bash
# With default config.toml in the current directory
./target/release/bh-rust

# With a custom config file (--config or -c)
./target/release/bh-rust --config /etc/bh-rust/config.toml
./target/release/bh-rust -c /etc/bh-rust/config.toml
```

### Defaults

| Parameter | Default | Description |
|---|---|---|
| `server.host` | `0.0.0.0` | Listen address |
| `server.port` | `8080` | Listen port |
| `image.resize_short_side` | `720` | Short side threshold (px) |
| `image.quality` | `80` | Compression quality (1–100) |
| `image.default_jpeg` | `false` | WebP by default |
| `image.default_bw` | `false` | Color by default |
| `image.max_download_bytes` | `0` | No download size limit |
| `image.prefer_original_if_smaller` | `true` | Return original if compression grows file |
| `image.max_image_dimension` | `65535` | Max source dimension (px) |
| `image.max_target_dimension` | `0` | No target dimension limit |

## API

### Compress image

```
GET /?url=<IMAGE_URL>[&jpg=1][&l=80][&bw=0][&resize=720][&headers=<BASE64_JSON>]
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `url` | string | **Required.** URL of the image to compress. |
| `jpg` | `0` or `1` | Convert to JPEG. Falls back to config default if absent. |
| `l` | `1`–`100` | Compression quality (JPEG/WebP). Falls back to config default if absent. |
| `bw` | `0` or `1` | Convert to grayscale. Falls back to config default if absent. |
| `resize` | integer | Max short side in pixels (`0` = no resize). Falls back to config default if absent. |
| `headers` | base64 | Base64-encoded JSON map of HTTP headers to forward to the origin (e.g. `{"Cookie":"sid=abc","User-Agent":"..."}`). Uses URL-safe base64 without padding. |

**Example:**

```
GET /?url=https://example.com/image.png&jpg=1&l=75&resize=1080
```

**Response:** The compressed image with `Content-Type: image/jpeg` or `image/webp` depending on `jpg` param and config default (or the original if compression made it larger and `prefer_original_if_smaller` is enabled).

### Health check

```
GET /health
```

Returns `{"status": "ok", "service": "bh-rust"}`.

### Statistics

```
GET /stats
```

Returns real-time counters:

```json
{
  "startedOn": "2024-07-15T12:00:00Z",
  "sourceTotal": 524288000,
  "compressedTotal": 104857600,
  "droppedTotal": 1048576,
  "destTotal": 105906176
}
```

| Field | Description |
|-------|-------------|
| `startedOn` | RFC 3339 (ISO 8601) timestamp when the server started. |
| `sourceTotal` | Total bytes downloaded from origin servers. |
| `compressedTotal` | Total bytes served as compressed images. |
| `droppedTotal` | Total bytes of compressed images discarded (original was smaller). |
| `destTotal` | Total bytes actually served to clients. |

## Running behind a reverse proxy

Example `nginx` config:

```nginx
location /bh/ {
    proxy_pass http://127.0.0.1:8080/;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
}
```

## License

GPL-3.0 license
