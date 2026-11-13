#ifndef MANTA_ACCEL_H
#define MANTA_ACCEL_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct MantaRingBuffer MantaRingBuffer;

/* Compiler-proof volatile memory zeroing for keys, tokens, and shredded tabs */
void     manta_cpp_secure_zero(void* ptr, size_t len);

/* Direct OS Virtual Memory Arena (mmap + madvise MADV_DONTNEED to return RSS to kernel) */
void*    manta_cpp_arena_alloc(size_t bytes);
void     manta_cpp_arena_release_pages(void* ptr, size_t offset, size_t len);
void     manta_cpp_arena_free(void* ptr, size_t bytes);

/* SIMD-accelerated HTML/XML delimiter scanner ('<' or '&') */
size_t   manta_cpp_scan_html_delim(const uint8_t* data, size_t len, size_t start);

/* Fast Hash-LZ + RLE block compressor/decompressor for tab & asset snapshots */
size_t   manta_cpp_fast_compress(const uint8_t* src, size_t src_len, uint8_t* dst, size_t dst_cap);
size_t   manta_cpp_fast_decompress(const uint8_t* src, size_t src_len, uint8_t* dst, size_t dst_cap);

/* Lock-free cache-line-aligned SPSC ring buffer for ultra-low-latency video/audio streaming */
MantaRingBuffer* manta_cpp_ring_create(uint32_t capacity_pow2);
void             manta_cpp_ring_destroy(MantaRingBuffer* rb);
uint32_t         manta_cpp_ring_write(MantaRingBuffer* rb, const uint8_t* data, uint32_t len);
uint32_t         manta_cpp_ring_read(MantaRingBuffer* rb, uint8_t* out, uint32_t max_len);
uint32_t         manta_cpp_ring_available_read(const MantaRingBuffer* rb);
uint32_t         manta_cpp_ring_capacity(const MantaRingBuffer* rb);
void             manta_cpp_ring_reset(MantaRingBuffer* rb);

/* SIMD-friendly ARGB rectangle alpha compositor for fast software/hybrid tile rendering */
void     manta_cpp_blend_rect_argb(uint32_t* fb, uint32_t fb_w, uint32_t fb_h,
                                   int32_t x, int32_t y, int32_t w, int32_t h, uint32_t argb);

/* Dynamic POSIX OpenSSL runtime loader (libssl.so.3) with TLS 1.2+, SNI & X.509 verify */
int32_t  manta_cpp_tls_is_available(void);
void*    manta_cpp_tls_connect(int32_t sock_fd, const char* hostname);
int32_t  manta_cpp_tls_write(void* tls_handle, const uint8_t* data, int32_t len);
int32_t  manta_cpp_tls_read(void* tls_handle, uint8_t* out, int32_t cap);
void     manta_cpp_tls_close(void* tls_handle);

#ifdef __cplusplus
}
#endif

#endif /* MANTA_ACCEL_H */
