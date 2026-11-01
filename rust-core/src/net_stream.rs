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

/// Pure-Rust zero-cache HTTP/1.1 streaming client for local dev servers (`http://localhost:...`,
/// `http://192.168...`, `http://...`) and chunked byte-pipe receiver for TLS streams from JNI.
pub struct StreamingNetworkPipe;

impl StreamingNetworkPipe {
    /// Streams an HTTP URL in 4 KB chunks directly into the `on_chunk` callback without
    /// ever buffering the full response on disk or in a giant allocation.
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
        let mut req = InterceptedRequest {
            method: method.to_string(),
            url: url.to_string(),
            headers: extra_headers,
            is_cors_preflight: method.eq_ignore_ascii_case("OPTIONS"),
        };

        match modules.dispatch_before_request(&mut req) {
            ModuleDecision::Block { reason } => {
                return Err(format!("Shield blocked request: {}", reason));
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
        let mut total_body_bytes = 0usize;
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

                    if !body_remainder.is_empty() {
                        let allowed = body_remainder.len().min(max_bytes);
                        total_body_bytes += allowed;
                        on_chunk(&body_remainder[..allowed], false);
                    }
                    header_accum.clear();
                    header_accum.shrink_to_fit();
                }
            } else {
                if total_body_bytes + n > max_bytes {
                    let remaining = max_bytes.saturating_sub(total_body_bytes);
                    if remaining > 0 {
                        total_body_bytes += remaining;
                        on_chunk(&buf[..remaining], true);
                    }
                    break;
                }
                total_body_bytes += n;
                on_chunk(&buf[..n], false);
            }
        }

        Ok(StreamFetchOutcome {
            final_url: req.url,
            status_code: response_meta.status_code,
            headers: response_meta.headers,
            total_bytes_streamed: total_body_bytes,
        })
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
