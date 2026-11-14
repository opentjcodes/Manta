#include "manta_accel.h"

#include <stdlib.h>
#include <string.h>

#if defined(__unix__) || defined(__linux__) || defined(__ANDROID__)
#include <dlfcn.h>
#include <sys/mman.h>
#include <unistd.h>
#define MANTA_HAS_POSIX 1
#else
#define MANTA_HAS_POSIX 0
#endif

#if defined(__SSE2__)
#include <emmintrin.h>
#endif

#if defined(__aarch64__)
#include <arm_neon.h>
#endif

namespace {

inline size_t get_page_size() {
#if MANTA_HAS_POSIX
    long p = ::sysconf(_SC_PAGESIZE);
    return (p > 0) ? static_cast<size_t>(p) : 4096u;
#else
    return 4096u;
#endif
}

inline size_t align_up(size_t val, size_t align) {
    return (val + align - 1u) & ~(align - 1u);
}

inline uint32_t read_u32_le(const uint8_t* p) {
    return static_cast<uint32_t>(p[0]) |
           (static_cast<uint32_t>(p[1]) << 8) |
           (static_cast<uint32_t>(p[2]) << 16) |
           (static_cast<uint32_t>(p[3]) << 24);
}

inline void write_u32_le(uint8_t* p, uint32_t v) {
    p[0] = static_cast<uint8_t>(v & 0xFFu);
    p[1] = static_cast<uint8_t>((v >> 8) & 0xFFu);
    p[2] = static_cast<uint8_t>((v >> 16) & 0xFFu);
    p[3] = static_cast<uint8_t>((v >> 24) & 0xFFu);
}

inline bool f64_less(double a, double b) {
    if (!(a == a)) return false; // NaN sorts last
    if (!(b == b)) return true;
    return a < b;
}

inline uint8_t ascii_lower(uint8_t c) {
    return (c >= 'A' && c <= 'Z') ? static_cast<uint8_t>(c + 32) : c;
}

inline bool is_ascii_digit(uint8_t c) { return c >= '0' && c <= '9'; }
inline bool is_ascii_word(uint8_t c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_';
}
inline bool is_ascii_space(uint8_t c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\f' || c == '\v';
}

struct RegexCtx {
    const char* pat;
    size_t pat_len;
    const char* text;
    size_t text_len;
    bool case_insensitive;
    bool multiline;
    bool dot_all;
    uint32_t steps;
    uint32_t max_steps;
};

bool match_seq(RegexCtx& ctx, size_t p_start, size_t p_end, size_t t_pos, size_t* out_end, int depth);

bool match_char_class(const char* pat, size_t& p, size_t p_end, uint8_t ch, bool ci) {
    size_t scan = p + 1;
    bool neg = false;
    if (scan < p_end && pat[scan] == '^') {
        neg = true;
        ++scan;
    }
    bool matched = false;
    bool first = true;
    uint8_t cmp_ch = ci ? ascii_lower(ch) : ch;

    while (scan < p_end) {
        if (!first && pat[scan] == ']') {
            p = scan + 1;
            return neg ? !matched : matched;
        }
        first = false;
        uint8_t c1 = static_cast<uint8_t>(pat[scan]);
        if (c1 == '\\' && scan + 1 < p_end) {
            uint8_t esc = static_cast<uint8_t>(pat[scan + 1]);
            scan += 2;
            switch (esc) {
                case 'd': if (is_ascii_digit(ch)) matched = true; continue;
                case 'D': if (!is_ascii_digit(ch)) matched = true; continue;
                case 'w': if (is_ascii_word(ch)) matched = true; continue;
                case 'W': if (!is_ascii_word(ch)) matched = true; continue;
                case 's': if (is_ascii_space(ch)) matched = true; continue;
                case 'S': if (!is_ascii_space(ch)) matched = true; continue;
                case 'n': c1 = '\n'; break;
                case 'r': c1 = '\r'; break;
                case 't': c1 = '\t'; break;
                default: c1 = esc; break;
            }
        } else {
            ++scan;
        }

        if (scan + 1 < p_end && pat[scan] == '-' && pat[scan + 1] != ']') {
            uint8_t c2 = static_cast<uint8_t>(pat[scan + 1]);
            scan += 2;
            uint8_t lo = ci ? ascii_lower(c1) : c1;
            uint8_t hi = ci ? ascii_lower(c2) : c2;
            if (cmp_ch >= lo && cmp_ch <= hi) matched = true;
        } else {
            uint8_t target = ci ? ascii_lower(c1) : c1;
            if (cmp_ch == target) matched = true;
        }
    }
    p = p_end;
    return false;
}

bool match_atom(RegexCtx& ctx, size_t a_start, size_t a_end, size_t t_pos, size_t* out_end, int depth) {
    if (a_start >= a_end) {
        *out_end = t_pos;
        return true;
    }
    char first = ctx.pat[a_start];
    if (first == '(') {
        size_t inner_start = a_start + 1;
        if (inner_start + 1 < a_end && ctx.pat[inner_start] == '?' && ctx.pat[inner_start + 1] == ':') {
            inner_start += 2;
        }
        size_t inner_end = (a_end > a_start && ctx.pat[a_end - 1] == ')') ? (a_end - 1) : a_end;
        return match_seq(ctx, inner_start, inner_end, t_pos, out_end, depth + 1);
    }
    if (t_pos >= ctx.text_len) return false;
    uint8_t ch = static_cast<uint8_t>(ctx.text[t_pos]);

    if (first == '.') {
        if (!ctx.dot_all && (ch == '\n' || ch == '\r')) return false;
        *out_end = t_pos + 1;
        return true;
    }
    if (first == '[') {
        size_t p_tmp = a_start;
        if (match_char_class(ctx.pat, p_tmp, a_end, ch, ctx.case_insensitive)) {
            *out_end = t_pos + 1;
            return true;
        }
        return false;
    }
    if (first == '\\' && a_start + 1 < a_end) {
        uint8_t esc = static_cast<uint8_t>(ctx.pat[a_start + 1]);
        bool ok = false;
        switch (esc) {
            case 'd': ok = is_ascii_digit(ch); break;
            case 'D': ok = !is_ascii_digit(ch); break;
            case 'w': ok = is_ascii_word(ch); break;
            case 'W': ok = !is_ascii_word(ch); break;
            case 's': ok = is_ascii_space(ch); break;
            case 'S': ok = !is_ascii_space(ch); break;
            case 'n': ok = (ch == '\n'); break;
            case 'r': ok = (ch == '\r'); break;
            case 't': ok = (ch == '\t'); break;
            default: {
                uint8_t a = ctx.case_insensitive ? ascii_lower(ch) : ch;
                uint8_t b = ctx.case_insensitive ? ascii_lower(esc) : esc;
                ok = (a == b);
                break;
            }
        }
        if (ok) {
            *out_end = t_pos + 1;
            return true;
        }
        return false;
    }

    uint8_t a = ctx.case_insensitive ? ascii_lower(ch) : ch;
    uint8_t b = ctx.case_insensitive ? ascii_lower(static_cast<uint8_t>(first)) : static_cast<uint8_t>(first);
    if (a == b) {
        *out_end = t_pos + 1;
        return true;
    }
    return false;
}

size_t parse_brace_quantifier(const char* pat, size_t pos, size_t p_end, size_t& min_r, size_t& max_r) {
    size_t i = pos + 1;
    size_t v1 = 0;
    bool has1 = false;
    while (i < p_end && is_ascii_digit(static_cast<uint8_t>(pat[i]))) {
        v1 = v1 * 10u + static_cast<size_t>(pat[i] - '0');
        has1 = true;
        ++i;
    }
    if (!has1 || i >= p_end) return pos;
    if (pat[i] == '}') {
        min_r = v1;
        max_r = v1;
        return i + 1;
    }
    if (pat[i] == ',') {
        ++i;
        if (i < p_end && pat[i] == '}') {
            min_r = v1;
            max_r = 0xFFFFFFu;
            return i + 1;
        }
        size_t v2 = 0;
        bool has2 = false;
        while (i < p_end && is_ascii_digit(static_cast<uint8_t>(pat[i]))) {
            v2 = v2 * 10u + static_cast<size_t>(pat[i] - '0');
            has2 = true;
            ++i;
        }
        if (has2 && i < p_end && pat[i] == '}') {
            min_r = v1;
            max_r = (v2 < v1) ? v1 : v2;
            return i + 1;
        }
    }
    return pos;
}

bool match_branch(RegexCtx& ctx, size_t p_pos, size_t p_end, size_t t_pos, size_t* out_end, int depth) {
    if (++ctx.steps > ctx.max_steps || depth > 64) return false;
    if (p_pos >= p_end) {
        *out_end = t_pos;
        return true;
    }

    if (ctx.pat[p_pos] == '^') {
        bool at_start = (t_pos == 0) || (ctx.multiline && t_pos > 0 && ctx.text[t_pos - 1] == '\n');
        return at_start ? match_branch(ctx, p_pos + 1, p_end, t_pos, out_end, depth) : false;
    }
    if (ctx.pat[p_pos] == '$') {
        bool at_end = (t_pos == ctx.text_len) || (ctx.multiline && t_pos < ctx.text_len && ctx.text[t_pos] == '\n');
        return at_end ? match_branch(ctx, p_pos + 1, p_end, t_pos, out_end, depth) : false;
    }
    if (ctx.pat[p_pos] == '\\' && p_pos + 1 < p_end && (ctx.pat[p_pos + 1] == 'b' || ctx.pat[p_pos + 1] == 'B')) {
        bool prev_w = (t_pos > 0) && is_ascii_word(static_cast<uint8_t>(ctx.text[t_pos - 1]));
        bool curr_w = (t_pos < ctx.text_len) && is_ascii_word(static_cast<uint8_t>(ctx.text[t_pos]));
        bool is_bound = (prev_w != curr_w);
        bool ok = (ctx.pat[p_pos + 1] == 'b') ? is_bound : !is_bound;
        return ok ? match_branch(ctx, p_pos + 2, p_end, t_pos, out_end, depth) : false;
    }

    size_t a_start = p_pos;
    size_t a_end = p_pos;
    if (ctx.pat[p_pos] == '(') {
        int paren_depth = 1;
        size_t i = p_pos + 1;
        while (i < p_end && paren_depth > 0) {
            if (ctx.pat[i] == '\\' && i + 1 < p_end) {
                i += 2;
                continue;
            }
            if (ctx.pat[i] == '[') {
                ++i;
                while (i < p_end) {
                    if (ctx.pat[i] == '\\' && i + 1 < p_end) i += 2;
                    else if (ctx.pat[i] == ']') { ++i; break; }
                    else ++i;
                }
                continue;
            }
            if (ctx.pat[i] == '(') ++paren_depth;
            else if (ctx.pat[i] == ')') --paren_depth;
            ++i;
        }
        a_end = i;
    } else if (ctx.pat[p_pos] == '[') {
        size_t i = p_pos + 1;
        if (i < p_end && ctx.pat[i] == '^') ++i;
        if (i < p_end && ctx.pat[i] == ']') ++i;
        while (i < p_end) {
            if (ctx.pat[i] == '\\' && i + 1 < p_end) i += 2;
            else if (ctx.pat[i] == ']') { ++i; break; }
            else ++i;
        }
        a_end = i;
    } else if (ctx.pat[p_pos] == '\\' && p_pos + 1 < p_end) {
        a_end = p_pos + 2;
    } else {
        a_end = p_pos + 1;
    }

    size_t min_rep = 1;
    size_t max_rep = 1;
    size_t next_p = a_end;
    bool has_quant = false;
    bool lazy = false;

    if (next_p < p_end) {
        char q = ctx.pat[next_p];
        if (q == '*') { min_rep = 0; max_rep = 0xFFFFFFu; ++next_p; has_quant = true; }
        else if (q == '+') { min_rep = 1; max_rep = 0xFFFFFFu; ++next_p; has_quant = true; }
        else if (q == '?') { min_rep = 0; max_rep = 1; ++next_p; has_quant = true; }
        else if (q == '{') {
            size_t after_q = parse_brace_quantifier(ctx.pat, next_p, p_end, min_rep, max_rep);
            if (after_q != next_p) {
                next_p = after_q;
                has_quant = true;
            }
        }
        if (has_quant && next_p < p_end && ctx.pat[next_p] == '?') {
            lazy = true;
            ++next_p;
        }
    }

    if (!has_quant) {
        size_t mid_t = 0;
        if (!match_atom(ctx, a_start, a_end, t_pos, &mid_t, depth)) return false;
        return match_branch(ctx, next_p, p_end, mid_t, out_end, depth);
    }

    size_t cur_t = t_pos;
    size_t rep = 0;
    while (rep < min_rep) {
        size_t nxt = 0;
        if (!match_atom(ctx, a_start, a_end, cur_t, &nxt, depth)) return false;
        cur_t = nxt;
        ++rep;
    }

    if (lazy) {
        while (rep <= max_rep) {
            if (match_branch(ctx, next_p, p_end, cur_t, out_end, depth)) return true;
            size_t nxt = 0;
            if (!match_atom(ctx, a_start, a_end, cur_t, &nxt, depth)) break;
            if (nxt == cur_t) break;
            cur_t = nxt;
            ++rep;
        }
        return false;
    }

    static constexpr size_t STACK_POS_CAP = 256;
    size_t stack_pos[STACK_POS_CAP];
    size_t count = 1;
    stack_pos[0] = cur_t;

    while (rep < max_rep && count < STACK_POS_CAP) {
        size_t nxt = 0;
        if (!match_atom(ctx, a_start, a_end, cur_t, &nxt, depth)) break;
        if (nxt == cur_t) break;
        cur_t = nxt;
        stack_pos[count++] = cur_t;
        ++rep;
    }

    for (size_t idx = count; idx > 0; --idx) {
        if (match_branch(ctx, next_p, p_end, stack_pos[idx - 1], out_end, depth)) return true;
    }
    return false;
}

bool match_seq(RegexCtx& ctx, size_t p_start, size_t p_end, size_t t_pos, size_t* out_end, int depth) {
    if (++ctx.steps > ctx.max_steps || depth > 64) return false;

    int paren = 0;
    size_t seg_start = p_start;
    for (size_t i = p_start; i <= p_end; ++i) {
        bool at_end = (i == p_end);
        if (!at_end) {
            if (ctx.pat[i] == '\\' && i + 1 < p_end) { ++i; continue; }
            if (ctx.pat[i] == '[') {
                ++i;
                while (i < p_end) {
                    if (ctx.pat[i] == '\\' && i + 1 < p_end) i += 2;
                    else if (ctx.pat[i] == ']') break;
                    else ++i;
                }
                continue;
            }
            if (ctx.pat[i] == '(') { ++paren; continue; }
            if (ctx.pat[i] == ')') { if (paren > 0) --paren; continue; }
        }
        if (at_end || (paren == 0 && ctx.pat[i] == '|')) {
            if (match_branch(ctx, seg_start, i, t_pos, out_end, depth)) {
                return true;
            }
            seg_start = i + 1;
        }
    }
    return false;
}

} // namespace

extern "C" {

void manta_cpp_secure_zero(void* ptr, size_t len) {
    if (!ptr || len == 0) return;
    volatile uint8_t* p = static_cast<volatile uint8_t*>(ptr);
    for (size_t i = 0; i < len; ++i) {
        p[i] = 0;
    }
    __atomic_signal_fence(__ATOMIC_SEQ_CST);
}

void* manta_cpp_arena_alloc(size_t bytes) {
    if (bytes == 0) return nullptr;
#if MANTA_HAS_POSIX
    const size_t page = get_page_size();
    if (bytes >= page) {
        const size_t total = align_up(bytes, page);
        void* mem = ::mmap(nullptr, total, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (mem != MAP_FAILED) {
            return mem;
        }
    }
#endif
    return ::calloc(1, bytes);
}

void manta_cpp_arena_release_pages(void* ptr, size_t offset, size_t len) {
    if (!ptr || len == 0) return;
#if MANTA_HAS_POSIX
    const size_t page = get_page_size();
    const uintptr_t base = reinterpret_cast<uintptr_t>(ptr);
    const uintptr_t start = align_up(base + offset, page);
    const uintptr_t end = (base + offset + len) & ~(page - 1u);
    if (end > start) {
        ::madvise(reinterpret_cast<void*>(start), static_cast<size_t>(end - start), MADV_DONTNEED);
    }
#else
    manta_cpp_secure_zero(static_cast<uint8_t*>(ptr) + offset, len);
#endif
}

void manta_cpp_arena_free(void* ptr, size_t bytes) {
    if (!ptr) return;
#if MANTA_HAS_POSIX
    const size_t page = get_page_size();
    if (bytes >= page) {
        const size_t total = align_up(bytes, page);
        manta_cpp_secure_zero(ptr, bytes);
        ::munmap(ptr, total);
        return;
    }
#endif
    manta_cpp_secure_zero(ptr, bytes);
    ::free(ptr);
}

size_t manta_cpp_scan_html_delim(const uint8_t* data, size_t len, size_t start) {
    if (!data || start >= len) return len;
    size_t i = start;

#if defined(__SSE2__)
    const __m128i lt_vec = _mm_set1_epi8('<');
    const __m128i amp_vec = _mm_set1_epi8('&');
    while (i + 16 <= len) {
        __m128i chunk = _mm_loadu_si128(reinterpret_cast<const __m128i*>(data + i));
        __m128i eq_lt = _mm_cmpeq_epi8(chunk, lt_vec);
        __m128i eq_amp = _mm_cmpeq_epi8(chunk, amp_vec);
        int mask = _mm_movemask_epi8(_mm_or_si128(eq_lt, eq_amp));
        if (mask != 0) {
            return i + static_cast<size_t>(__builtin_ctz(static_cast<unsigned>(mask)));
        }
        i += 16;
    }
#elif defined(__aarch64__)
    const uint8x16_t lt_vec = vdupq_n_u8('<');
    const uint8x16_t amp_vec = vdupq_n_u8('&');
    while (i + 16 <= len) {
        uint8x16_t chunk = vld1q_u8(data + i);
        uint8x16_t match = vorrq_u8(vceqq_u8(chunk, lt_vec), vceqq_u8(chunk, amp_vec));
        if (vmaxvq_u8(match) != 0) {
            for (size_t k = 0; k < 16; ++k) {
                uint8_t c = data[i + k];
                if (c == '<' || c == '&') return i + k;
            }
        }
        i += 16;
    }
#endif

    while (i < len) {
        uint8_t c = data[i];
        if (c == '<' || c == '&') return i;
        ++i;
    }
    return len;
}

uint64_t manta_cpp_hash_bytes(const uint8_t* data, size_t len) {
    uint64_t h = 0xcbf29ce484222325ULL;
    if (!data) return h;
    for (size_t i = 0; i < len; ++i) {
        h ^= data[i];
        h *= 0x100000001b3ULL;
    }
    h ^= h >> 33;
    h *= 0xff51afd7ed558ccdULL;
    h ^= h >> 33;
    return h;
}

int64_t manta_cpp_find_substr(const uint8_t* hay, size_t hay_len,
                              const uint8_t* needle, size_t needle_len, size_t start) {
    if (needle_len == 0) {
        return (start <= hay_len) ? static_cast<int64_t>(start) : -1;
    }
    if (!hay || !needle || start + needle_len > hay_len) return -1;

    const uint8_t first = needle[0];
    const size_t limit = hay_len - needle_len;
    size_t i = start;

#if defined(__SSE2__)
    const __m128i first_vec = _mm_set1_epi8(static_cast<char>(first));
    while (i + 16 <= limit + 1) {
        __m128i chunk = _mm_loadu_si128(reinterpret_cast<const __m128i*>(hay + i));
        int mask = _mm_movemask_epi8(_mm_cmpeq_epi8(chunk, first_vec));
        while (mask != 0) {
            int bit = __builtin_ctz(static_cast<unsigned>(mask));
            size_t pos = i + static_cast<size_t>(bit);
            if (pos <= limit && ::memcmp(hay + pos, needle, needle_len) == 0) {
                return static_cast<int64_t>(pos);
            }
            mask &= (mask - 1);
        }
        i += 16;
    }
#elif defined(__aarch64__)
    const uint8x16_t first_vec = vdupq_n_u8(first);
    while (i + 16 <= limit + 1) {
        uint8x16_t chunk = vld1q_u8(hay + i);
        uint8x16_t eq = vceqq_u8(chunk, first_vec);
        if (vmaxvq_u8(eq) != 0) {
            for (size_t k = 0; k < 16; ++k) {
                size_t pos = i + k;
                if (pos <= limit && hay[pos] == first && ::memcmp(hay + pos, needle, needle_len) == 0) {
                    return static_cast<int64_t>(pos);
                }
            }
        }
        i += 16;
    }
#endif

    while (i <= limit) {
        if (hay[i] == first && ::memcmp(hay + i, needle, needle_len) == 0) {
            return static_cast<int64_t>(i);
        }
        ++i;
    }
    return -1;
}

int32_t manta_cpp_regex_match(const char* pattern, size_t pat_len,
                              const char* flags, size_t flags_len,
                              const char* text, size_t text_len,
                              size_t start_pos,
                              size_t* out_match_start, size_t* out_match_end) {
    if (!pattern || !text || pat_len > 2048 || start_pos > text_len) return 0;

    RegexCtx ctx = {};
    ctx.pat = pattern;
    ctx.pat_len = pat_len;
    ctx.text = text;
    ctx.text_len = text_len;
    ctx.max_steps = 250000u;

    if (flags) {
        for (size_t i = 0; i < flags_len; ++i) {
            if (flags[i] == 'i') ctx.case_insensitive = true;
            else if (flags[i] == 'm') ctx.multiline = true;
            else if (flags[i] == 's') ctx.dot_all = true;
        }
    }

    for (size_t pos = start_pos; pos <= text_len; ++pos) {
        ctx.steps = 0;
        size_t end_pos = pos;
        if (match_seq(ctx, 0, pat_len, pos, &end_pos, 0)) {
            if (out_match_start) *out_match_start = pos;
            if (out_match_end) *out_match_end = end_pos;
            return 1;
        }
    }
    return 0;
}

size_t manta_cpp_base64_encode(const uint8_t* src, size_t src_len, char* dst, size_t dst_cap) {
    if (!src || src_len == 0) return 0;
    const size_t needed = ((src_len + 2u) / 3u) * 4u;
    if (!dst || dst_cap < needed) return needed;

    static constexpr char TBL[65] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    size_t i = 0;
    size_t o = 0;
    while (i + 3u <= src_len) {
        uint32_t n = (static_cast<uint32_t>(src[i]) << 16) |
                     (static_cast<uint32_t>(src[i + 1]) << 8) |
                     static_cast<uint32_t>(src[i + 2]);
        dst[o++] = TBL[(n >> 18) & 0x3Fu];
        dst[o++] = TBL[(n >> 12) & 0x3Fu];
        dst[o++] = TBL[(n >> 6) & 0x3Fu];
        dst[o++] = TBL[n & 0x3Fu];
        i += 3u;
    }
    if (i < src_len) {
        uint32_t b0 = src[i];
        uint32_t b1 = (i + 1u < src_len) ? src[i + 1u] : 0u;
        dst[o++] = TBL[(b0 >> 2) & 0x3Fu];
        dst[o++] = TBL[((b0 & 0x03u) << 4) | ((b1 >> 4) & 0x0Fu)];
        dst[o++] = (i + 1u < src_len) ? TBL[(b1 & 0x0Fu) << 2] : '=';
        dst[o++] = '=';
    }
    return o;
}

size_t manta_cpp_base64_decode(const char* src, size_t src_len, uint8_t* dst, size_t dst_cap) {
    if (!src || src_len == 0 || !dst || dst_cap == 0) return 0;
    uint32_t buf = 0;
    uint8_t bits = 0;
    size_t out = 0;
    for (size_t i = 0; i < src_len; ++i) {
        uint8_t c = static_cast<uint8_t>(src[i]);
        uint8_t val = 0xFFu;
        if (c >= 'A' && c <= 'Z') val = c - 'A';
        else if (c >= 'a' && c <= 'z') val = c - 'a' + 26u;
        else if (c >= '0' && c <= '9') val = c - '0' + 52u;
        else if (c == '+') val = 62u;
        else if (c == '/') val = 63u;
        else continue;

        buf = (buf << 6) | val;
        bits += 6u;
        if (bits >= 8u) {
            bits -= 8u;
            if (out >= dst_cap) break;
            dst[out++] = static_cast<uint8_t>((buf >> bits) & 0xFFu);
        }
    }
    return out;
}

size_t manta_cpp_json_escape(const uint8_t* src, size_t src_len, uint8_t* dst, size_t dst_cap) {
    if (!src || src_len == 0) return 0;
    static constexpr char HEX[17] = "0123456789abcdef";
    size_t needed = 0;
    for (size_t i = 0; i < src_len; ++i) {
        uint8_t c = src[i];
        switch (c) {
            case '"':
            case '\\':
            case '\n':
            case '\r':
            case '\t':
                needed += 2u;
                break;
            default:
                needed += (c < 0x20u) ? 6u : 1u;
                break;
        }
    }
    if (!dst || dst_cap < needed) return needed;

    size_t o = 0;
    for (size_t i = 0; i < src_len; ++i) {
        uint8_t c = src[i];
        switch (c) {
            case '"':  dst[o++] = '\\'; dst[o++] = '"'; break;
            case '\\': dst[o++] = '\\'; dst[o++] = '\\'; break;
            case '\n': dst[o++] = '\\'; dst[o++] = 'n'; break;
            case '\r': dst[o++] = '\\'; dst[o++] = 'r'; break;
            case '\t': dst[o++] = '\\'; dst[o++] = 't'; break;
            default:
                if (c < 0x20u) {
                    dst[o++] = '\\';
                    dst[o++] = 'u';
                    dst[o++] = '0';
                    dst[o++] = '0';
                    dst[o++] = static_cast<uint8_t>(HEX[(c >> 4) & 0x0Fu]);
                    dst[o++] = static_cast<uint8_t>(HEX[c & 0x0Fu]);
                } else {
                    dst[o++] = c;
                }
                break;
        }
    }
    return o;
}

double manta_cpp_f64_sum(const double* arr, size_t len) {
    if (!arr || len == 0) return 0.0;
    double acc0 = 0.0, acc1 = 0.0, acc2 = 0.0, acc3 = 0.0;
    size_t i = 0;
    for (; i + 4 <= len; i += 4) {
        acc0 += arr[i];
        acc1 += arr[i + 1];
        acc2 += arr[i + 2];
        acc3 += arr[i + 3];
    }
    double total = (acc0 + acc1) + (acc2 + acc3);
    for (; i < len; ++i) {
        total += arr[i];
    }
    return total;
}

double manta_cpp_f64_dot(const double* a, const double* b, size_t len) {
    if (!a || !b || len == 0) return 0.0;
    double acc0 = 0.0, acc1 = 0.0, acc2 = 0.0, acc3 = 0.0;
    size_t i = 0;
    for (; i + 4 <= len; i += 4) {
        acc0 += a[i] * b[i];
        acc1 += a[i + 1] * b[i + 1];
        acc2 += a[i + 2] * b[i + 2];
        acc3 += a[i + 3] * b[i + 3];
    }
    double total = (acc0 + acc1) + (acc2 + acc3);
    for (; i < len; ++i) {
        total += a[i] * b[i];
    }
    return total;
}

void manta_cpp_f64_scale_add(double* dst, const double* src, size_t len, double scale, double bias) {
    if (!dst || !src || len == 0) return;
    for (size_t i = 0; i < len; ++i) {
        dst[i] = src[i] * scale + bias;
    }
}

void manta_cpp_f64_sort(double* arr, size_t len) {
    if (!arr || len <= 1) return;
    if (len < 16) {
        for (size_t i = 1; i < len; ++i) {
            double key = arr[i];
            size_t j = i;
            while (j > 0 && f64_less(key, arr[j - 1])) {
                arr[j] = arr[j - 1];
                --j;
            }
            arr[j] = key;
        }
        return;
    }

    auto sift_down = [&](size_t start, size_t end) {
        size_t root = start;
        while (root * 2 + 1 <= end) {
            size_t child = root * 2 + 1;
            size_t swap_idx = root;
            if (f64_less(arr[swap_idx], arr[child])) {
                swap_idx = child;
            }
            if (child + 1 <= end && f64_less(arr[swap_idx], arr[child + 1])) {
                swap_idx = child + 1;
            }
            if (swap_idx == root) return;
            double tmp = arr[root];
            arr[root] = arr[swap_idx];
            arr[swap_idx] = tmp;
            root = swap_idx;
        }
    };

    for (size_t start = len / 2; start > 0; --start) {
        sift_down(start - 1, len - 1);
    }
    for (size_t end = len - 1; end > 0; --end) {
        double tmp = arr[0];
        arr[0] = arr[end];
        arr[end] = tmp;
        sift_down(0, end - 1);
    }
}

size_t manta_cpp_fast_compress(const uint8_t* src, size_t src_len, uint8_t* dst, size_t dst_cap) {
    if (!src || !dst || src_len == 0 || dst_cap < 5) return 0;

    write_u32_le(dst, static_cast<uint32_t>(src_len));
    size_t out_pos = 4;
    size_t in_pos = 0;
    size_t lit_start = 0;

    static constexpr uint32_t HASH_BITS = 10;
    static constexpr uint32_t HASH_SIZE = 1u << HASH_BITS;
    uint32_t ht[HASH_SIZE];
    for (uint32_t h = 0; h < HASH_SIZE; ++h) ht[h] = UINT32_MAX;

    auto flush_literals = [&](size_t end_pos) -> bool {
        size_t pos = lit_start;
        while (pos < end_pos) {
            size_t chunk = end_pos - pos;
            if (chunk > 128) chunk = 128;
            if (out_pos + 1 + chunk > dst_cap) return false;
            dst[out_pos++] = static_cast<uint8_t>(chunk - 1);
            ::memcpy(dst + out_pos, src + pos, chunk);
            out_pos += chunk;
            pos += chunk;
        }
        lit_start = end_pos;
        return true;
    };

    while (in_pos < src_len) {
        size_t run = 1;
        while (in_pos + run < src_len && src[in_pos + run] == src[in_pos] && run < 66) {
            ++run;
        }
        if (run >= 4) {
            if (!flush_literals(in_pos)) return 0;
            if (out_pos + 2 > dst_cap) return 0;
            dst[out_pos++] = static_cast<uint8_t>(0x80u | static_cast<uint8_t>(run - 3));
            dst[out_pos++] = src[in_pos];
            in_pos += run;
            lit_start = in_pos;
            continue;
        }

        if (in_pos + 4 <= src_len) {
            uint32_t v = read_u32_le(src + in_pos);
            uint32_t h = ((v * 2654435761u) >> (32 - HASH_BITS)) & (HASH_SIZE - 1u);
            uint32_t ref = ht[h];
            ht[h] = static_cast<uint32_t>(in_pos);

            if (ref != UINT32_MAX && in_pos > ref && (in_pos - ref) <= 65535u) {
                size_t dist = in_pos - ref;
                size_t match_len = 0;
                while (in_pos + match_len < src_len && match_len < 67 &&
                       src[ref + match_len] == src[in_pos + match_len]) {
                    ++match_len;
                }
                if (match_len >= 4) {
                    if (!flush_literals(in_pos)) return 0;
                    if (out_pos + 3 > dst_cap) return 0;
                    dst[out_pos++] = static_cast<uint8_t>(0xC0u | static_cast<uint8_t>(match_len - 4));
                    dst[out_pos++] = static_cast<uint8_t>(dist & 0xFFu);
                    dst[out_pos++] = static_cast<uint8_t>((dist >> 8) & 0xFFu);
                    in_pos += match_len;
                    lit_start = in_pos;
                    continue;
                }
            }
        }
        ++in_pos;
    }

    if (!flush_literals(src_len)) return 0;
    return out_pos;
}

size_t manta_cpp_fast_decompress(const uint8_t* src, size_t src_len, uint8_t* dst, size_t dst_cap) {
    if (!src || !dst || src_len < 4) return 0;
    uint32_t orig_len = read_u32_le(src);
    if (orig_len == 0 || orig_len > dst_cap) return 0;

    size_t in_pos = 4;
    size_t out_pos = 0;

    while (in_pos < src_len && out_pos < orig_len) {
        uint8_t tag = src[in_pos++];
        if (tag < 0x80u) {
            size_t count = static_cast<size_t>(tag & 0x7Fu) + 1u;
            if (in_pos + count > src_len || out_pos + count > orig_len) return 0;
            ::memcpy(dst + out_pos, src + in_pos, count);
            in_pos += count;
            out_pos += count;
        } else if ((tag & 0xC0u) == 0x80u) {
            size_t count = static_cast<size_t>(tag & 0x3Fu) + 3u;
            if (in_pos >= src_len || out_pos + count > orig_len) return 0;
            uint8_t val = src[in_pos++];
            ::memset(dst + out_pos, val, count);
            out_pos += count;
        } else {
            size_t count = static_cast<size_t>(tag & 0x3Fu) + 4u;
            if (in_pos + 2 > src_len || out_pos + count > orig_len) return 0;
            size_t dist = static_cast<size_t>(src[in_pos]) |
                          (static_cast<size_t>(src[in_pos + 1]) << 8);
            in_pos += 2;
            if (dist == 0 || dist > out_pos) return 0;
            for (size_t k = 0; k < count; ++k) {
                dst[out_pos] = dst[out_pos - dist];
                ++out_pos;
            }
        }
    }

    return (out_pos == orig_len) ? out_pos : 0;
}

struct MantaRingBuffer {
    alignas(64) uint32_t head;
    alignas(64) uint32_t tail;
    uint32_t capacity;
    uint32_t mask;
    uint8_t* data;
};

MantaRingBuffer* manta_cpp_ring_create(uint32_t capacity_pow2) {
    uint32_t cap = capacity_pow2;
    if (cap < 4096u) cap = 4096u;
    if ((cap & (cap - 1u)) != 0u) {
        uint32_t p = 4096u;
        while (p < cap && p < (1u << 28)) p <<= 1u;
        cap = p;
    }
    MantaRingBuffer* rb = static_cast<MantaRingBuffer*>(::calloc(1, sizeof(MantaRingBuffer)));
    if (!rb) return nullptr;
    uint8_t* buf = static_cast<uint8_t*>(manta_cpp_arena_alloc(cap));
    if (!buf) {
        ::free(rb);
        return nullptr;
    }
    rb->head = 0;
    rb->tail = 0;
    rb->capacity = cap;
    rb->mask = cap - 1u;
    rb->data = buf;
    return rb;
}

void manta_cpp_ring_destroy(MantaRingBuffer* rb) {
    if (!rb) return;
    if (rb->data) {
        manta_cpp_arena_free(rb->data, rb->capacity);
        rb->data = nullptr;
    }
    ::free(rb);
}

uint32_t manta_cpp_ring_write(MantaRingBuffer* rb, const uint8_t* data, uint32_t len) {
    if (!rb || !data || len == 0) return 0;
    const uint32_t h = __atomic_load_n(&rb->head, __ATOMIC_RELAXED);
    const uint32_t t = __atomic_load_n(&rb->tail, __ATOMIC_ACQUIRE);
    const uint32_t free_space = rb->capacity - (h - t);
    const uint32_t to_write = (len < free_space) ? len : free_space;
    if (to_write == 0) return 0;

    const uint32_t idx = h & rb->mask;
    const uint32_t first = rb->capacity - idx;
    if (to_write <= first) {
        ::memcpy(rb->data + idx, data, to_write);
    } else {
        ::memcpy(rb->data + idx, data, first);
        ::memcpy(rb->data, data + first, to_write - first);
    }
    __atomic_store_n(&rb->head, h + to_write, __ATOMIC_RELEASE);
    return to_write;
}

uint32_t manta_cpp_ring_read(MantaRingBuffer* rb, uint8_t* out, uint32_t max_len) {
    if (!rb || !out || max_len == 0) return 0;
    const uint32_t t = __atomic_load_n(&rb->tail, __ATOMIC_RELAXED);
    const uint32_t h = __atomic_load_n(&rb->head, __ATOMIC_ACQUIRE);
    const uint32_t avail = h - t;
    const uint32_t to_read = (max_len < avail) ? max_len : avail;
    if (to_read == 0) return 0;

    const uint32_t idx = t & rb->mask;
    const uint32_t first = rb->capacity - idx;
    if (to_read <= first) {
        ::memcpy(out, rb->data + idx, to_read);
    } else {
        ::memcpy(out, rb->data + idx, first);
        ::memcpy(out + first, rb->data, to_read - first);
    }
    __atomic_store_n(&rb->tail, t + to_read, __ATOMIC_RELEASE);
    return to_read;
}

uint32_t manta_cpp_ring_available_read(const MantaRingBuffer* rb) {
    if (!rb) return 0;
    const uint32_t h = __atomic_load_n(&rb->head, __ATOMIC_ACQUIRE);
    const uint32_t t = __atomic_load_n(&rb->tail, __ATOMIC_ACQUIRE);
    return h - t;
}

uint32_t manta_cpp_ring_capacity(const MantaRingBuffer* rb) {
    return rb ? rb->capacity : 0;
}

void manta_cpp_ring_reset(MantaRingBuffer* rb) {
    if (!rb) return;
    __atomic_store_n(&rb->head, 0u, __ATOMIC_RELEASE);
    __atomic_store_n(&rb->tail, 0u, __ATOMIC_RELEASE);
    if (rb->data) {
        manta_cpp_arena_release_pages(rb->data, 0, rb->capacity);
    }
}

void manta_cpp_blend_rect_argb(uint32_t* fb, uint32_t fb_w, uint32_t fb_h,
                               int32_t x, int32_t y, int32_t w, int32_t h, uint32_t argb) {
    if (!fb || fb_w == 0 || fb_h == 0 || w <= 0 || h <= 0) return;
    uint32_t a = (argb >> 24) & 0xFFu;
    if (a == 0) return;

    int32_t x0 = (x < 0) ? 0 : x;
    int32_t y0 = (y < 0) ? 0 : y;
    int64_t x1_64 = static_cast<int64_t>(x) + w;
    int64_t y1_64 = static_cast<int64_t>(y) + h;
    int32_t x1 = (x1_64 > static_cast<int64_t>(fb_w)) ? static_cast<int32_t>(fb_w) : static_cast<int32_t>(x1_64);
    int32_t y1 = (y1_64 > static_cast<int64_t>(fb_h)) ? static_cast<int32_t>(fb_h) : static_cast<int32_t>(y1_64);
    if (x0 >= x1 || y0 >= y1) return;

    if (a == 255u) {
        for (int32_t row = y0; row < y1; ++row) {
            uint32_t* dst = fb + static_cast<size_t>(row) * fb_w;
            for (int32_t col = x0; col < x1; ++col) {
                dst[col] = argb;
            }
        }
        return;
    }

    uint32_t sr = (argb >> 16) & 0xFFu;
    uint32_t sg = (argb >> 8) & 0xFFu;
    uint32_t sb = argb & 0xFFu;
    uint32_t inv_a = 255u - a;

    for (int32_t row = y0; row < y1; ++row) {
        uint32_t* dst = fb + static_cast<size_t>(row) * fb_w;
        for (int32_t col = x0; col < x1; ++col) {
            uint32_t d = dst[col];
            uint32_t dr = (d >> 16) & 0xFFu;
            uint32_t dg = (d >> 8) & 0xFFu;
            uint32_t db = d & 0xFFu;
            uint32_t r = (sr * a + dr * inv_a) / 255u;
            uint32_t g = (sg * a + dg * inv_a) / 255u;
            uint32_t b = (sb * a + db * inv_a) / 255u;
            dst[col] = 0xFF000000u | (r << 16) | (g << 8) | b;
        }
    }
}

struct MantaTlsHandle {
    void* ssl;
    void* ctx;
};

#if MANTA_HAS_POSIX
namespace {

typedef void* (*FnTlsMethod)(void);
typedef void* (*FnSslCtxNew)(void*);
typedef void  (*FnSslCtxFree)(void*);
typedef int   (*FnSslCtxDefaultPaths)(void*);
typedef void  (*FnSslCtxSetVerify)(void*, int, void*);
typedef long  (*FnSslCtxCtrl)(void*, int, long, void*);
typedef void* (*FnSslNew)(void*);
typedef void  (*FnSslFree)(void*);
typedef int   (*FnSslSetFd)(void*, int);
typedef int   (*FnSslSet1Host)(void*, const char*);
typedef long  (*FnSslCtrl)(void*, int, long, void*);
typedef int   (*FnSslConnect)(void*);
typedef int   (*FnSslWrite)(void*, const void*, int);
typedef int   (*FnSslRead)(void*, void*, int);
typedef int   (*FnSslShutdown)(void*);
typedef long  (*FnSslGetVerifyResult)(const void*);

struct OpenSslApi {
    bool loaded;
    bool attempted;
    void* lib;
    FnTlsMethod tls_client_method;
    FnSslCtxNew ssl_ctx_new;
    FnSslCtxFree ssl_ctx_free;
    FnSslCtxDefaultPaths ssl_ctx_default_paths;
    FnSslCtxSetVerify ssl_ctx_set_verify;
    FnSslCtxCtrl ssl_ctx_ctrl;
    FnSslNew ssl_new;
    FnSslFree ssl_free;
    FnSslSetFd ssl_set_fd;
    FnSslSet1Host ssl_set1_host;
    FnSslCtrl ssl_ctrl;
    FnSslConnect ssl_connect;
    FnSslWrite ssl_write;
    FnSslRead ssl_read;
    FnSslShutdown ssl_shutdown;
    FnSslGetVerifyResult ssl_get_verify_result;
};

OpenSslApi g_ssl = {};

bool ensure_openssl_loaded() {
    if (g_ssl.attempted) return g_ssl.loaded;
    g_ssl.attempted = true;

    const char* candidates[] = {"libssl.so.3", "libssl.so.1.1", "libssl.so"};
    for (int i = 0; i < 3; ++i) {
        g_ssl.lib = ::dlopen(candidates[i], RTLD_NOW | RTLD_LOCAL);
        if (g_ssl.lib) break;
    }
    if (!g_ssl.lib) return false;

    g_ssl.tls_client_method = reinterpret_cast<FnTlsMethod>(::dlsym(g_ssl.lib, "TLS_client_method"));
    g_ssl.ssl_ctx_new = reinterpret_cast<FnSslCtxNew>(::dlsym(g_ssl.lib, "SSL_CTX_new"));
    g_ssl.ssl_ctx_free = reinterpret_cast<FnSslCtxFree>(::dlsym(g_ssl.lib, "SSL_CTX_free"));
    g_ssl.ssl_ctx_default_paths = reinterpret_cast<FnSslCtxDefaultPaths>(::dlsym(g_ssl.lib, "SSL_CTX_set_default_verify_paths"));
    g_ssl.ssl_ctx_set_verify = reinterpret_cast<FnSslCtxSetVerify>(::dlsym(g_ssl.lib, "SSL_CTX_set_verify"));
    g_ssl.ssl_ctx_ctrl = reinterpret_cast<FnSslCtxCtrl>(::dlsym(g_ssl.lib, "SSL_CTX_ctrl"));
    g_ssl.ssl_new = reinterpret_cast<FnSslNew>(::dlsym(g_ssl.lib, "SSL_new"));
    g_ssl.ssl_free = reinterpret_cast<FnSslFree>(::dlsym(g_ssl.lib, "SSL_free"));
    g_ssl.ssl_set_fd = reinterpret_cast<FnSslSetFd>(::dlsym(g_ssl.lib, "SSL_set_fd"));
    g_ssl.ssl_set1_host = reinterpret_cast<FnSslSet1Host>(::dlsym(g_ssl.lib, "SSL_set1_host"));
    g_ssl.ssl_ctrl = reinterpret_cast<FnSslCtrl>(::dlsym(g_ssl.lib, "SSL_ctrl"));
    g_ssl.ssl_connect = reinterpret_cast<FnSslConnect>(::dlsym(g_ssl.lib, "SSL_connect"));
    g_ssl.ssl_write = reinterpret_cast<FnSslWrite>(::dlsym(g_ssl.lib, "SSL_write"));
    g_ssl.ssl_read = reinterpret_cast<FnSslRead>(::dlsym(g_ssl.lib, "SSL_read"));
    g_ssl.ssl_shutdown = reinterpret_cast<FnSslShutdown>(::dlsym(g_ssl.lib, "SSL_shutdown"));
    g_ssl.ssl_get_verify_result = reinterpret_cast<FnSslGetVerifyResult>(::dlsym(g_ssl.lib, "SSL_get_verify_result"));

    g_ssl.loaded = g_ssl.tls_client_method && g_ssl.ssl_ctx_new && g_ssl.ssl_ctx_free &&
                   g_ssl.ssl_ctx_default_paths && g_ssl.ssl_ctx_set_verify &&
                   g_ssl.ssl_new && g_ssl.ssl_free && g_ssl.ssl_set_fd &&
                   g_ssl.ssl_set1_host && g_ssl.ssl_ctrl && g_ssl.ssl_connect &&
                   g_ssl.ssl_write && g_ssl.ssl_read && g_ssl.ssl_shutdown &&
                   g_ssl.ssl_get_verify_result;
    return g_ssl.loaded;
}

} // namespace

int32_t manta_cpp_tls_is_available(void) {
    return ensure_openssl_loaded() ? 1 : 0;
}

void* manta_cpp_tls_connect(int32_t sock_fd, const char* hostname) {
    if (sock_fd < 0 || !hostname || hostname[0] == '\0') return nullptr;
    if (!ensure_openssl_loaded()) return nullptr;

    void* method = g_ssl.tls_client_method();
    if (!method) return nullptr;

    void* ctx = g_ssl.ssl_ctx_new(method);
    if (!ctx) return nullptr;

    if (g_ssl.ssl_ctx_default_paths(ctx) != 1) {
        g_ssl.ssl_ctx_free(ctx);
        return nullptr;
    }

    if (g_ssl.ssl_ctx_ctrl) {
        g_ssl.ssl_ctx_ctrl(ctx, 123, 0x0303L, nullptr);
    }
    g_ssl.ssl_ctx_set_verify(ctx, 1, nullptr);

    void* ssl = g_ssl.ssl_new(ctx);
    if (!ssl) {
        g_ssl.ssl_ctx_free(ctx);
        return nullptr;
    }

    if (g_ssl.ssl_set1_host(ssl, hostname) != 1) {
        g_ssl.ssl_free(ssl);
        g_ssl.ssl_ctx_free(ctx);
        return nullptr;
    }
    g_ssl.ssl_ctrl(ssl, 55, 0L, const_cast<char*>(hostname));

    if (g_ssl.ssl_set_fd(ssl, sock_fd) != 1 || g_ssl.ssl_connect(ssl) != 1) {
        g_ssl.ssl_free(ssl);
        g_ssl.ssl_ctx_free(ctx);
        return nullptr;
    }

    if (g_ssl.ssl_get_verify_result(ssl) != 0L) {
        g_ssl.ssl_shutdown(ssl);
        g_ssl.ssl_free(ssl);
        g_ssl.ssl_ctx_free(ctx);
        return nullptr;
    }

    MantaTlsHandle* h = static_cast<MantaTlsHandle*>(::calloc(1, sizeof(MantaTlsHandle)));
    if (!h) {
        g_ssl.ssl_shutdown(ssl);
        g_ssl.ssl_free(ssl);
        g_ssl.ssl_ctx_free(ctx);
        return nullptr;
    }
    h->ssl = ssl;
    h->ctx = ctx;
    return h;
}

int32_t manta_cpp_tls_write(void* tls_handle, const uint8_t* data, int32_t len) {
    if (!tls_handle || !data || len <= 0 || !g_ssl.loaded) return -1;
    MantaTlsHandle* h = static_cast<MantaTlsHandle*>(tls_handle);
    return g_ssl.ssl_write(h->ssl, data, len);
}

int32_t manta_cpp_tls_read(void* tls_handle, uint8_t* out, int32_t cap) {
    if (!tls_handle || !out || cap <= 0 || !g_ssl.loaded) return -1;
    MantaTlsHandle* h = static_cast<MantaTlsHandle*>(tls_handle);
    return g_ssl.ssl_read(h->ssl, out, cap);
}

void manta_cpp_tls_close(void* tls_handle) {
    if (!tls_handle) return;
    MantaTlsHandle* h = static_cast<MantaTlsHandle*>(tls_handle);
    if (g_ssl.loaded && h->ssl) {
        g_ssl.ssl_shutdown(h->ssl);
        g_ssl.ssl_free(h->ssl);
    }
    if (g_ssl.loaded && h->ctx) {
        g_ssl.ssl_ctx_free(h->ctx);
    }
    ::free(h);
}

#else

int32_t manta_cpp_tls_is_available(void) { return 0; }
void*   manta_cpp_tls_connect(int32_t, const char*) { return nullptr; }
int32_t manta_cpp_tls_write(void*, const uint8_t*, int32_t) { return -1; }
int32_t manta_cpp_tls_read(void*, uint8_t*, int32_t) { return -1; }
void    manta_cpp_tls_close(void*) {}

#endif

} // extern "C"
