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

/// SIMD-accelerated delimiter search for `<` and `&` in HTML data streams.
#[inline]
pub fn scan_html_delim(bytes: &[u8], start: usize) -> usize {
    if start >= bytes.len() {
        return bytes.len();
    }
    unsafe { manta_cpp_scan_html_delim(bytes.as_ptr(), bytes.len(), start) }
}

/// Fast C++ Hash-LZ + RLE block compressor.
pub fn fast_compress(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }
    // Worst-case overhead is ~2% + 8 bytes for incompressible literals
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

/// Fast C++ Hash-LZ + RLE block decompressor.
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

/// Direct OS `mmap`-backed byte slab that can release unused pages via `madvise(MADV_DONTNEED)`.
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

    /// Returns spare physical memory pages above `self.len` directly back to the OS kernel.
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

/// Lock-free cache-line-aligned SPSC ring buffer for ultra-low-latency media streaming.
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

/// Verified TLS stream wrapping `TcpStream` via runtime OpenSSL in C++.
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
    fn test_cpp_simd_scanner_and_lz_rle_codec_and_ring() {
        let html = b"Hello world this is plain text with <div>tags</div> &amp; entities";
        assert_eq!(scan_html_delim(html, 0), 36);
        assert_eq!(scan_html_delim(html, 41), 41);
        assert_eq!(scan_html_delim(html, 42), 52);

        let payload = b"MantaBrowserEngine!!!!MantaBrowserEngine!!!!AAAAABBBBBCCCCCDDDDDMantaBrowserEngine";
        let compressed = fast_compress(payload);
        assert!(!compressed.is_empty());
        assert!(compressed.len() < payload.len());
        let decompressed = fast_decompress(&compressed).unwrap();
        assert_eq!(&decompressed, payload);

        let ring = CppMediaRing::new(4096).unwrap();
        assert_eq!(ring.write(b"VIDEO_FRAME_1"), 13);
        assert_eq!(ring.available_read(), 13);
        let mut out = [0u8; 32];
        let n = ring.read(&mut out);
        assert_eq!(&out[..n], b"VIDEO_FRAME_1");
        assert_eq!(ring.available_read(), 0);
    }
}
