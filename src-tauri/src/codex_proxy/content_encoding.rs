//! HTTP content-encoding helpers.
//!
//! Ported from cc-switch `proxy/content_encoding.rs`. reqwest auto-decompression
//! is disabled on the forwarding path (we force `accept-encoding: identity`),
//! but the Codex client may still compress its request bodies (zstd in
//! logged-in Desktop mode), so incoming bodies are decoded here.

use axum::http::header::HeaderMap;
use std::io::Read;

/// 把 content-encoding 值拆成有序 coding 列表（去掉 identity 与空值）。
///
/// HTTP 允许堆叠编码（如 `gzip, zstd`），各 coding 以逗号分隔；亦允许重复
/// content-encoding 头，语义等同逗号拼接（见 [`get_content_encoding`]）。
fn split_codings(content_encoding: &str) -> Vec<&str> {
    content_encoding
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty() && *c != "identity")
        .collect()
}

/// 单个 coding 是否可被解压。
fn is_single_supported(coding: &str) -> bool {
    matches!(
        coding,
        "gzip" | "x-gzip" | "deflate" | "br" | "zstd" | "zst"
    )
}

/// 解压失败原因。把「输出超预算」与「数据损坏」区分开：前者是安全拒绝信号。
#[derive(Debug)]
pub(crate) enum DecompressError {
    /// 底层解码失败（数据损坏 / 格式不符）。
    Io(std::io::Error),
    /// 解压输出超过 `limit` 字节即中止；此时真实输出大小未知，只会大于 limit。
    TooLarge { limit: usize },
}

impl std::fmt::Display for DecompressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::TooLarge { limit } => write!(f, "解压输出超过上限 {limit} 字节"),
        }
    }
}

impl std::error::Error for DecompressError {}

impl From<std::io::Error> for DecompressError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<DecompressError> for std::io::Error {
    fn from(e: DecompressError) -> Self {
        match e {
            DecompressError::Io(e) => e,
            DecompressError::TooLarge { limit } => {
                std::io::Error::other(format!("decompressed body exceeds {limit} bytes"))
            }
        }
    }
}

/// 从解码器读取解压输出，最多 `max_bytes`；一旦输出超过预算立即中止读取并返回
/// [`DecompressError::TooLarge`] —— 压缩炸弹在预算耗尽处被截停，而不是先在内存里
/// 完整展开再比较大小。
fn read_with_output_limit<R: Read>(
    reader: R,
    max_bytes: usize,
) -> Result<Vec<u8>, DecompressError> {
    // saturating_add：无界调用（max_bytes = usize::MAX）时预算保持 usize::MAX
    let budget = max_bytes.saturating_add(1) as u64;
    let mut limited = reader.take(budget);
    let mut out = Vec::new();
    limited.read_to_end(&mut out)?;
    if out.len() > max_bytes {
        return Err(DecompressError::TooLarge { limit: max_bytes });
    }
    Ok(out)
}

/// 解压单个 content-coding，输出上限 `max_output_bytes`。未知编码返回 `Ok(None)`。
fn decompress_single(
    coding: &str,
    body: &[u8],
    max_output_bytes: usize,
) -> Result<Option<Vec<u8>>, DecompressError> {
    match coding {
        "gzip" | "x-gzip" => {
            let decoder = flate2::read::GzDecoder::new(body);
            Ok(Some(read_with_output_limit(decoder, max_output_bytes)?))
        }
        "deflate" => {
            // RFC 9110: deflate 指 zlib 包裹格式；但部分上游 / 客户端发 raw deflate 流。
            // 先按规范尝试 zlib，失败再回退 raw —— 否则合规来源必然解压失败。
            let zlib = flate2::read::ZlibDecoder::new(body);
            match read_with_output_limit(zlib, max_output_bytes) {
                Ok(decompressed) => Ok(Some(decompressed)),
                Err(_) => {
                    let raw = flate2::read::DeflateDecoder::new(body);
                    Ok(Some(read_with_output_limit(raw, max_output_bytes)?))
                }
            }
        }
        "br" => {
            let decoder = brotli::Decompressor::new(std::io::Cursor::new(body), 4096);
            Ok(Some(read_with_output_limit(decoder, max_output_bytes)?))
        }
        "zstd" | "zst" => {
            // Codex 登录态对请求体启用 zstd（Compression::Zstd）。
            let decoder = zstd::stream::read::Decoder::new(std::io::Cursor::new(body))?;
            Ok(Some(read_with_output_limit(decoder, max_output_bytes)?))
        }
        _ => Ok(None),
    }
}

/// 根据 content-encoding 解压 body 字节，支持堆叠编码（如 `gzip, zstd`），
/// 且每个 coding 的解压输出（含堆叠编码的中间产物）都受 `max_output_bytes`
/// 限制，超限即中止并返回 [`DecompressError::TooLarge`]。
///
/// RFC 9110 §8.4：codings 按**应用顺序**列出，故解压须**反向**（最后应用的先解）。
/// 返回 `Ok(None)` 表示存在不受支持的编码、原样透传。
pub(crate) fn decompress_body_with_limit(
    content_encoding: &str,
    body: &[u8],
    max_output_bytes: usize,
) -> Result<Option<Vec<u8>>, DecompressError> {
    let codings = split_codings(content_encoding);
    if codings.is_empty() {
        return Ok(None);
    }
    // 任一 coding 不支持就整体放弃解压、保头透传，避免半解码的脏数据。
    if !codings.iter().all(|c| is_single_supported(c)) {
        return Ok(None);
    }

    // 反向解码：列表末尾是最后应用的编码，须最先解。
    let mut data: Option<Vec<u8>> = None;
    for coding in codings.iter().rev() {
        let input = data.as_deref().unwrap_or(body);
        match decompress_single(coding, input, max_output_bytes)? {
            Some(decompressed) => data = Some(decompressed),
            // 上面 is_single_supported 已校验，理论不会发生；防御性兜底。
            None => return Ok(None),
        }
    }
    Ok(data)
}

/// 无输出上限的 [`decompress_body_with_limit`] 版本。
///
/// 新的 HTTP 入口不应使用这个兼容 helper；它只保留给已经在解压前完成
/// 严格大小约束的内部调用方。
#[cfg(test)]
pub(crate) fn decompress_body(
    content_encoding: &str,
    body: &[u8],
) -> Result<Option<Vec<u8>>, std::io::Error> {
    decompress_body_with_limit(content_encoding, body, usize::MAX).map_err(Into::into)
}

/// 该 content-encoding（含堆叠，如 `gzip, zstd`）是否全部可被解压。
///
/// 请求侧用它做闸门：无法解压的压缩体不能透传给 JSON 解析，需直接拒绝。
pub(crate) fn is_supported_content_encoding(content_encoding: &str) -> bool {
    let codings = split_codings(content_encoding);
    !codings.is_empty() && codings.iter().all(|c| is_single_supported(c))
}

/// 从 header 提取 content-encoding（合并重复头，忽略 identity 与空值）。
///
/// HTTP 允许重复 content-encoding 头，语义等同逗号拼接，故用 `get_all` 合并；
/// 返回值可能含多个逗号分隔的 coding，交由 [`decompress_body`] 反向解码。
pub(crate) fn get_content_encoding(headers: &HeaderMap) -> Option<String> {
    let combined = headers
        .get_all("content-encoding")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
        .to_lowercase();
    if split_codings(&combined).is_empty() {
        return None;
    }
    Some(combined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip_bytes(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        );
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn zstd_bytes(data: &[u8]) -> Vec<u8> {
        zstd::stream::encode_all(std::io::Cursor::new(data), 3).unwrap()
    }

    #[test]
    fn decompress_body_zstd_roundtrip() {
        let compressed = zstd_bytes(b"hello codex");
        let decoded = decompress_body("zstd", &compressed).unwrap().unwrap();
        assert_eq!(decoded, b"hello codex");
    }

    #[test]
    fn decompress_body_gzip_roundtrip() {
        let compressed = gzip_bytes(b"hello proxy");
        let decoded = decompress_body("gzip", &compressed).unwrap().unwrap();
        assert_eq!(decoded, b"hello proxy");
    }

    #[test]
    fn decompress_body_stacked_gzip_then_zstd_decodes_in_reverse() {
        let double = zstd_bytes(&gzip_bytes(b"stacked"));
        let decoded = decompress_body("gzip, zstd", &double).unwrap().unwrap();
        assert_eq!(decoded, b"stacked");
    }

    #[test]
    fn decompress_body_unknown_encoding_returns_none_to_keep_headers() {
        assert!(decompress_body("gzip, br2", b"x").unwrap().is_none());
    }

    #[test]
    fn decompress_body_with_limit_rejects_zstd_bomb() {
        let bomb = zstd_bytes(&b"A".repeat(1_000_000));
        let err = decompress_body_with_limit("zstd", &bomb, 10_000).unwrap_err();
        assert!(matches!(err, DecompressError::TooLarge { .. }));
    }

    #[test]
    fn is_supported_content_encoding_matches_decompressable() {
        assert!(is_supported_content_encoding("zstd"));
        assert!(is_supported_content_encoding("gzip, zstd"));
        assert!(!is_supported_content_encoding("gzip, br2"));
        assert!(!is_supported_content_encoding("identity"));
    }

    #[test]
    fn get_content_encoding_ignores_identity_only() {
        let mut headers = HeaderMap::new();
        assert!(get_content_encoding(&headers).is_none());
        headers.insert("content-encoding", "identity".parse().unwrap());
        assert!(get_content_encoding(&headers).is_none());
        headers.insert("content-encoding", "zstd".parse().unwrap());
        assert_eq!(get_content_encoding(&headers).as_deref(), Some("zstd"));
    }
}
