use std::collections::HashMap;

use crate::cpp_bridge::CppMediaRing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamProtocol {
    Progressive,
    Hls,
    Dash,
}

impl StreamProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            StreamProtocol::Progressive => "progressive",
            StreamProtocol::Hls => "hls",
            StreamProtocol::Dash => "dash",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPlaybackState {
    Idle,
    Buffering,
    Playing,
    Paused,
    Ended,
    Error,
}

impl StreamPlaybackState {
    pub fn as_str(&self) -> &'static str {
        match self {
            StreamPlaybackState::Idle => "idle",
            StreamPlaybackState::Buffering => "buffering",
            StreamPlaybackState::Playing => "playing",
            StreamPlaybackState::Paused => "paused",
            StreamPlaybackState::Ended => "ended",
            StreamPlaybackState::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MediaVariant {
    pub index: usize,
    pub bandwidth_bps: u64,
    pub width: u32,
    pub height: u32,
    pub codecs: String,
    pub uri: String,
}

#[derive(Debug, Clone)]
pub struct MediaSegment {
    pub index: usize,
    pub uri: String,
    pub duration_ms: u64,
    pub start_ms: u64,
    pub byte_range: Option<(u64, u64)>, // (offset, length)
    pub is_init: bool,
    pub is_low_latency_part: bool,
}

/// EWMA + Buffer-Occupancy Adaptive Bitrate Controller tuned for <150ms cold-start TTFF
/// and zero-stall live/VOD streaming.
#[derive(Debug, Clone)]
pub struct AbrController {
    pub fast_ewma_bps: f64,
    pub slow_ewma_bps: f64,
    pub last_rtt_ms: u32,
    pub samples_count: u32,
    pub active_variant_idx: usize,
}

impl Default for AbrController {
    fn default() -> Self {
        Self {
            fast_ewma_bps: 0.0,
            slow_ewma_bps: 0.0,
            last_rtt_ms: 0,
            samples_count: 0,
            active_variant_idx: 0,
        }
    }
}

impl AbrController {
    pub fn record_download(&mut self, bytes: usize, duration_ms: u64, rtt_ms: u32) {
        let safe_ms = duration_ms.max(1) as f64;
        let sample_bps = (bytes as f64 * 8.0 * 1000.0) / safe_ms;
        if self.samples_count == 0 {
            self.fast_ewma_bps = sample_bps;
            self.slow_ewma_bps = sample_bps;
        } else {
            self.fast_ewma_bps = 0.35 * sample_bps + 0.65 * self.fast_ewma_bps;
            self.slow_ewma_bps = 0.12 * sample_bps + 0.88 * self.slow_ewma_bps;
        }
        self.last_rtt_ms = rtt_ms;
        self.samples_count += 1;
    }

    pub fn estimated_bandwidth_bps(&self) -> u64 {
        if self.samples_count == 0 {
            return 2_500_000; // Initial optimistic 2.5 Mbps estimate after first fast-start segment
        }
        self.fast_ewma_bps.min(self.slow_ewma_bps).max(150_000.0) as u64
    }

    pub fn select_variant(&mut self, variants: &[MediaVariant], buffered_ms: u64) -> usize {
        if variants.is_empty() {
            return 0;
        }
        // Fast cold-start: use lowest bandwidth variant on segment 0 for instant startup
        if self.samples_count == 0 {
            let lowest = variants
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| v.bandwidth_bps)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.active_variant_idx = lowest;
            return lowest;
        }

        // Safety headroom: 80% normal, 55% if buffer drops below 1.5s low-watermark
        let safety = if buffered_ms < 1500 { 0.55 } else { 0.80 };
        let budget = (self.estimated_bandwidth_bps() as f64 * safety) as u64;

        let mut best_idx = 0usize;
        let mut best_bw = 0u64;
        let mut min_idx = 0usize;
        let mut min_bw = u64::MAX;

        for (i, v) in variants.iter().enumerate() {
            if v.bandwidth_bps < min_bw {
                min_bw = v.bandwidth_bps;
                min_idx = i;
            }
            if v.bandwidth_bps <= budget && v.bandwidth_bps >= best_bw {
                best_bw = v.bandwidth_bps;
                best_idx = i;
            }
        }

        self.active_variant_idx = if best_bw > 0 { best_idx } else { min_idx };
        self.active_variant_idx
    }
}

/// Zero-cache media streaming session backed by a C++ lock-free SPSC ring buffer.
pub struct MediaStreamSession {
    pub id: u32,
    pub source_url: String,
    pub protocol: StreamProtocol,
    pub state: StreamPlaybackState,
    pub is_live: bool,
    pub target_duration_ms: u64,
    pub total_duration_ms: u64,
    pub current_time_ms: u64,
    pub buffered_ms: u64,
    pub ttff_ms: Option<u32>,
    pub dropped_frames: u32,
    pub av_drift_ms: i32,
    pub total_bytes_streamed: u64,
    pub next_segment_cursor: usize,
    pub variants: Vec<MediaVariant>,
    pub segments: Vec<MediaSegment>,
    pub abr: AbrController,
    pub ring: CppMediaRing,
}

/// **Manta Low-Latency Media Streaming Manager (`media_stream.rs`)**:
/// - Streams progressive MP4/WebM (via HTTP Range), Low-Latency HLS (`.m3u8` + `#EXT-X-PART`),
///   and MPEG-DASH (`.mpd`) directly through C++ lock-free SPSC ring buffers (`mmap`-backed).
/// - Never caches video/audio bytes permanently to disk.
pub struct MantaMediaManager {
    sessions: HashMap<u32, MediaStreamSession>,
    next_id: u32,
    default_ring_bytes_pow2: u32,
    max_concurrent_streams: usize,
}

impl Default for MantaMediaManager {
    fn default() -> Self {
        Self::new(4 * 1024 * 1024)
    }
}

impl MantaMediaManager {
    pub fn new(default_ring_bytes_pow2: u32) -> Self {
        Self {
            sessions: HashMap::new(),
            next_id: 1,
            default_ring_bytes_pow2: default_ring_bytes_pow2.max(64 * 1024),
            max_concurrent_streams: 8,
        }
    }

    pub fn open_stream(&mut self, source_url: &str, ring_bytes_pow2: u32) -> Result<u32, &'static str> {
        if self.sessions.len() >= self.max_concurrent_streams {
            if let Some(oldest_idle) = self
                .sessions
                .iter()
                .find(|(_, s)| {
                    matches!(
                        s.state,
                        StreamPlaybackState::Idle | StreamPlaybackState::Ended | StreamPlaybackState::Error
                    )
                })
                .map(|(&id, _)| id)
            {
                self.sessions.remove(&oldest_idle);
            } else {
                return Err("Maximum active media streams reached");
            }
        }

        let cap = if ring_bytes_pow2 == 0 {
            self.default_ring_bytes_pow2
        } else {
            ring_bytes_pow2.clamp(64 * 1024, 16 * 1024 * 1024)
        };
        let ring = CppMediaRing::new(cap).ok_or("Failed allocating C++ media ring buffer")?;

        let id = self.next_id;
        self.next_id += 1;

        let lower = source_url.to_ascii_lowercase();
        let protocol = if lower.contains(".m3u8") {
            StreamProtocol::Hls
        } else if lower.contains(".mpd") {
            StreamProtocol::Dash
        } else {
            StreamProtocol::Progressive
        };

        self.sessions.insert(
            id,
            MediaStreamSession {
                id,
                source_url: source_url.to_string(),
                protocol,
                state: StreamPlaybackState::Buffering,
                is_live: false,
                target_duration_ms: 4000,
                total_duration_ms: 0,
                current_time_ms: 0,
                buffered_ms: 0,
                ttff_ms: None,
                dropped_frames: 0,
                av_drift_ms: 0,
                total_bytes_streamed: 0,
                next_segment_cursor: 0,
                variants: Vec::new(),
                segments: Vec::new(),
                abr: AbrController::default(),
                ring,
            },
        );
        Ok(id)
    }

    pub fn ingest_manifest(&mut self, stream_id: u32, base_url: &str, manifest_text: &str) -> bool {
        let Some(sess) = self.sessions.get_mut(&stream_id) else {
            return false;
        };
        let trimmed = manifest_text.trim();
        if trimmed.starts_with("#EXTM3U") {
            sess.protocol = StreamProtocol::Hls;
            parse_hls_manifest(sess, base_url, trimmed);
            true
        } else if trimmed.contains("<MPD") {
            sess.protocol = StreamProtocol::Dash;
            parse_dash_manifest(sess, base_url, trimmed);
            true
        } else {
            false
        }
    }

    /// Returns the next `(segment_index, resolved_url, optional_range_header)` to prefetch.
    pub fn next_prefetch_request(&mut self, stream_id: u32) -> Option<(usize, String, Option<String>)> {
        let sess = self.sessions.get_mut(&stream_id)?;
        if sess.protocol == StreamProtocol::Progressive {
            if sess.next_segment_cursor == 0 {
                sess.next_segment_cursor = 1;
                return Some((0, sess.source_url.clone(), None));
            }
            return None;
        }

        if !sess.variants.is_empty() && sess.segments.is_empty() {
            let v_idx = sess.abr.select_variant(&sess.variants, sess.buffered_ms);
            let variant_url = sess.variants[v_idx].uri.clone();
            return Some((0, variant_url, None));
        }

        let idx = sess.next_segment_cursor;
        let seg = sess.segments.get(idx)?.clone();
        sess.next_segment_cursor += 1;
        let range_hdr = seg
            .byte_range
            .map(|(off, len)| format!("bytes={}-{}", off, off + len.saturating_sub(1)));
        Some((seg.index, seg.uri, range_hdr))
    }

    /// Feeds network media bytes directly into the lock-free C++ ring buffer and updates ABR telemetry.
    pub fn feed_bytes(
        &mut self,
        stream_id: u32,
        bytes: &[u8],
        download_ms: u64,
        rtt_ms: u32,
        segment_duration_ms: u64,
    ) -> usize {
        let Some(sess) = self.sessions.get_mut(&stream_id) else {
            return 0;
        };
        let written = sess.ring.write(bytes);
        sess.total_bytes_streamed += written as u64;
        if written > 0 {
            if sess.ttff_ms.is_none() {
                sess.ttff_ms = Some((download_ms as u32).max(1));
            }
            sess.abr.record_download(written, download_ms, rtt_ms);
            sess.buffered_ms += segment_duration_ms;
            sess.state = StreamPlaybackState::Playing;
            if !sess.variants.is_empty() {
                sess.abr.select_variant(&sess.variants, sess.buffered_ms);
            }
        }
        written
    }

    pub fn read_playback_chunk(&mut self, stream_id: u32, out: &mut [u8]) -> usize {
        let Some(sess) = self.sessions.get_mut(&stream_id) else {
            return 0;
        };
        let n = sess.ring.read(out);
        if n > 0 && sess.ring.available_read() == 0 && sess.next_segment_cursor >= sess.segments.len() && !sess.is_live {
            sess.state = StreamPlaybackState::Ended;
        }
        n
    }

    /// Flushes the ring buffer immediately on seek and snaps to the target segment boundary.
    pub fn seek_to_ms(&mut self, stream_id: u32, target_ms: u64) -> bool {
        let Some(sess) = self.sessions.get_mut(&stream_id) else {
            return false;
        };
        sess.ring.reset();
        sess.current_time_ms = target_ms;
        sess.buffered_ms = 0;
        sess.state = StreamPlaybackState::Buffering;

        if !sess.segments.is_empty() {
            let pos = sess
                .segments
                .partition_point(|s| s.start_ms + s.duration_ms <= target_ms);
            sess.next_segment_cursor = pos.min(sess.segments.len().saturating_sub(1));
        } else {
            sess.next_segment_cursor = 0;
        }
        true
    }

    pub fn update_av_sync(&mut self, stream_id: u32, playback_ms: u64, drift_ms: i32, dropped_frames: u32) {
        if let Some(sess) = self.sessions.get_mut(&stream_id) {
            sess.current_time_ms = playback_ms;
            sess.av_drift_ms = drift_ms;
            sess.dropped_frames = sess.dropped_frames.saturating_add(dropped_frames);
            if sess.buffered_ms > 500 {
                sess.buffered_ms = sess.buffered_ms.saturating_sub(100);
            }
        }
    }

    pub fn get_stats_json(&self, stream_id: u32) -> String {
        let Some(sess) = self.sessions.get(&stream_id) else {
            return String::from("{}");
        };
        let active_v = sess.variants.get(sess.abr.active_variant_idx);
        let bw = active_v
            .map(|v| v.bandwidth_bps)
            .unwrap_or_else(|| sess.abr.estimated_bandwidth_bps());
        let width = active_v.map(|v| v.width).unwrap_or(0);
        let height = active_v.map(|v| v.height).unwrap_or(0);

        format!(
            concat!(
                "{{",
                "\"stream_id\":{},",
                "\"protocol\":\"{}\",",
                "\"state\":\"{}\",",
                "\"is_live\":{},",
                "\"active_bitrate_bps\":{},",
                "\"estimated_bw_bps\":{},",
                "\"resolution\":\"{}x{}\",",
                "\"current_time_ms\":{},",
                "\"total_duration_ms\":{},",
                "\"buffered_ms\":{},",
                "\"ttff_ms\":{},",
                "\"av_drift_ms\":{},",
                "\"dropped_frames\":{},",
                "\"ring_used_bytes\":{},",
                "\"ring_capacity_bytes\":{},",
                "\"segments_count\":{}",
                "}}"
            ),
            sess.id,
            sess.protocol.as_str(),
            sess.state.as_str(),
            sess.is_live,
            bw,
            sess.abr.estimated_bandwidth_bps(),
            width,
            height,
            sess.current_time_ms,
            sess.total_duration_ms,
            sess.buffered_ms,
            sess.ttff_ms.unwrap_or(0),
            sess.av_drift_ms,
            sess.dropped_frames,
            sess.ring.available_read(),
            sess.ring.capacity(),
            sess.segments.len()
        )
    }

    pub fn close_stream(&mut self, stream_id: u32) -> bool {
        self.sessions.remove(&stream_id).is_some()
    }

    pub fn clear_all(&mut self) {
        self.sessions.clear();
    }

    pub fn active_stream_count(&self) -> usize {
        self.sessions.len()
    }
}

fn parse_hls_manifest(sess: &mut MediaStreamSession, base_url: &str, text: &str) {
    sess.variants.clear();
    sess.segments.clear();
    sess.is_live = true;

    let mut pending_bw = 0u64;
    let mut pending_res = (0u32, 0u32);
    let mut pending_codecs = String::new();
    let mut expecting_variant_uri = false;

    let mut pending_dur_ms = 0u64;
    let mut pending_range: Option<(u64, u64)> = None;
    let mut expecting_seg_uri = false;
    let mut cum_ms = 0u64;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "#EXT-X-ENDLIST" {
            sess.is_live = false;
        } else if let Some(td) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            if let Ok(sec) = td.trim().parse::<f64>() {
                sess.target_duration_ms = (sec * 1000.0) as u64;
            }
        } else if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending_bw = extract_attr_val(attrs, "BANDWIDTH")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(500_000);
            if let Some(res) = extract_attr_val(attrs, "RESOLUTION") {
                if let Some((w, h)) = res.split_once('x') {
                    pending_res = (w.parse().unwrap_or(0), h.parse().unwrap_or(0));
                }
            }
            pending_codecs = extract_attr_val(attrs, "CODECS")
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            expecting_variant_uri = true;
        } else if let Some(attrs) = line.strip_prefix("#EXT-X-MAP:") {
            if let Some(uri) = extract_attr_val(attrs, "URI") {
                let resolved = resolve_relative_url(base_url, uri.trim_matches('"'));
                let idx = sess.segments.len();
                sess.segments.push(MediaSegment {
                    index: idx,
                    uri: resolved,
                    duration_ms: 0,
                    start_ms: cum_ms,
                    byte_range: None,
                    is_init: true,
                    is_low_latency_part: false,
                });
            }
        } else if let Some(attrs) = line.strip_prefix("#EXT-X-PART:") {
            if let Some(uri) = extract_attr_val(attrs, "URI") {
                let dur = extract_attr_val(attrs, "DURATION")
                    .and_then(|s| s.parse::<f64>().ok())
                    .map(|s| (s * 1000.0) as u64)
                    .unwrap_or(500);
                let resolved = resolve_relative_url(base_url, uri.trim_matches('"'));
                let idx = sess.segments.len();
                sess.segments.push(MediaSegment {
                    index: idx,
                    uri: resolved,
                    duration_ms: dur,
                    start_ms: cum_ms,
                    byte_range: None,
                    is_init: false,
                    is_low_latency_part: true,
                });
                cum_ms += dur;
            }
        } else if let Some(inf) = line.strip_prefix("#EXTINF:") {
            let dur_str = inf.split(',').next().unwrap_or("0").trim();
            pending_dur_ms = dur_str
                .parse::<f64>()
                .map(|s| (s * 1000.0) as u64)
                .unwrap_or(2000);
            expecting_seg_uri = true;
        } else if let Some(rng) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            pending_range = parse_byterange(rng);
        } else if !line.starts_with('#') {
            let resolved = resolve_relative_url(base_url, line);
            if expecting_variant_uri {
                let idx = sess.variants.len();
                sess.variants.push(MediaVariant {
                    index: idx,
                    bandwidth_bps: pending_bw,
                    width: pending_res.0,
                    height: pending_res.1,
                    codecs: std::mem::take(&mut pending_codecs),
                    uri: resolved,
                });
                expecting_variant_uri = false;
            } else if expecting_seg_uri {
                let idx = sess.segments.len();
                sess.segments.push(MediaSegment {
                    index: idx,
                    uri: resolved,
                    duration_ms: pending_dur_ms,
                    start_ms: cum_ms,
                    byte_range: pending_range.take(),
                    is_init: false,
                    is_low_latency_part: false,
                });
                cum_ms += pending_dur_ms;
                expecting_seg_uri = false;
            }
        }
    }
    sess.total_duration_ms = cum_ms;
    sess.next_segment_cursor = 0;
}

fn parse_dash_manifest(sess: &mut MediaStreamSession, base_url: &str, xml: &str) {
    sess.variants.clear();
    sess.segments.clear();
    sess.is_live = xml.contains("type=\"dynamic\"");

    for part in xml.split("<Representation").skip(1) {
        let header = part.split('>').next().unwrap_or("");
        let bw = extract_xml_attr(header, "bandwidth")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(800_000);
        let w = extract_xml_attr(header, "width")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let h = extract_xml_attr(header, "height")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let codecs = extract_xml_attr(header, "codecs").unwrap_or_default();

        let seg_uri = if let Some(b_start) = part.find("<BaseURL>") {
            let rest = &part[b_start + 9..];
            rest.split("</BaseURL>").next().unwrap_or("").trim().to_string()
        } else if let Some(media) = extract_xml_attr(part, "media") {
            media
        } else {
            String::new()
        };

        if !seg_uri.is_empty() {
            let resolved = resolve_relative_url(base_url, &seg_uri);
            let idx = sess.variants.len();
            sess.variants.push(MediaVariant {
                index: idx,
                bandwidth_bps: bw,
                width: w,
                height: h,
                codecs,
                uri: resolved,
            });
        }
    }
    sess.next_segment_cursor = 0;
}

fn extract_attr_val<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = attrs;
    while !rest.is_empty() {
        let trimmed = rest.trim_start_matches([',', ' ']);
        if let Some(eq) = trimmed.find('=') {
            let k = trimmed[..eq].trim();
            let after_eq = &trimmed[eq + 1..];
            if after_eq.starts_with('"') {
                if let Some(end_q) = after_eq[1..].find('"') {
                    let v = &after_eq[1..1 + end_q];
                    if k.eq_ignore_ascii_case(key) {
                        return Some(&after_eq[..end_q + 2]);
                    }
                    rest = &after_eq[end_q + 2..];
                    let _ = v;
                    continue;
                }
            }
            let end_comma = after_eq.find(',').unwrap_or(after_eq.len());
            let v = after_eq[..end_comma].trim();
            if k.eq_ignore_ascii_case(key) {
                return Some(v);
            }
            rest = &after_eq[end_comma..];
        } else {
            break;
        }
    }
    None
}

fn extract_xml_attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{}=\"", name);
    let idx = tag.find(&needle)?;
    let rest = &tag[idx + needle.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn parse_byterange(raw: &str) -> Option<(u64, u64)> {
    let clean = raw.trim();
    if let Some((len_s, off_s)) = clean.split_once('@') {
        let len = len_s.parse::<u64>().ok()?;
        let off = off_s.parse::<u64>().ok()?;
        Some((off, len))
    } else {
        let len = clean.parse::<u64>().ok()?;
        Some((0, len))
    }
}

pub fn resolve_relative_url(base: &str, rel: &str) -> String {
    let r = rel.trim();
    if r.starts_with("http://") || r.starts_with("https://") || r.starts_with("manta://") {
        return r.to_string();
    }
    let clean_base = base.split('?').next().unwrap_or(base);
    if r.starts_with("//") {
        let scheme = base.split("://").next().unwrap_or("https");
        return format!("{}:{}", scheme, r);
    }
    if r.starts_with('/') {
        if let Some(scheme_pos) = clean_base.find("://") {
            let after = &clean_base[scheme_pos + 3..];
            let host_end = after.find('/').unwrap_or(after.len());
            return format!("{}{}", &clean_base[..scheme_pos + 3 + host_end], r);
        }
    }
    match clean_base.rfind('/') {
        Some(slash) if slash > 7 => format!("{}/{}", &clean_base[..slash], r),
        _ => format!("{}/{}", clean_base.trim_end_matches('/'), r),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hls_ll_manifest_abr_and_seek_zero_cache() {
        let mut mgr = MantaMediaManager::new(128 * 1024);
        let sid = mgr
            .open_stream("https://cdn.manta.dev/live/master.m3u8", 0)
            .unwrap();

        let master = r#"#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=600000,RESOLUTION=640x360,CODECS="avc1.42e01e"
low/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=4500000,RESOLUTION=1920x1080,CODECS="avc1.640028"
hd/index.m3u8
"#;
        assert!(mgr.ingest_manifest(sid, "https://cdn.manta.dev/live/master.m3u8", master));
        let (_, variant_url, _) = mgr.next_prefetch_request(sid).unwrap();
        // Cold start picks lowest-bitrate variant for instant <150ms startup
        assert_eq!(variant_url, "https://cdn.manta.dev/live/low/index.m3u8");

        // Feed child playlist with LL-HLS parts & segments
        let playlist = r#"#EXTM3U
#EXT-X-TARGETDURATION:2
#EXTINF:2.0,
seg0.ts
#EXTINF:2.0,
seg1.ts
#EXT-X-ENDLIST
"#;
        assert!(mgr.ingest_manifest(sid, &variant_url, playlist));
        let (_, s0_url, _) = mgr.next_prefetch_request(sid).unwrap();
        assert_eq!(s0_url, "https://cdn.manta.dev/live/low/seg0.ts");

        // Feed video bytes into C++ SPSC ring
        let written = mgr.feed_bytes(sid, b"TS_PKT_DATA_001", 2, 10, 2000);
        assert_eq!(written, 15);
        let mut out = [0u8; 32];
        assert_eq!(mgr.read_playback_chunk(sid, &mut out), 15);
        assert_eq!(&out[..15], b"TS_PKT_DATA_001");

        // Seek resets ring cleanly without draining stale bytes
        assert!(mgr.seek_to_ms(sid, 2500));
        assert_eq!(mgr.read_playback_chunk(sid, &mut out), 0);
        assert!(mgr.close_stream(sid));
    }
}
