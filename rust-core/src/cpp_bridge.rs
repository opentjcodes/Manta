use std::ffi::{c_char, c_void, CString};
use std::net::TcpStream;

#[cfg(unix)]
use std::os::fd::AsRawFd;

#[repr(C)]
pub struct MantaRingBuffer {
    _opaque: [u8; 0],
}

extern "C" {
    fn manta_cpp_secure_zero(ptr: *mut c_void, len: usize);
    fn manta_cpp_arena_alloc(bytes: usize) -> *mut u8;
    fn manta_cpp_arena_release_pages(ptr: *mut u8, offset: usize, len: usize);
    fn manta_cpp_arena_free(ptr: *mut u8, bytes: usize);
    fn manta_cpp_scan_html_delim(data: *const u8, len: usize, start: usize) -> usize;
    fn manta_cpp_hash_bytes(data: *const u8, len: usize) -> u64;
    fn manta_cpp_find_substr(
        hay: *const u8,
        hay_len: usize,
        needle: *const u8,
        needle_len: usize,
        start: usize,
    ) -> i64;
    fn manta_cpp_regex_match(
        pattern: *const c_char,
        pat_len: usize,
        flags: *const c_char,
        flags_len: usize,
        text: *const c_char,
        text_len: usize,
        start_pos: usize,
        out_match_start: *mut usize,
        out_match_end: *mut usize,
    ) -> i32;
    fn manta_cpp_base64_encode(
        src: *const u8,
        src_len: usize,
        dst: *mut c_char,
        dst_cap: usize,
    ) -> usize;
    fn manta_cpp_base64_decode(
        src: *const c_char,
        src_len: usize,
        dst: *mut u8,
        dst_cap: usize,
    ) -> usize;
    fn manta_cpp_json_escape(
        src: *const u8,
        src_len: usize,
        dst: *mut u8,
        dst_cap: usize,
    ) -> usize;
    fn manta_cpp_f64_sum(arr: *const f64, len: usize) -> f64;
    fn manta_cpp_f64_dot(a: *const f64, b: *const f64, len: usize) -> f64;
    fn manta_cpp_f64_scale_add(
        dst: *mut f64,
        src: *const f64,
        len: usize,
        scale: f64,
        bias: f64,
    );
    fn manta_cpp_f64_sort(arr: *mut f64, len: usize);
    fn manta_cpp_fast_compress(
        src: *const u8,
        src_len: usize,
        dst: *mut u8,
        dst_cap: usize,
    ) -> usize;
    fn manta_cpp_fast_decompress(
        src: *const u8,
        src_len: usize,
        dst: *mut u8,
        dst_cap: usize,
    ) -> usize;
    fn manta_cpp_ring_create(capacity_pow2: u32) -> *mut MantaRingBuffer;
    fn manta_cpp_ring_destroy(rb: *mut MantaRingBuffer);
    fn manta_cpp_ring_write(rb: *mut MantaRingBuffer, data: *const u8, len: u32) -> u32;
    fn manta_cpp_ring_read(rb: *mut MantaRingBuffer, out: *mut u8, max_len: u32) -> u32;
    fn manta_cpp_ring_available_read(rb: *const MantaRingBuffer) -> u32;
    fn manta_cpp_ring_capacity(rb: *const MantaRingBuffer) -> u32;
    fn manta_cpp_ring_reset(rb: *mut MantaRingBuffer);
    fn manta_cpp_blend_rect_argb(
        fb: *mut u32,
        fb_w: u32,
        fb_h: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        argb: u32,
    );
    fn manta_cpp_tls_is_available() -> i32;
    fn manta_cpp_tls_connect(sock_fd: i32, hostname: *const c_char) -> *mut c_void;
    fn manta_cpp_tls_write(tls_handle: *mut c_void, data: *const u8, len: i32) -> i32;
    fn manta_cpp_tls_read(tls_handle: *mut c_void, out: *mut u8, cap: i32) -> i32;
    fn manta_cpp_tls_close(tls_handle: *mut c_void);
}

#[inline]
pub fn secure_zero_slice(buf: &mut [u8]) {
    if !buf.is_empty() {
        unsafe {
            manta_cpp_secure_zero(buf.as_mut_ptr() as *mut c_void, buf.len());
        }
    }
}

#[inline]
pub fn secure_zero_string(s: &mut String) {
    if !s.is_empty() {
        unsafe {
            let bytes = s.as_bytes_mut();
            manta_cpp_secure_zero(bytes.as_mut_ptr() as *mut c_void, bytes.len());
        }
        s.clear();
    }
}

#[inline]
pub fn scan_html_delim(bytes: &[u8], start: usize) -> usize {
    if start >= bytes.len() {
        return bytes.len();
    }
    unsafe { manta_cpp_scan_html_delim(bytes.as_ptr(), bytes.len(), start) }
}

#[inline]
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    unsafe { manta_cpp_hash_bytes(bytes.as_ptr(), bytes.len()) }
}

#[inline]
pub fn find_substr(hay: &str, needle: &str, start: usize) -> Option<usize> {
    let hb = hay.as_bytes();
    let nb = needle.as_bytes();
    if nb.is_empty() {
        return if start <= hb.len() { Some(start) } else { None };
    }
    let idx = unsafe { manta_cpp_find_substr(hb.as_ptr(), hb.len(), nb.as_ptr(), nb.len(), start) };
    if idx >= 0 {
        Some(idx as usize)
    } else {
        None
    }
}

pub fn regex_find(pattern: &str, flags: &str, text: &str, start_byte: usize) -> Option<(usize, usize)> {
    let mut m_start: usize = 0;
    let mut m_end: usize = 0;
    let matched = unsafe {
        manta_cpp_regex_match(
            pattern.as_ptr() as *const c_char,
            pattern.len(),
            flags.as_ptr() as *const c_char,
            flags.len(),
            text.as_ptr() as *const c_char,
            text.len(),
            start_byte,
            &mut m_start,
            &mut m_end,
        )
    };
    if matched == 1 {
        Some((m_start, m_end))
    } else {
        None
    }
}

pub fn base64_encode(input: &[u8]) -> String {
    if input.is_empty() {
        return String::new();
    }
    let cap = ((input.len() + 2) / 3) * 4;
    let mut out = vec![0u8; cap];
    let written = unsafe {
        manta_cpp_base64_encode(
            input.as_ptr(),
            input.len(),
            out.as_mut_ptr() as *mut c_char,
            cap,
        )
    };
    out.truncate(written);
    String::from_utf8(out).unwrap_or_default()
}

pub fn base64_decode(input: &str) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }
    let cap = (input.len() * 3) / 4 + 4;
    let mut out = vec![0u8; cap];
    let written = unsafe {
        manta_cpp_base64_decode(
            input.as_ptr() as *const c_char,
            input.len(),
            out.as_mut_ptr(),
            cap,
        )
    };
    out.truncate(written);
    out
}

pub fn json_escape_str(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    let bytes = input.as_bytes();
    let cap = bytes.len() * 6 + 4;
    let mut dst = vec![0u8; cap];
    let written = unsafe { manta_cpp_json_escape(bytes.as_ptr(), bytes.len(), dst.as_mut_ptr(), cap) };
    dst.truncate(written);
    String::from_utf8(dst).unwrap_or_default()
}

#[inline]
pub fn fast_f64_sum(arr: &[f64]) -> f64 {
    if arr.is_empty() {
        return 0.0;
    }
    unsafe { manta_cpp_f64_sum(arr.as_ptr(), arr.len()) }
}

#[inline]
pub fn fast_f64_dot(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    unsafe { manta_cpp_f64_dot(a.as_ptr(), b.as_ptr(), n) }
}

#[inline]
pub fn fast_f64_scale_add(dst: &mut [f64], src: &[f64], scale: f64, bias: f64) {
    let n = dst.len().min(src.len());
    if n > 0 {
        unsafe { manta_cpp_f64_scale_add(dst.as_mut_ptr(), src.as_ptr(), n, scale, bias) }
    }
}

#[inline]
pub fn fast_f64_sort(arr: &mut [f64]) {
    if arr.len() > 1 {
        unsafe { manta_cpp_f64_sort(arr.as_mut_ptr(), arr.len()) }
    }
}

pub fn fast_compress(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }
    let max_cap = input.len() + (input.len() / 64) + 32;
    let mut dst = vec![0u8; max_cap];
    let written = unsafe {
        manta_cpp_fast_compress(input.as_ptr(), input.len(), dst.as_mut_ptr(), max_cap)
    };
    if written == 0 {
        return Vec::new();
    }
    dst.truncate(written);
    dst
}

pub fn fast_decompress(compressed: &[u8]) -> Option<Vec<u8>> {
    if compressed.len() < 4 {
        return None;
    }
    let orig_len = u32::from_le_bytes([
        compressed[0],
        compressed[1],
        compressed[2],
        compressed[3],
    ]) as usize;
    if orig_len == 0 || orig_len > 64 * 1024 * 1024 {
        return None;
    }
    let mut dst = vec![0u8; orig_len];
    let decoded = unsafe {
        manta_cpp_fast_decompress(
            compressed.as_ptr(),
            compressed.len(),
            dst.as_mut_ptr(),
            orig_len,
        )
    };
    if decoded != orig_len {
        return None;
    }
    Some(dst)
}

pub struct CppMmapSlab {
    ptr: *mut u8,
    cap: usize,
    len: usize,
}

unsafe impl Send for CppMmapSlab {}
unsafe impl Sync for CppMmapSlab {}

impl CppMmapSlab {
    pub fn with_capacity(cap: usize) -> Self {
        let actual_cap = cap.max(4096);
        let ptr = unsafe { manta_cpp_arena_alloc(actual_cap) };
        Self {
            ptr,
            cap: if ptr.is_null() { 0 } else { actual_cap },
            len: 0,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) -> Option<usize> {
        if bytes.is_empty() {
            return Some(self.len);
        }
        let needed = self.len.checked_add(bytes.len())?;
        if needed > self.cap {
            let new_cap = needed.next_power_of_two().max(self.cap * 2).max(8192);
            let new_ptr = unsafe { manta_cpp_arena_alloc(new_cap) };
            if new_ptr.is_null() {
                return None;
            }
            if !self.ptr.is_null() && self.len > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(self.ptr, new_ptr, self.len);
                }
            }
            if !self.ptr.is_null() {
                unsafe { manta_cpp_arena_free(self.ptr, self.cap) };
            }
            self.ptr = new_ptr;
            self.cap = new_cap;
        }
        let offset = self.len;
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr.add(offset), bytes.len());
        }
        self.len += bytes.len();
        Some(offset)
    }

    pub fn release_unused_pages(&mut self) {
        if !self.ptr.is_null() && self.cap > self.len {
            unsafe {
                manta_cpp_arena_release_pages(self.ptr, self.len, self.cap - self.len);
            }
        }
    }

    pub fn clear_and_release(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                manta_cpp_arena_free(self.ptr, self.cap);
            }
            self.ptr = std::ptr::null_mut();
        }
        self.cap = 0;
        self.len = 0;
    }
}

impl Drop for CppMmapSlab {
    fn drop(&mut self) {
        self.clear_and_release();
    }
}

pub struct CppMediaRing {
    ptr: *mut MantaRingBuffer,
}

unsafe impl Send for CppMediaRing {}
unsafe impl Sync for CppMediaRing {}

impl CppMediaRing {
    pub fn new(capacity_pow2: u32) -> Option<Self> {
        let ptr = unsafe { manta_cpp_ring_create(capacity_pow2) };
        if ptr.is_null() {
            None
        } else {
            Some(Self { ptr })
        }
    }

    #[inline]
    pub fn write(&self, data: &[u8]) -> usize {
        if data.is_empty() {
            return 0;
        }
        unsafe { manta_cpp_ring_write(self.ptr, data.as_ptr(), data.len().min(u32::MAX as usize) as u32) as usize }
    }

    #[inline]
    pub fn read(&self, out: &mut [u8]) -> usize {
        if out.is_empty() {
            return 0;
        }
        unsafe {
            manta_cpp_ring_read(
                self.ptr,
                out.as_mut_ptr(),
                out.len().min(u32::MAX as usize) as u32,
            ) as usize
        }
    }

    #[inline]
    pub fn available_read(&self) -> usize {
        unsafe { manta_cpp_ring_available_read(self.ptr) as usize }
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        unsafe { manta_cpp_ring_capacity(self.ptr) as usize }
    }

    #[inline]
    pub fn reset(&self) {
        unsafe { manta_cpp_ring_reset(self.ptr) }
    }
}

impl Drop for CppMediaRing {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { manta_cpp_ring_destroy(self.ptr) };
            self.ptr = std::ptr::null_mut();
        }
    }
}

pub fn blend_rect_argb(
    fb: &mut [u32],
    fb_w: u32,
    fb_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    argb: u32,
) {
    if fb.len() < (fb_w as usize).saturating_mul(fb_h as usize) {
        return;
    }
    unsafe {
        manta_cpp_blend_rect_argb(fb.as_mut_ptr(), fb_w, fb_h, x, y, w, h, argb);
    }
}

pub fn tls_is_available() -> bool {
    unsafe { manta_cpp_tls_is_available() == 1 }
}

pub struct CppTlsStream {
    handle: *mut c_void,
    _socket: TcpStream,
}

unsafe impl Send for CppTlsStream {}

impl CppTlsStream {
    #[cfg(unix)]
    pub fn connect(socket: TcpStream, hostname: &str) -> Result<Self, &'static str> {
        let c_host = CString::new(hostname).map_err(|_| "Invalid TLS hostname")?;
        let raw_fd = socket.as_raw_fd();
        let handle = unsafe { manta_cpp_tls_connect(raw_fd, c_host.as_ptr()) };
        if handle.is_null() {
            return Err("TLS handshake or certificate verification failed");
        }
        Ok(Self {
            handle,
            _socket: socket,
        })
    }

    #[cfg(not(unix))]
    pub fn connect(_socket: TcpStream, _hostname: &str) -> Result<Self, &'static str> {
        Err("Native POSIX TLS requires Unix sockets or registered host transport")
    }

    pub fn write_all_tls(&mut self, mut data: &[u8]) -> Result<(), &'static str> {
        while !data.is_empty() {
            let chunk_len = data.len().min(16384) as i32;
            let n = unsafe { manta_cpp_tls_write(self.handle, data.as_ptr(), chunk_len) };
            if n <= 0 {
                return Err("TLS socket write failed");
            }
            data = &data[n as usize..];
        }
        Ok(())
    }

    pub fn read_tls(&mut self, out: &mut [u8]) -> Result<usize, &'static str> {
        if out.is_empty() {
            return Ok(0);
        }
        let cap = out.len().min(i32::MAX as usize) as i32;
        let n = unsafe { manta_cpp_tls_read(self.handle, out.as_mut_ptr(), cap) };
        if n < 0 {
            return Err("TLS socket read failed");
        }
        Ok(n as usize)
    }
}

impl Drop for CppTlsStream {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { manta_cpp_tls_close(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpp_simd_substr_regex_sort_base64_and_json_escape() {
        let html = b"Hello world this is plain text with <div>tags</div> &amp; entities";
        assert_eq!(scan_html_delim(html, 0), 36);
        assert_eq!(
            find_substr("Manta high-velocity browser core", "velocity", 0),
            Some(11)
        );
        assert_eq!(
            regex_find(r"manta[0-9]+", "i", "Version MANTA2026 release", 0),
            Some((8, 17))
        );
        let nums = [1.5, 2.5, 3.0, 4.0, 5.0];
        assert!((fast_f64_sum(&nums) - 16.0).abs() < 1e-9);
        assert!((fast_f64_dot(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]) - 32.0).abs() < 1e-9);

        let mut unsorted = [9.0, -3.0, 42.0, 1.5, 0.0, 12.0, 7.0, 3.0, 8.0, 2.0, 5.0, 11.0, 6.0, 4.0, 10.0, -1.0, 20.0];
        fast_f64_sort(&mut unsorted);
        assert_eq!(unsorted[0], -3.0);
        assert_eq!(unsorted[unsorted.len() - 1], 42.0);

        let enc = base64_encode(b"Manta Core");
        assert_eq!(enc, "TWFudGEgQ29yZQ==");
        assert_eq!(&base64_decode(&enc), b"Manta Core");
        assert_eq!(json_escape_str("hi \"manta\"\n"), "hi \\\"manta\\\"\\n");
    }
}
