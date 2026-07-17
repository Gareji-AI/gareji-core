//! LSP-style Content-Length framing with strict header bounds.

use agentmesh_proto::limits::Limits;
use bytes::{Buf, BytesMut};
use std::collections::BTreeMap;
use thiserror::Error;

/// Framing-specific limits derived from host limits.
#[derive(Debug, Clone, Copy)]
pub struct FrameLimits {
    /// Max body bytes.
    pub frame_max_bytes: usize,
    /// Max header block.
    pub header_block_max_bytes: usize,
    /// Max bytes per header line.
    pub header_line_max_bytes: usize,
    /// Max header lines.
    pub header_max_lines: usize,
    /// Max Content-Length digit count.
    pub content_length_digits_max: usize,
}

impl From<&Limits> for FrameLimits {
    fn from(limits: &Limits) -> Self {
        Self {
            frame_max_bytes: limits.frame_max_bytes,
            header_block_max_bytes: limits.header_block_max_bytes,
            header_line_max_bytes: limits.header_line_max_bytes,
            header_max_lines: limits.header_max_lines,
            content_length_digits_max: limits.content_length_digits_max,
        }
    }
}

impl Default for FrameLimits {
    fn default() -> Self {
        Self::from(&Limits::default())
    }
}

/// Frame decode failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FrameDecodeError {
    /// Malformed or unbounded headers.
    #[error("invalid framing: {0}")]
    InvalidFraming(String),
    /// Declared/accumulated body too large.
    #[error("frame too large: {0}")]
    FrameTooLarge(String),
    /// Stream ended before a complete frame.
    #[error("unexpected EOF while reading frame")]
    UnexpectedEof,
}

/// Decoded frame plus ignored unknown headers for audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    /// Raw UTF-8 JSON body bytes (validated as UTF-8 separately by callers).
    pub body: Vec<u8>,
    /// Unknown syntactically valid headers (excluding Content-Length).
    pub unknown_headers: BTreeMap<String, String>,
}

/// Incremental frame decoder. Never uses unbounded read_line.
#[derive(Debug)]
pub struct FrameDecoder {
    limits: FrameLimits,
    buf: BytesMut,
    state: DecodeState,
}

#[derive(Debug)]
enum DecodeState {
    Headers {
        block: Vec<u8>,
        line_start: usize,
        lines: usize,
    },
    Body {
        length: usize,
        unknown_headers: BTreeMap<String, String>,
    },
}

impl FrameDecoder {
    /// Create a decoder with the given limits.
    pub fn new(limits: FrameLimits) -> Self {
        Self {
            limits,
            buf: BytesMut::new(),
            state: DecodeState::Headers {
                block: Vec::new(),
                line_start: 0,
                lines: 0,
            },
        }
    }

    /// Push incoming bytes and attempt to decode zero or one complete frame.
    pub fn push(&mut self, data: &[u8]) -> Result<Option<DecodedFrame>, FrameDecodeError> {
        self.buf.extend_from_slice(data);
        self.try_decode()
    }

    /// Signal EOF; return error if a partial frame remains.
    pub fn finish(&mut self) -> Result<Option<DecodedFrame>, FrameDecodeError> {
        if self.buf.is_empty() {
            match &self.state {
                DecodeState::Headers { block, .. } if block.is_empty() => Ok(None),
                DecodeState::Headers { .. } | DecodeState::Body { .. } => {
                    Err(FrameDecodeError::UnexpectedEof)
                }
            }
        } else {
            self.try_decode()?
                .map_or_else(|| Err(FrameDecodeError::UnexpectedEof), |f| Ok(Some(f)))
        }
    }

    /// True if any buffered bytes remain unread for framing.
    pub fn has_pending(&self) -> bool {
        if !self.buf.is_empty() {
            return true;
        }
        match &self.state {
            DecodeState::Body { .. } => true,
            DecodeState::Headers { block, .. } => !block.is_empty(),
        }
    }

    /// Bytes currently buffered.
    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    fn try_decode(&mut self) -> Result<Option<DecodedFrame>, FrameDecodeError> {
        loop {
            match &mut self.state {
                DecodeState::Headers {
                    block,
                    line_start,
                    lines,
                } => {
                    if self.buf.is_empty() {
                        return Ok(None);
                    }
                    let byte = self.buf[0];
                    self.buf.advance(1);
                    block.push(byte);
                    if block.len() > self.limits.header_block_max_bytes {
                        return Err(FrameDecodeError::InvalidFraming(format!(
                            "header block exceeds {} bytes",
                            self.limits.header_block_max_bytes
                        )));
                    }
                    let line_len = block.len() - *line_start;
                    if line_len > self.limits.header_line_max_bytes {
                        return Err(FrameDecodeError::InvalidFraming(format!(
                            "header line exceeds {} bytes without CRLF",
                            self.limits.header_line_max_bytes
                        )));
                    }
                    if block.len() >= 2 && &block[block.len() - 2..] == b"\r\n" {
                        let line_end = block.len() - 2;
                        let line_bytes = &block[*line_start..line_end];
                        if line_bytes.is_empty() {
                            // End of headers: blank line.
                            let header_bytes = block[..*line_start].to_vec();
                            let (length, unknown) = parse_headers(
                                &header_bytes,
                                self.limits.content_length_digits_max,
                                self.limits.frame_max_bytes,
                                self.limits.header_max_lines,
                            )?;
                            self.state = DecodeState::Body {
                                length,
                                unknown_headers: unknown,
                            };
                            continue;
                        }
                        *lines += 1;
                        if *lines > self.limits.header_max_lines {
                            return Err(FrameDecodeError::InvalidFraming(format!(
                                "more than {} header lines",
                                self.limits.header_max_lines
                            )));
                        }
                        *line_start = block.len();
                    }
                }
                DecodeState::Body {
                    length,
                    unknown_headers,
                } => {
                    if self.buf.len() < *length {
                        return Ok(None);
                    }
                    let body = self.buf.split_to(*length).to_vec();
                    let unknown = std::mem::take(unknown_headers);
                    self.state = DecodeState::Headers {
                        block: Vec::new(),
                        line_start: 0,
                        lines: 0,
                    };
                    return Ok(Some(DecodedFrame {
                        body,
                        unknown_headers: unknown,
                    }));
                }
            }
        }
    }
}

fn parse_headers(
    header_bytes: &[u8],
    content_length_digits_max: usize,
    frame_max_bytes: usize,
    header_max_lines: usize,
) -> Result<(usize, BTreeMap<String, String>), FrameDecodeError> {
    let text = std::str::from_utf8(header_bytes)
        .map_err(|_| FrameDecodeError::InvalidFraming("header block is not valid UTF-8".into()))?;
    let mut content_length: Option<usize> = None;
    let mut unknown = BTreeMap::new();
    let mut line_count = 0usize;
    for raw_line in text.split("\r\n") {
        if raw_line.is_empty() {
            continue;
        }
        line_count += 1;
        if line_count > header_max_lines {
            return Err(FrameDecodeError::InvalidFraming(format!(
                "more than {header_max_lines} header lines"
            )));
        }
        let Some((name, value)) = raw_line.split_once(':') else {
            return Err(FrameDecodeError::InvalidFraming(format!(
                "malformed header line: {raw_line}"
            )));
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || !is_valid_header_name(name) {
            return Err(FrameDecodeError::InvalidFraming(format!(
                "malformed header name: {name}"
            )));
        }
        if !is_valid_header_value(value) {
            return Err(FrameDecodeError::InvalidFraming(format!(
                "malformed header value for {name}"
            )));
        }
        if name.eq_ignore_ascii_case("Content-Length") {
            if content_length.is_some() {
                return Err(FrameDecodeError::InvalidFraming(
                    "duplicate Content-Length header".into(),
                ));
            }
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(FrameDecodeError::InvalidFraming(
                    "Content-Length must be an unsigned decimal integer".into(),
                ));
            }
            if value.len() > content_length_digits_max {
                return Err(FrameDecodeError::FrameTooLarge(format!(
                    "Content-Length digit count {} exceeds max {content_length_digits_max}",
                    value.len()
                )));
            }
            let parsed: u64 = value.parse().map_err(|_| {
                FrameDecodeError::FrameTooLarge("Content-Length integer overflow".into())
            })?;
            if parsed > frame_max_bytes as u64 {
                return Err(FrameDecodeError::FrameTooLarge(format!(
                    "Content-Length {parsed} exceeds frame max {frame_max_bytes}"
                )));
            }
            content_length = Some(parsed as usize);
        } else {
            unknown.insert(name.to_string(), value.to_string());
        }
    }
    let length = content_length
        .ok_or_else(|| FrameDecodeError::InvalidFraming("missing Content-Length header".into()))?;
    Ok((length, unknown))
}

fn is_valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_valid_header_value(value: &str) -> bool {
    value.bytes().all(|b| (0x20..=0x7E).contains(&b))
}

/// Encode a JSON body into a Content-Length framed message.
pub struct FrameEncoder;

impl FrameEncoder {
    /// Frame body bytes (byte length, not character count).
    pub fn encode(body: &[u8]) -> Vec<u8> {
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut out = Vec::with_capacity(header.len() + body.len());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_byte_length_not_chars() {
        let body = "あ".as_bytes(); // 3 bytes
        let framed = FrameEncoder::encode(body);
        assert!(framed.starts_with(b"Content-Length: 3\r\n\r\n"));
        assert_eq!(&framed[framed.len() - 3..], body);
    }

    #[test]
    fn decodes_chunked_headers_and_body() {
        let mut dec = FrameDecoder::new(FrameLimits::default());
        let body = br#"{"ok":true}"#;
        let framed = FrameEncoder::encode(body);
        assert!(dec.push(&framed[..5]).unwrap().is_none());
        assert!(dec.push(&framed[5..20]).unwrap().is_none());
        let got = dec.push(&framed[20..]).unwrap().unwrap();
        assert_eq!(got.body, body);
    }

    #[test]
    fn rejects_duplicate_content_length() {
        let mut dec = FrameDecoder::new(FrameLimits::default());
        let msg = b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}";
        let err = dec.push(msg).unwrap_err();
        assert!(matches!(err, FrameDecodeError::InvalidFraming(_)));
    }

    #[test]
    fn rejects_huge_length_before_allocation() {
        let mut limits = FrameLimits::default();
        limits.frame_max_bytes = 16;
        let mut dec = FrameDecoder::new(limits);
        let msg = b"Content-Length: 999999999\r\n\r\n";
        let err = dec.push(msg).unwrap_err();
        assert!(matches!(err, FrameDecodeError::FrameTooLarge(_)));
    }

    #[test]
    fn ignores_unknown_headers_within_bounds() {
        let mut dec = FrameDecoder::new(FrameLimits::default());
        let body = b"{}";
        let msg = b"Content-Length: 2\r\nX-Trace: abc\r\n\r\n{}";
        let got = dec.push(msg).unwrap().unwrap();
        assert_eq!(got.body, body);
        assert_eq!(
            got.unknown_headers.get("X-Trace").map(String::as_str),
            Some("abc")
        );
    }

    #[test]
    fn byte_at_a_time() {
        let mut dec = FrameDecoder::new(FrameLimits::default());
        let body = br#"{"a":1}"#;
        let framed = FrameEncoder::encode(body);
        let mut result = None;
        for byte in framed {
            result = dec.push(&[byte]).unwrap();
            if result.is_some() {
                break;
            }
        }
        assert_eq!(result.unwrap().body, body);
    }
}
