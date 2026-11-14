use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::cpp_bridge::CppTlsStream;
use crate::deflate::decompress_http_encoding;
use crate::module_bus::{InterceptedRequest, InterceptedResponse, ModuleBus, ModuleDecision};
use crate::site_isolation::{is_private_or_reserved_ip, OriginKey};

pub const STREAM_CHUNK_SIZE: usize = 4096;
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const KEEP_ALIVE_MAX_CONNS: usize = 8;
const KEEP_ALIVE_TTL_SECS: u64 = 15;

pub type MantaNetFetchCallback = extern "C" fn(
    method: *const c_char,
    url: *const c_char,
    req_headers_crlf: *const c_char,
    user_ctx: *mut c_void,
    on_response_meta: extern "C" fn(*mut c_void, u16, *const c_char),
    on_body_chunk: extern "C" fn(*mut c_void, *const u8, u32, bool),
) -> i32;

static HOST_NET_CALLBACK: OnceLock<Mutex<Option<MantaNetFetchCallback>>> = OnceLock::new();

pub fn register_host_net_transport(cb: Option<MantaNetFetchCallback>) {
    let lock = HOST_NET_CALLBACK.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = lock.lock() {
        *guard = cb;
    }
}

fn get_host_net_transport() -> Option<MantaNetFetchCallback> {
    let lock = HOST_NET_CALLBACK.get_or_init(|| Mutex::new(None));
    lock.lock().ok().and_then(|g| *g)
}

pub struct StreamFetchOutcome {
    pub final_url: String,
    pub status_code: u16,
    pub headers: HashMap<String, String>,
    pub total_bytes_streamed: usize,
}

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

    #[inline]
    pub fn is_done(&self) -> bool {
        self.done
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

enum TransportStream {
    Plain(TcpStream),
    Tls(CppTlsStream),
}

impl TransportStream {
    fn write_all(&mut self, data: &[u8]) -> Result<(), String> {
        match self {
            TransportStream::Plain(s) => {
                s.write_all(data).map_err(|e| format!("Socket write error: {}", e))
            }
            TransportStream::Tls(t) => t.write_all_tls(data).map_err(|e| e.to_string()),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        match self {
            TransportStream::Plain(s) => s.read(buf).map_err(|e| format!("Stream read error: {}", e)),
            TransportStream::Tls(t) => t.read_tls(buf).map_err(|e| e.to_string()),
        }
    }
}

struct PooledConn {
    key: String,
    stream: TransportStream,
    expires_at: Instant,
}

static KEEP_ALIVE_POOL: OnceLock<Mutex<Vec<PooledConn>>> = OnceLock::new();

fn pool_take(key: &str) -> Option<TransportStream> {
    let lock = KEEP_ALIVE_POOL.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(mut pool) = lock.lock() else {
        return None;
    };
    let now = Instant::now();
    pool.retain(|c| c.expires_at > now);
    if let Some(pos) = pool.iter().position(|c| c.key == key) {
        Some(pool.remove(pos).stream)
    } else {
        None
    }
}

fn pool_put(key: String, stream: TransportStream) {
    let lock = KEEP_ALIVE_POOL.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(mut pool) = lock.lock() else {
        return;
    };
    let now = Instant::now();
    pool.retain(|c| c.expires_at > now);
    if pool.len() >= KEEP_ALIVE_MAX_CONNS {
        pool.remove(0);
    }
    pool.push(PooledConn {
        key,
        stream,
        expires_at: now + Duration::from_secs(KEEP_ALIVE_TTL_SECS),
    });
}

/// Immediately drops and closes all idle pooled TCP/TLS connections (called on shred/wipe).
pub fn clear_keep_alive_pool() {
    if let Some(lock) = KEEP_ALIVE_POOL.get() {
        if let Ok(mut pool) = lock.lock() {
            pool.clear();
            pool.shrink_to_fit();
        }
    }
}

struct HostTransportBridgeState<'a> {
    status: u16,
    headers: HashMap<String, String>,
    compressed_buf: Vec<u8>,
    is_compressed: bool,
    encoding_val: String,
    total_bytes: usize,
    max_bytes: usize,
    on_chunk: &'a mut dyn FnMut(&[u8], bool),
}

extern "C" fn host_on_meta(ctx: *mut c_void, status: u16, headers_crlf: *const c_char) {
    if ctx.is_null() {
        return;
    }
    let state = unsafe { &mut *(ctx as *mut HostTransportBridgeState<'_>) };
    state.status = status;
    if !headers_crlf.is_null() {
        let raw = unsafe { CStr::from_ptr(headers_crlf).to_string_lossy() };
        for line in raw.lines() {
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim().to_string();
                let val = v.trim().to_string();
                if key.eq_ignore_ascii_case("Content-Encoding")
                    && (val.to_ascii_lowercase().contains("gzip")
                        || val.to_ascii_lowercase().contains("deflate"))
                {
                    state.is_compressed = true;
                    state.encoding_val = val.clone();
                }
                state.headers.insert(key, val);
            }
        }
    }
}

extern "C" fn host_on_chunk(ctx: *mut c_void, ptr: *const u8, len: u32, is_eof: bool) {
    if ctx.is_null() {
        return;
    }
    let state = unsafe { &mut *(ctx as *mut HostTransportBridgeState<'_>) };
    let slice = if ptr.is_null() || len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(ptr, len as usize) }
    };
    if state.is_compressed {
        let allowed = slice
            .len()
            .min(state.max_bytes.saturating_sub(state.compressed_buf.len()));
        state.compressed_buf.extend_from_slice(&slice[..allowed]);
        if is_eof && !state.compressed_buf.is_empty() {
            if let Ok(decoded) = decompress_http_encoding(
                &state.encoding_val,
                &state.compressed_buf,
                state.max_bytes,
            ) {
                state.total_bytes = decoded.len();
                (state.on_chunk)(&decoded, true);
            } else {
                (state.on_chunk)(&[], true);
            }
            state.compressed_buf.clear();
        }
    } else {
        let rem = state.max_bytes.saturating_sub(state.total_bytes);
        let take = slice.len().min(rem);
        state.total_bytes += take;
        (state.on_chunk)(&slice[..take], is_eof);
    }
}

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
        if has_crlf_or_nul(method) || has_crlf_or_nul(url) {
            return Err("Rejected request containing CRLF or NUL characters".to_string());
        }

        let mut current_method = method.trim().to_ascii_uppercase();
        let mut current_url = url.trim().to_string();
        let mut current_headers = sanitize_headers(extra_headers)?;
        let initial_origin = OriginKey::from_url(&current_url);
        let mut redirects_left = 6u8;

        loop {
            let mut req = InterceptedRequest {
                method: current_method.clone(),
                url: current_url.clone(),
                headers: current_headers.clone(),
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

            if let Some(host_cb) = get_host_net_transport() {
                let mut hdr_raw = String::new();
                for (k, v) in &req.headers {
                    hdr_raw.push_str(k);
                    hdr_raw.push_str(": ");
                    hdr_raw.push_str(v);
                    hdr_raw.push_str("\r\n");
                }
                let c_method = CString::new(req.method.as_str()).map_err(|_| "Invalid method")?;
                let c_url = CString::new(req.url.as_str()).map_err(|_| "Invalid URL")?;
                let c_hdrs = CString::new(hdr_raw).map_err(|_| "Invalid headers")?;

                let mut bridge_state = HostTransportBridgeState {
                    status: 200,
                    headers: HashMap::new(),
                    compressed_buf: Vec::new(),
                    is_compressed: false,
                    encoding_val: String::new(),
                    total_bytes: 0,
                    max_bytes,
                    on_chunk: &mut on_chunk,
                };
                let rc = host_cb(
                    c_method.as_ptr(),
                    c_url.as_ptr(),
                    c_hdrs.as_ptr(),
                    (&mut bridge_state as *mut HostTransportBridgeState<'_>) as *mut c_void,
                    host_on_meta,
                    host_on_chunk,
                );
                if rc != 0 {
                    return Err(format!("Host network transport error ({})", rc));
                }
                return Ok(StreamFetchOutcome {
                    final_url: req.url,
                    status_code: bridge_state.status,
                    headers: bridge_state.headers,
                    total_bytes_streamed: bridge_state.total_bytes,
                });
            }

            let parsed = ParsedUrl::parse(&req.url)?;
            let pool_key = format!(
                "{}:{}:{}",
                if parsed.is_https { "https" } else { "http" },
                parsed.host,
                parsed.port
            );

            let host_header = if (parsed.is_https && parsed.port == 443)
                || (!parsed.is_https && parsed.port == 80)
            {
                parsed.host.clone()
            } else {
                format!("{}:{}", parsed.host, parsed.port)
            };

            let mut raw_request = format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Manta/3.0\r\nConnection: keep-alive\r\nAccept-Encoding: gzip, deflate\r\n",
                req.method, parsed.path_and_query, host_header
            );
            for (k, v) in &req.headers {
                if !k.eq_ignore_ascii_case("Host")
                    && !k.eq_ignore_ascii_case("Connection")
                    && !k.eq_ignore_ascii_case("Accept-Encoding")
                {
                    raw_request.push_str(&format!("{}: {}\r\n", k, v));
                }
            }
            raw_request.push_str("\r\n");

            let mut attempt_pooled = true;
            let (response_meta, total_body_bytes, redirect_target) = loop {
                let (mut stream, was_pooled) = if attempt_pooled {
                    if let Some(s) = pool_take(&pool_key) {
                        (s, true)
                    } else {
                        attempt_pooled = false;
                        (open_fresh_transport(&parsed, &req, &initial_origin)?, false)
                    }
                } else {
                    (open_fresh_transport(&parsed, &req, &initial_origin)?, false)
                };

                if stream.write_all(raw_request.as_bytes()).is_err() {
                    if was_pooled {
                        attempt_pooled = false;
                        continue;
                    }
                    return Err("Socket write error".to_string());
                }

                let mut buf = [0u8; STREAM_CHUNK_SIZE];
                let mut header_accum = Vec::with_capacity(1024);
                let mut headers_parsed = false;
                let mut is_chunked = false;
                let mut expected_len: Option<usize> = None;
                let mut server_keep_alive = true;
                let mut raw_body_received = 0usize;
                let mut content_encoding = String::new();
                let mut compressed_accum = Vec::new();
                let mut chunk_decoder = ChunkedTransferDecoder::new();
                let mut decoded_scratch = Vec::with_capacity(STREAM_CHUNK_SIZE);
                let mut total_body = 0usize;
                let mut redir: Option<(u16, String)> = None;
                let mut body_complete_clean = false;
                let mut retry_fresh = false;

                let mut meta = InterceptedResponse {
                    status_code: 200,
                    headers: HashMap::new(),
                    synthetic_body: None,
                };

                loop {
                    let read_res = stream.read(&mut buf);
                    let n = match read_res {
                        Ok(0) => {
                            if !headers_parsed && was_pooled && header_accum.is_empty() {
                                retry_fresh = true;
                            }
                            break;
                        }
                        Ok(k) => k,
                        Err(e) => {
                            if !headers_parsed && was_pooled && header_accum.is_empty() {
                                retry_fresh = true;
                                break;
                            }
                            return Err(e);
                        }
                    };

                    if !headers_parsed {
                        if header_accum.len() + n > MAX_HTTP_HEADER_BYTES {
                            return Err("HTTP response headers exceeded 64 KB cap".to_string());
                        }
                        header_accum.extend_from_slice(&buf[..n]);
                        if let Some(split_pos) = find_double_crlf(&header_accum) {
                            let header_bytes = &header_accum[..split_pos];
                            let body_remainder = &header_accum[split_pos + 4..];

                            let (status, hdrs) = parse_http_headers(header_bytes);
                            meta.status_code = status;
                            meta.headers = hdrs;
                            modules.dispatch_response_headers(&req, &mut meta);
                            headers_parsed = true;

                            if matches!(status, 301 | 302 | 303 | 307 | 308) && redirects_left > 0
                            {
                                if let Some(loc) = meta
                                    .headers
                                    .iter()
                                    .find(|(k, _)| k.eq_ignore_ascii_case("Location"))
                                    .map(|(_, v)| v.clone())
                                {
                                    redir = Some((status, loc));
                                    break;
                                }
                            }

                            is_chunked = meta.headers.iter().any(|(k, v)| {
                                k.eq_ignore_ascii_case("Transfer-Encoding")
                                    && v.to_ascii_lowercase().contains("chunked")
                            });
                            expected_len = meta
                                .headers
                                .iter()
                                .find(|(k, _)| k.eq_ignore_ascii_case("Content-Length"))
                                .and_then(|(_, v)| v.trim().parse::<usize>().ok());
                            server_keep_alive = !meta.headers.iter().any(|(k, v)| {
                                k.eq_ignore_ascii_case("Connection")
                                    && v.to_ascii_lowercase().contains("close")
                            });
                            content_encoding = meta
                                .headers
                                .iter()
                                .find(|(k, _)| k.eq_ignore_ascii_case("Content-Encoding"))
                                .map(|(_, v)| v.to_ascii_lowercase())
                                .filter(|v| v.contains("gzip") || v.contains("deflate"))
                                .unwrap_or_default();

                            if matches!(status, 204 | 304) || req.method == "HEAD" {
                                body_complete_clean = true;
                                break;
                            }

                            if !body_remainder.is_empty() {
                                raw_body_received += body_remainder.len();
                                let payload: &[u8] = if is_chunked {
                                    decoded_scratch.clear();
                                    chunk_decoder
                                        .decode_bytes(body_remainder, &mut decoded_scratch);
                                    &decoded_scratch
                                } else {
                                    body_remainder
                                };
                                if !content_encoding.is_empty() {
                                    if compressed_accum.len() + payload.len() > max_bytes {
                                        return Err(
                                            "Compressed stream exceeded max allowed bytes"
                                                .to_string(),
                                        );
                                    }
                                    compressed_accum.extend_from_slice(payload);
                                } else {
                                    let allowed = payload.len().min(max_bytes);
                                    total_body += allowed;
                                    on_chunk(&payload[..allowed], false);
                                }
                            }

                            header_accum.clear();
                            header_accum.shrink_to_fit();

                            if (is_chunked && chunk_decoder.is_done())
                                || expected_len
                                    .map(|len| raw_body_received >= len)
                                    .unwrap_or(false)
                            {
                                body_complete_clean = true;
                                break;
                            }
                        }
                    } else {
                        raw_body_received += n;
                        let payload_slice: &[u8] = if is_chunked {
                            decoded_scratch.clear();
                            chunk_decoder.decode_bytes(&buf[..n], &mut decoded_scratch);
                            &decoded_scratch
                        } else {
                            &buf[..n]
                        };

                        if !content_encoding.is_empty() {
                            if compressed_accum.len() + payload_slice.len() > max_bytes {
                                return Err(
                                    "Compressed stream exceeded max allowed bytes".to_string(),
                                );
                            }
                            compressed_accum.extend_from_slice(payload_slice);
                        } else {
                            if total_body + payload_slice.len() > max_bytes {
                                let remaining = max_bytes.saturating_sub(total_body);
                                if remaining > 0 {
                                    total_body += remaining;
                                    on_chunk(&payload_slice[..remaining], true);
                                }
                                break;
                            }
                            total_body += payload_slice.len();
                            if !payload_slice.is_empty() {
                                on_chunk(payload_slice, false);
                            }
                        }

                        if (is_chunked && chunk_decoder.is_done())
                            || expected_len
                                .map(|len| raw_body_received >= len)
                                .unwrap_or(false)
                        {
                            body_complete_clean = true;
                            break;
                        }
                    }
                }

                if retry_fresh {
                    attempt_pooled = false;
                    continue;
                }

                if redir.is_none() {
                    if !content_encoding.is_empty() && !compressed_accum.is_empty() {
                        let dec = decompress_http_encoding(
                            &content_encoding,
                            &compressed_accum,
                            max_bytes,
                        )?;
                        total_body = dec.len();
                        on_chunk(&dec, true);
                    } else {
                        on_chunk(&[], true);
                    }

                    if body_complete_clean && server_keep_alive {
                        pool_put(pool_key.clone(), stream);
                    }
                }

                break (meta, total_body, redir);
            };

            if let Some((status, next_loc)) = redirect_target {
                if has_crlf_or_nul(&next_loc) {
                    return Err("Rejected redirect with CRLF/NUL".to_string());
                }
                redirects_left -= 1;
                let resolved = if next_loc.starts_with("http://")
                    || next_loc.starts_with("https://")
                {
                    next_loc
                } else if next_loc.starts_with("//") {
                    format!(
                        "{}:{}",
                        if parsed.is_https { "https" } else { "http" },
                        next_loc
                    )
                } else if next_loc.starts_with('/') {
                    format!(
                        "{}://{}:{}{}",
                        if parsed.is_https { "https" } else { "http" },
                        parsed.host,
                        parsed.port,
                        next_loc
                    )
                } else {
                    format!(
                        "{}://{}:{}/{}",
                        if parsed.is_https { "https" } else { "http" },
                        parsed.host,
                        parsed.port,
                        next_loc
                    )
                };

                let prev_origin = OriginKey::from_url(&current_url);
                let next_origin = OriginKey::from_url(&resolved);
                if !prev_origin.is_same_origin(&next_origin) {
                    current_headers.retain(|k, _| {
                        !k.eq_ignore_ascii_case("Cookie")
                            && !k.eq_ignore_ascii_case("Authorization")
                            && !k.eq_ignore_ascii_case("Proxy-Authorization")
                    });
                }

                if matches!(status, 301 | 302 | 303) && current_method == "POST" {
                    current_method = "GET".to_string();
                }

                current_url = resolved;
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

fn open_fresh_transport(
    parsed: &ParsedUrl,
    req: &InterceptedRequest,
    initial_origin: &OriginKey,
) -> Result<TransportStream, String> {
    let addr_str = format!("{}:{}", parsed.host, parsed.port);
    let addrs: Vec<SocketAddr> = addr_str
        .to_socket_addrs()
        .map_err(|e| format!("DNS resolution failed: {}", e))?
        .collect();

    if addrs.is_empty() {
        return Err("No socket address resolved".to_string());
    }

    let initiator_is_public = req
        .headers
        .get("Origin")
        .or_else(|| req.headers.get("Referer"))
        .map(|s| !OriginKey::from_url(s).is_private_or_loopback_network())
        .unwrap_or(!initial_origin.is_private_or_loopback_network());

    let safe_addr = addrs
        .into_iter()
        .find(|a| {
            if initiator_is_public {
                !is_private_or_reserved_ip(a.ip())
            } else {
                true
            }
        })
        .ok_or_else(|| {
            "Blocked resolved private/loopback/metadata IP (DNS rebinding SSRF guard)".to_string()
        })?;

    let tcp = TcpStream::connect_timeout(&safe_addr, Duration::from_secs(6))
        .map_err(|e| format!("Connect error: {}", e))?;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(5)));

    if parsed.is_https {
        let tls = CppTlsStream::connect(tcp, &parsed.host)
            .map_err(|e| format!("HTTPS TLS error: {}", e))?;
        Ok(TransportStream::Tls(tls))
    } else {
        Ok(TransportStream::Plain(tcp))
    }
}

struct ParsedUrl {
    is_https: bool,
    host: String,
    port: u16,
    path_and_query: String,
}

impl ParsedUrl {
    fn parse(url: &str) -> Result<Self, String> {
        let (is_https, rest) = if let Some(stripped) = url.strip_prefix("https://") {
            (true, stripped)
        } else if let Some(stripped) = url.strip_prefix("http://") {
            (false, stripped)
        } else {
            return Err("Only http:// and https:// network URLs are permitted".to_string());
        };

        let default_port = if is_https { 443 } else { 80 };
        let path_split = rest.find(['/', '?', '#']);
        let (authority, raw_tail) = match path_split {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, "/"),
        };

        let no_frag = raw_tail.split('#').next().unwrap_or("/");
        let path_and_query = if no_frag.starts_with('/') {
            no_frag.to_string()
        } else {
            format!("/{}", no_frag)
        };

        let host_port = authority.rsplit('@').next().unwrap_or(authority);
        let (host, port) = if host_port.starts_with('[') {
            if let Some(close) = host_port.find(']') {
                let h = host_port[1..close].to_string();
                let p = host_port[close + 1..]
                    .strip_prefix(':')
                    .and_then(|s| s.parse::<u16>().ok())
                    .unwrap_or(default_port);
                (h, p)
            } else {
                return Err("Malformed IPv6 host in URL".to_string());
            }
        } else if let Some(idx) = host_port.rfind(':') {
            let p = host_port[idx + 1..]
                .parse::<u16>()
                .map_err(|_| "Invalid port".to_string())?;
            (host_port[..idx].to_string(), p)
        } else {
            (host_port.to_string(), default_port)
        };

        if host.is_empty() || has_crlf_or_nul(&host) || has_crlf_or_nul(&path_and_query) {
            return Err("Invalid host or path in URL".to_string());
        }

        Ok(Self {
            is_https,
            host,
            port,
            path_and_query,
        })
    }
}

fn has_crlf_or_nul(s: &str) -> bool {
    s.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
}

fn sanitize_headers(headers: HashMap<String, String>) -> Result<HashMap<String, String>, String> {
    let mut clean = HashMap::with_capacity(headers.len());
    for (k, v) in headers {
        if has_crlf_or_nul(&k) || has_crlf_or_nul(&v) {
            return Err("Rejected header containing CRLF or NUL characters".to_string());
        }
        clean.insert(k, v);
    }
    Ok(clean)
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
    fn test_parsed_url_https_port_443_and_crlf_rejection() {
        let p = ParsedUrl::parse("https://example.com/api?v=1#frag").unwrap();
        assert!(p.is_https);
        assert_eq!(p.host, "example.com");
        assert_eq!(p.port, 443);
        assert_eq!(p.path_and_query, "/api?v=1");

        assert!(ParsedUrl::parse("https://example.com/evil\r\nInjected: 1").is_err());

        let mut bad_hdrs = HashMap::new();
        bad_hdrs.insert("X-Test".to_string(), "val\r\nCookie: steal".to_string());
        assert!(sanitize_headers(bad_hdrs).is_err());
    }
}
