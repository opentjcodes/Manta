use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::module_bus::{InterceptedRequest, InterceptedResponse, ModuleBus, ModuleDecision};

/// Streaming chunk size: 4 KB (fits inside a single CPU page, zero heap bloat).
pub const STREAM_CHUNK_SIZE: usize = 4096;

/// Result of a streaming fetch operation.
pub struct StreamFetchOutcome {
    pub final_url: String,
    pub status_code: u16,
    pub headers: HashMap<String, String>,
    pub total_bytes_streamed: usize,
}

/// On-the-fly HTTP/1.1 `Transfer-Encoding: chunked` state-machine decoder.
#[derive(Debug, Default)]
pub struct ChunkedTransferDecoder {
    reading_size: bool,
    size_hex_buf: String,
    remaining_in_chunk: usize,
    skip_crlf_bytes: u8,
    done: bool,
}

impl ChunkedTransferDecoder {
    pub fn new() -> Self {
        Self {
            reading_size: true,
            size_hex_buf: String::with_capacity(8),
            remaining_in_chunk: 0,
            skip_crlf_bytes: 0,
            done: false,
        }
    }

    pub fn decode_bytes(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &b in input {
            if self.done {
                break;
            }
            if self.skip_crlf_bytes > 0 {
                if b == b'\r' || b == b'\n' {
                    self.skip_crlf_bytes -= 1;
                    continue;
                } else {
                    self.skip_crlf_bytes = 0;
                }
            }

            if self.reading_size {
                if b == b'\r' {
                    continue;
                } else if b == b'\n' {
                    let clean_hex = self
                        .size_hex_buf
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .trim();
                    if clean_hex.is_empty() {
                        continue;
                    }
                    let chunk_len = usize::from_str_radix(clean_hex, 16).unwrap_or(0);
                    self.size_hex_buf.clear();
                    if chunk_len == 0 {
                        self.done = true;
                        break;
                    }
                    self.remaining_in_chunk = chunk_len;
                    self.reading_size = false;
                } else if self.size_hex_buf.len() < 16 {
                    self.size_hex_buf.push(b as char);
                }
            } else {
                out.push(b);
                self.remaining_in_chunk -= 1;
                if self.remaining_in_chunk == 0 {
                    self.reading_size = true;
                    self.skip_crlf_bytes = 2;
                }
            }
        }
    }
}

/// Pure-Rust zero-cache HTTP/1.1 streaming client with automatic redirect resolution
/// and on-the-fly `Transfer-Encoding: chunked` decoding.
pub struct StreamingNetworkPipe;

impl StreamingNetworkPipe {
    pub fn stream_http<F>(
        method: &str,
        url: &str,
        extra_headers: HashMap<String, String>,
        modules: &ModuleBus,
        max_bytes: usize,
        mut on_chunk: F,
    ) -> Result<StreamFetchOutcome, String>
    where
        F: FnMut(&[u8], bool),
    {
        let mut current_url = url.to_string();
        let mut redirects_left = 6u8;

        loop {
            let mut req = InterceptedRequest {
                method: method.to_string(),
                url: current_url.clone(),
                headers: extra_headers.clone(),
            };

            match modules.dispatch_before_request(&mut req) {
                ModuleDecision::Block { reason } => {
                    return Err(format!("Site isolation blocked request: {}", reason));
                }
                ModuleDecision::SynthesizeResponse(mut synth) => {
                    modules.dispatch_response_headers(&req, &mut synth);
                    if let Some(body) = &synth.synthetic_body {
                        on_chunk(body, true);
                    }
                    return Ok(StreamFetchOutcome {
                        final_url: req.url,
                        status_code: synth.status_code,
                        headers: synth.headers,
                        total_bytes_streamed: 0,
                    });
                }
                ModuleDecision::Proceed => {}
            }

            let parsed = ParsedUrl::parse(&req.url)?;
            let addr_str = format!("{}:{}", parsed.host, parsed.port);
            let socket_addr = addr_str
                .to_socket_addrs()
                .map_err(|e| format!("DNS resolution failed: {}", e))?
                .next()
                .ok_or_else(|| "No socket address resolved".to_string())?;

            let mut stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(6))
                .map_err(|e| format!("Connect error: {}", e))?;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(8)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

            let mut raw_request = format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept-Encoding: identity\r\n",
                req.method, parsed.path_and_query, parsed.host
            );
            for (k, v) in &req.headers {
                if !k.eq_ignore_ascii_case("Host") && !k.eq_ignore_ascii_case("Connection") {
                    raw_request.push_str(&format!("{}: {}\r\n", k, v));
                }
            }
            raw_request.push_str("\r\n");

            stream
                .write_all(raw_request.as_bytes())
                .map_err(|e| format!("Socket write error: {}", e))?;

            let mut buf = [0u8; STREAM_CHUNK_SIZE];
            let mut header_accum = Vec::with_capacity(1024);
            let mut headers_parsed = false;
            let mut is_chunked = false;
            let mut chunk_decoder = ChunkedTransferDecoder::new();
            let mut decoded_scratch = Vec::with_capacity(STREAM_CHUNK_SIZE);
            let mut total_body_bytes = 0usize;
            let mut redirect_target: Option<String> = None;

            let mut response_meta = InterceptedResponse {
                status_code: 200,
                headers: HashMap::new(),
                synthetic_body: None,
            };

            loop {
                let n = stream
                    .read(&mut buf)
                    .map_err(|e| format!("Stream read error: {}", e))?;
                if n == 0 {
                    on_chunk(&[], true);
                    break;
                }

                if !headers_parsed {
                    header_accum.extend_from_slice(&buf[..n]);
                    if let Some(split_pos) = find_double_crlf(&header_accum) {
                        let header_bytes = &header_accum[..split_pos];
                        let body_remainder = &header_accum[split_pos + 4..];

                        let (status, hdrs) = parse_http_headers(header_bytes);
                        response_meta.status_code = status;
                        response_meta.headers = hdrs;
                        modules.dispatch_response_headers(&req, &mut response_meta);
                        headers_parsed = true;

                        if matches!(status, 301 | 302 | 303 | 307 | 308) && redirects_left > 0 {
                            if let Some(loc) = response_meta
                                .headers
                                .iter()
                                .find(|(k, _)| k.eq_ignore_ascii_case("Location"))
                                .map(|(_, v)| v.clone())
                            {
                                redirect_target = Some(loc);
                                break;
                            }
                        }

                        is_chunked = response_meta
                            .headers
                            .iter()
                            .any(|(k, v)| {
                                k.eq_ignore_ascii_case("Transfer-Encoding")
                                    && v.to_ascii_lowercase().contains("chunked")
                            });

                        if !body_remainder.is_empty() {
                            if is_chunked {
                                decoded_scratch.clear();
                                chunk_decoder.decode_bytes(body_remainder, &mut decoded_scratch);
                                let allowed = decoded_scratch.len().min(max_bytes);
                                total_body_bytes += allowed;
                                on_chunk(&decoded_scratch[..allowed], false);
                            } else {
                                let allowed = body_remainder.len().min(max_bytes);
                                total_body_bytes += allowed;
                                on_chunk(&body_remainder[..allowed], false);
                            }
                        }
                        header_accum.clear();
                        header_accum.shrink_to_fit();
                    }
                } else {
                    let payload_slice: &[u8] = if is_chunked {
                        decoded_scratch.clear();
                        chunk_decoder.decode_bytes(&buf[..n], &mut decoded_scratch);
                        &decoded_scratch
                    } else {
                        &buf[..n]
                    };

                    if total_body_bytes + payload_slice.len() > max_bytes {
                        let remaining = max_bytes.saturating_sub(total_body_bytes);
                        if remaining > 0 {
                            total_body_bytes += remaining;
                            on_chunk(&payload_slice[..remaining], true);
                        }
                        break;
                    }
                    total_body_bytes += payload_slice.len();
                    if !payload_slice.is_empty() {
                        on_chunk(payload_slice, false);
                    }
                }
            }

            if let Some(next_loc) = redirect_target {
                redirects_left -= 1;
                current_url = if next_loc.starts_with("http://") || next_loc.starts_with("https://")
                {
                    next_loc
                } else {
                    format!("http://{}:{}{}", parsed.host, parsed.port, next_loc)
                };
                continue;
            }

            return Ok(StreamFetchOutcome {
                final_url: req.url,
                status_code: response_meta.status_code,
                headers: response_meta.headers,
                total_bytes_streamed: total_body_bytes,
            });
        }
    }
}

struct ParsedUrl {
    host: String,
    port: u16,
    path_and_query: String,
}

impl ParsedUrl {
    fn parse(url: &str) -> Result<Self, String> {
        let rest = if let Some(stripped) = url.strip_prefix("http://") {
            stripped
        } else if let Some(stripped) = url.strip_prefix("https://") {
            stripped
        } else {
            url
        };

        let (authority, path_and_query) = match rest.find('/') {
            Some(idx) => (&rest[..idx], rest[idx..].to_string()),
            None => (rest, "/".to_string()),
        };

        let (host, port) = match authority.find(':') {
            Some(idx) => {
                let p = authority[idx + 1..]
                    .parse::<u16>()
                    .map_err(|_| "Invalid port".to_string())?;
                (authority[..idx].to_string(), p)
            }
            None => (authority.to_string(), 80),
        };

        if host.is_empty() {
            return Err("Empty host in URL".to_string());
        }

        Ok(Self {
            host,
            port,
            path_and_query,
        })
    }
}

fn find_double_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_http_headers(raw: &[u8]) -> (u16, HashMap<String, String>) {
    let text = String::from_utf8_lossy(raw);
    let mut lines = text.lines();
    let status_line = lines.next().unwrap_or("HTTP/1.1 200 OK");
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(200);

    let mut headers = HashMap::new();
    for line in lines {
        if let Some(colon) = line.find(':') {
            let k = line[..colon].trim().to_string();
            let v = line[colon + 1..].trim().to_string();
            headers.insert(k, v);
        }
    }
    (status_code, headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunked_transfer_encoding_decoder() {
        let mut decoder = ChunkedTransferDecoder::new();
        let mut out = Vec::new();
        let raw_chunked = b"5\r\nManta\r\n7\r\n Chrome\r\n0\r\n\r\n";
        // Feed in small slices to verify cross-boundary state continuity
        for slice in raw_chunked.chunks(3) {
            decoder.decode_bytes(slice, &mut out);
        }
        assert_eq!(&out, b"Manta Chrome");
    }
}
