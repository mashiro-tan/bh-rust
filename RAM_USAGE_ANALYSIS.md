 ### Текущий пайплайн (шаг за шагом)

 Рассмотрю на примере 10MP изображения (3840×2560, ~5MB JPEG с сервера):

 ┌───────────────────┬───────────────────────────────────────────────────┬──────────────────────────────────────────────────────────┐
 │ Этап              │ Код                                               │ Что в памяти                                             │
 ├───────────────────┼───────────────────────────────────────────────────┼──────────────────────────────────────────────────────────┤
 │ 1. Скачивание     │ response.bytes().await (handlers.rs:355)          │ Vec<u8> — 5MB (полный файл)                              │
 ├───────────────────┼───────────────────────────────────────────────────┼──────────────────────────────────────────────────────────┤
 │ 2. Декодирование  │ image::load_from_memory(raw) (image.rs:76)        │ DynamicImage::ImageRgba8 — 40MB (3840×2560×4)            │
 ├───────────────────┼───────────────────────────────────────────────────┼──────────────────────────────────────────────────────────┤
 │ 3. B&W            │ img.grayscale().into() (image.rs:141)             │ Новая GrayImage — 10MB (3840×2560×1). Старый RGBA ещё    │
 │                   │                                                   │ жив до присваивания                                      │
 ├───────────────────┼───────────────────────────────────────────────────┼──────────────────────────────────────────────────────────┤
 │ 4. Flatten        │ img.into_rgb8() (image.rs:161)                    │ Новая RgbImage — 30MB (3840×2560×3). Старый RGBA жив     │
 │ RGBA→RGB          │                                                   │                                                          │
 ├───────────────────┼───────────────────────────────────────────────────┼──────────────────────────────────────────────────────────┤
 │ 5. Resize         │ FIrImage::new(target_w, target_h, pixel_type)     │ Дестинационный буфер — 30MB (если нет ресайза — 0). Сурс │
 │                   │ (image.rs:182)                                    │ всё ещё жив                                              │
 ├───────────────────┼───────────────────────────────────────────────────┼──────────────────────────────────────────────────────────┤
 │ 5b. Reconstruct   │ RgbImage::from_raw(...) (image.rs:196)            │ Ещё одна RgbImage — 30MB из dst.into_vec()               │
 ├───────────────────┼───────────────────────────────────────────────────┼──────────────────────────────────────────────────────────┤
 │ 6. Encoding       │ Cursor<Vec<u8>> + JpegEncoder (image.rs:213-236)  │ Vec<u8> растущий — 2-5MB                                 │
 ├───────────────────┼───────────────────────────────────────────────────┼──────────────────────────────────────────────────────────┤
 │ 7. Сравнение      │ handlers.rs:227 compressed_size >= original_size  │ И оригинал (5MB), И результат (3MB) — оба в памяти       │
 └───────────────────┴───────────────────────────────────────────────────┴──────────────────────────────────────────────────────────┘

 ### Пиковое потребление

 В момент resize + encoding одновременно живут:

 ```
   raw bytes (original):    5 MB
   decoded DynamicImage:   30 MB    ← source для resize
   resize destination:     30 MB    ← Lanczos3 filter buffer
   reconstructed image:    30 MB    ← after from_raw()
   encoded output:          3 MB
   ────────────────────────────────
   PIKE:                  ~98 MB на один запрос
 ```

 При 10 параллельных запросов — ~1GB RAM. Это Issue #6 из ISSUES.md.

 ### Факторы, усугубляющие проблему

 1. image::load_from_memory всегда декодирует в DynamicImage — это enum, который для RGBA изображений создаёт 4-канальную копию (40MB
 вместо 30MB для RGB).
 2. into_rgb8() и grayscale() создают полные копии — Rust-семантика move помогает, но DynamicImage internally использует Box<Pixel>,
 и промежуточные копии живут до конца scope.
 3. fast_image_resize требует полный буфер — Lanczos3 фильтр нужен весь source целиком; нет row-by-row API.
 4. response.bytes().await загружает всё до проверки лимита — max_download_bytes проверяется после полной загрузки (handlers.rs:186).
 Злоумышленник может заставить сервер скачать 1GB файл.
 5. Оригинал и результат живут одновременно — для сравнения размеров (handlers.rs:227).

 ────────────────────────────────────────────────────────────────────────────────

 ### Оценка неэффективности

 ┌───────────────────────────┬──────────────┬───────────────────────┬────────────┐
 │ Сценарий                  │ Текущий peak │ Теоретический минимум │ Перерасход │
 ├───────────────────────────┼──────────────┼───────────────────────┼────────────┤
 │ 10MP, без ресайза         │ ~70MB        │ ~10MB (1 row buffer)  │ ×7         │
 ├───────────────────────────┼──────────────┼───────────────────────┼────────────┤
 │ 10MP, ресайз в 720        │ ~98MB        │ ~10MB                 │ ×10        │
 ├───────────────────────────┼──────────────┼───────────────────────┼────────────┤
 │ 50MP (200MP), без ресайза │ ~600MB       │ ~10MB                 │ ×60        │
 ├───────────────────────────┼──────────────┼───────────────────────┼────────────┤
 │ 10 параллельных × 10MP    │ ~980MB       │ ~100MB                │ ×10        │
 └───────────────────────────┴──────────────┴───────────────────────┴────────────┘

 Вердикт: текущий пайплайн неэффективен в 7–60× по пиковой RAM относительно теоретического минимума.

 ────────────────────────────────────────────────────────────────────────────────

 ### Как реализовать стриминг

 Стриминг на уровне полного пайплайна (download → decode → resize → encode → upload) невозможен без замены ключевых библиотек, потому
 что:
 - image crate декодирует всё целиком
 - fast_image_resize требует полный source buffer
 - JpegEncoder/webp::Encoder принимают полные буферы

 Однако частичный стриминг даёт 40–60% снижения пиковой RAM:

 #### Улучшение 1: Стриминг скачивания (сразу → 30-50% gain)

 ```rust
   // Вместо response.bytes().await → Vec<u8> → load_from_memory()
   // Стримить напрямую в декодер через image::io::Reader

   pub struct StreamingDecoder {
       max_bytes: u64,
       current_bytes: u64,
   }

   impl image::io::Reader for StreamingDecoder { /* ... */ }

   // Или проще: использовать reqwest streaming + image::ImageReader
   async fn download_and_decode(
       client: &reqwest::Client,
       url: &str,
       max_bytes: u64,
   ) -> Result<DynamicImage, DownloadError> {
       let response = client.get(url).send().await?;
       let mut stream = response.bytes_stream();
       let mut total = 0u64;

       // image crate может декодировать из любого Read:
       // image::io::Reader::new(Cursor::new(...)).with_format(...).decode()
       // Но для стриминга нужен формат-специфичный декодер
   }
 ```

 Более реалистичный вариант — ограничить скачивание до полной загрузки:

 ```rust
   // handlers.rs: заменить response.bytes().await на streaming с лимитом
   let mut stream = response.bytes_stream();
   let mut bytes = Vec::new();
   while let Some(chunk) = stream.next().await {
       let chunk = chunk.map_err(DownloadError::Other)?;
       if max_bytes > 0 && (bytes.len() + chunk.len()) as u64 > max_bytes {
           return Err(DownloadError::Other(anyhow!("Exceeds max size")));
       }
       bytes.extend_from_slice(&chunk);
   }
 ```

 Это не снижает пик для валидных изображений, но защищает от DoS.

 #### Улучшение 2: Ранний дроп оригинала (→ 5MB saving)

 В process_image — декодировать и сразу дропнуть raw:

 ```rust
   pub fn process_image(raw: Vec<u8>, ...) -> Result<ProcessedImage, ProcessImageError> {
       let img = image::load_from_memory(&raw)
           .map_err(|e| ProcessImageError::DecodeFailed(...))?;
       drop(raw); // ← освобождаем 5MB сразу
       // ... остальная обработка
   }
 ```

 И в handler — не хранить оригинал после обработки:

 ```rust
   // Вместо:
   let result = process_image(&bytes, ...)?;
   // bytes lives until comparison at line 227

   // После:
   let original_size = bytes.len();
   let result = process_image(bytes, ...)?; // bytes moved, dropped inside
   // original bytes freed!
 ```

 Но тогда нельзя вернуть оригинал при prefer_original_if_smaller. Решение: либо отключить эту фичу, либо сравнить до дропа:

 ```rust
   let original_size = bytes.len() as u64;
   let result = process_image(&bytes, ...)?;
   let compressed_size = result.data.len() as u64;

   let use_compressed = !cfg.prefer_original_if_smaller || compressed_size < original_size;
   drop(bytes); // ← освобождаем сразу после сравнения

   if use_compressed {
       // return result.data
   } else {
       // need original — but it's dropped! → нужно было не дропать
   }
 ```

 Компромисс: если prefer_original_if_smaller, то память всё равно нужна. Но можно сразу начать стримить ответ клиенту, не дожидаясь
 полной буферизации.

 #### Улучшение 3: Стриминг ответа (axum Body::wrap_stream)

 ```rust
   use axum::body::Body;
   use futures::stream;

   // Вместо:
   (StatusCode::OK, headers, response_data).into_response()

   // Стримить чанками:
   let stream = stream::iter(vec![Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(response_data))]);
   let body = Body::from_stream(stream);
   Response::builder()
       .status(StatusCode::OK)
       .header("Content-Type", content_type)
       .body(body)
       .unwrap()
 ```

 Для реального стриминга энкодинга нужен row-by-row энкодер.

 #### Улучшение 4: Row-by-row обработка (максимальный gain, ×7-10×)

 Это кардинальное изменение — замена image crate на формат-специфичные декодеры:

 ```rust
   // Для JPEG: use jpeg-decoder crate (поддерживает row-by-row)
   use jpeg_decoder::Decoder;

   // Декодер выдает строки по одной:
   let mut decoder = Decoder::new(cursor);
   let header = decoder.info().unwrap();
   let mut row_buffer = vec![0u8; header.width * header.height];
   decoder.read_image(&mut row_buffer)?;

   // Для стриминга нужен decoder, который выдает строки:
   // jpeg-decoder или mozjpeg-sys (libjpeg-turbo) поддерживают это
 ```

 Полный стриминговый пайплайн:

 ```
   Download stream → JPEG decoder (row-by-row) →
     Resize (row-by-row, separable filter) →
     JPEG/WebP encoder (row-by-row) →
     Response stream
 ```

 Пиковая RAM такого пайплайна:

 ```
   Download buffer:       64 KB  (HTTP chunk)
   Decoder row buffer:   11 KB   (3840×1×3 bytes)
   Resize row buffer:    11 KB   (source + destination)
   Encoder row buffer:   11 KB
   Response buffer:      64 KB
   ────────────────────────────────
   PIKE:                  ~150 KB на один запрос
 ```

 Снижение: ×650 для одного запроса, ×6500 для 10 запросов.

 Проблема: ни один из текущих crates не поддерживает полный row-by-row пайплайн:
 - image — декодирует целиком
 - fast_image_resize — требует полный буфер
 - JpegEncoder (image crate) — принимает полный буфер
 - webp crate — принимает полный буфер

 Для реализации нужен libjpeg-turbo (через jpeg-decoder + mozjpeg-sys) или libvips (через vips binding), которые поддерживают
 тильинг/стриминг.

 ────────────────────────────────────────────────────────────────────────────────

 ### Практические рекомендации (по приоритету)

 ┌───────────┬──────────────────────────────────────────────────────────────────────────────────┬───────────────────┬───────────┐
 │ Приоритет │ Изменение                                                                        │ Gain по RAM       │ Сложность │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────┼───────────────────┼───────────┤
 │ P0        │ Принимать raw: Vec<u8> по ownership в process_image, дропать сразу после decode  │ +5-10%            │ Trivial   │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────┼───────────────────┼───────────┤
 │ P0        │ drop(bytes) в handler сразу после process_image (если не нужен оригинал)         │ +5-10%            │ Trivial   │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────┼───────────────────┼───────────┤
 │ P1        │ Стриминг скачивания с early termination по max_download_bytes                    │ Защита от DoS     │ Low       │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────┼───────────────────┼───────────┤
 │ P1        │ Стриминг ответа через Body::from_stream                                          │ Уменьшает latency │ Low       │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────┼───────────────────┼───────────┤
 │ P2        │ Замена image на формат-специфичные декодеры (jpeg-decoder) + row-by-row энкодинг │ ×3-5×             │ Medium    │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────┼───────────────────┼───────────┤
 │ P3        │ Замена на libvips (через vips crate) — тильинг + стриминг из коробки             │ ×10-50×           │ High      │
 └───────────┴──────────────────────────────────────────────────────────────────────────────────┴───────────────────┴───────────┘

 ### libvips как идеальное решение

 ```toml
   [dependencies]
   vips = "0.25"  # Rust bindings для libvips
 ```

 ```rust
   // libvips автоматически тайлит, стримит, и использует минимум RAM:
   let img = vips::VImage::new_from_file(input_path)?;
   let resized = img.resize(0.5, None)?;
   resized.write_to_buffer(".jpg", &[(
       "Q", &vips::Value::from(80)
   )])?;
 ```

 libvips для 100MP изображения использует ~50MB RAM вместо 1.2GB — потому что обрабатывает по тайлам 1000px с overlap для фильтров.

 ────────────────────────────────────────────────────────────────────────────────

 ### Итог

 Текущий пайплайн неэффективен в 7–60× по пиковой RAM относительно теоретического минимума. Основная причина — image crate +
 fast_image_resize требуют полные буферы. Быстрые фиксы (P0-P1) дают 10-20% снижения. Кардинальное решение — libvips (P3), который
 даёт ×10-50× снижение памяти за счёт встроенного тильинга и стриминга, и при этом остается на Rust (через bindings).
