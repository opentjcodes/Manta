use std::time::{SystemTime, UNIX_EPOCH};

use crate::cpp_bridge::secure_zero_slice;

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const SHA256_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// Pure-Rust FIPS 180-4 SHA-256 digest.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state = SHA256_INIT;
    let bit_len = (data.len() as u64).wrapping_mul(8);

    let full_blocks = data.len() / 64;
    for blk_idx in 0..full_blocks {
        sha256_compress(&mut state, &data[blk_idx * 64..(blk_idx + 1) * 64]);
    }

    let rem = &data[full_blocks * 64..];
    let mut pad = [0u8; 128];
    pad[..rem.len()].copy_from_slice(rem);
    pad[rem.len()] = 0x80;

    let pad_len = if rem.len() + 1 + 8 <= 64 { 64 } else { 128 };
    pad[pad_len - 8..pad_len].copy_from_slice(&bit_len.to_be_bytes());

    sha256_compress(&mut state, &pad[..64]);
    if pad_len == 128 {
        sha256_compress(&mut state, &pad[64..128]);
    }

    let mut out = [0u8; 32];
    for (i, &w) in state.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&w.to_be_bytes());
    }
    out
}

#[inline]
fn sha256_compress(state: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(SHA256_K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(maj);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// RFC 2104 / RFC 4231 HMAC-SHA256.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k_pad = [0u8; 64];
    if key.len() > 64 {
        let hashed = sha256(key);
        k_pad[..32].copy_from_slice(&hashed);
    } else {
        k_pad[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k_pad[i];
        opad[i] ^= k_pad[i];
    }

    let mut inner = Vec::with_capacity(64 + data.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(data);
    let inner_hash = sha256(&inner);
    secure_zero_slice(&mut inner);

    let mut outer = [0u8; 96];
    outer[..64].copy_from_slice(&opad);
    outer[64..96].copy_from_slice(&inner_hash);
    let tag = sha256(&outer);
    secure_zero_slice(&mut k_pad);
    secure_zero_slice(&mut outer);
    tag
}

/// RFC 2898 / RFC 8018 PBKDF2-HMAC-SHA256 (32-byte key derivation).
pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let iters = iterations.max(1);
    let mut msg = Vec::with_capacity(salt.len() + 4);
    msg.extend_from_slice(salt);
    msg.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac_sha256(password, &msg);
    let mut out = u;
    for _ in 1..iters {
        u = hmac_sha256(password, &u);
        for i in 0..32 {
            out[i] ^= u[i];
        }
    }
    secure_zero_slice(&mut msg);
    out
}

#[inline]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);

    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut init = [0u32; 16];
    init[0] = 0x61707865;
    init[1] = 0x3320646e;
    init[2] = 0x79622d32;
    init[3] = 0x6b206574;

    for i in 0..8 {
        init[4 + i] = u32::from_le_bytes([
            key[i * 4],
            key[i * 4 + 1],
            key[i * 4 + 2],
            key[i * 4 + 3],
        ]);
    }
    init[12] = counter;
    for i in 0..3 {
        init[13 + i] = u32::from_le_bytes([
            nonce[i * 4],
            nonce[i * 4 + 1],
            nonce[i * 4 + 2],
            nonce[i * 4 + 3],
        ]);
    }

    let mut x = init;
    for _ in 0..10 {
        quarter_round(&mut x, 0, 4, 8, 12);
        quarter_round(&mut x, 1, 5, 9, 13);
        quarter_round(&mut x, 2, 6, 10, 14);
        quarter_round(&mut x, 3, 7, 11, 15);
        quarter_round(&mut x, 0, 5, 10, 15);
        quarter_round(&mut x, 1, 6, 11, 12);
        quarter_round(&mut x, 2, 7, 8, 13);
        quarter_round(&mut x, 3, 4, 9, 14);
    }

    let mut block = [0u8; 64];
    for i in 0..16 {
        let word = x[i].wrapping_add(init[i]);
        block[i * 4..(i + 1) * 4].copy_from_slice(&word.to_le_bytes());
    }
    block
}

/// RFC 8439 ChaCha20 XOR stream cipher.
pub fn chacha20_xor(key: &[u8; 32], nonce: &[u8; 12], initial_counter: u32, data: &mut [u8]) {
    let mut counter = initial_counter;
    let mut offset = 0usize;
    while offset < data.len() {
        let block = chacha20_block(key, counter, nonce);
        let take = (data.len() - offset).min(64);
        for i in 0..take {
            data[offset + i] ^= block[i];
        }
        offset += take;
        counter = counter.wrapping_add(1);
    }
}

#[inline]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn derive_subkeys(master_key: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let enc_key = hmac_sha256(master_key, b"manta_vault_enc_v4");
    let mac_key = hmac_sha256(master_key, b"manta_vault_mac_v4");
    (enc_key, mac_key)
}

/// Encrypt-then-MAC AEAD seal: outputs `nonce(12) || ciphertext(N) || hmac_sha256_tag(32)`.
pub fn seal_record(
    master_key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> Vec<u8> {
    let (mut enc_key, mut mac_key) = derive_subkeys(master_key);

    let mut ct = plaintext.to_vec();
    chacha20_xor(&enc_key, nonce, 1, &mut ct);

    let mut mac_input = Vec::with_capacity(aad.len() + 12 + 8 + ct.len());
    mac_input.extend_from_slice(aad);
    mac_input.extend_from_slice(nonce);
    mac_input.extend_from_slice(&(ct.len() as u64).to_le_bytes());
    mac_input.extend_from_slice(&ct);
    let tag = hmac_sha256(&mac_key, &mac_input);

    let mut out = Vec::with_capacity(12 + ct.len() + 32);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    out.extend_from_slice(&tag);

    secure_zero_slice(&mut enc_key);
    secure_zero_slice(&mut mac_key);
    secure_zero_slice(&mut mac_input);
    out
}

/// Verifies HMAC-SHA256 in constant time before decrypting ChaCha20 ciphertext.
pub fn open_record(
    master_key: &[u8; 32],
    aad: &[u8],
    sealed: &[u8],
) -> Result<Vec<u8>, &'static str> {
    if sealed.len() < 44 {
        return Err("Sealed record too short");
    }
    let nonce: [u8; 12] = sealed[0..12].try_into().unwrap();
    let ct_end = sealed.len() - 32;
    let ct = &sealed[12..ct_end];
    let expected_tag = &sealed[ct_end..];

    let (mut enc_key, mut mac_key) = derive_subkeys(master_key);

    let mut mac_input = Vec::with_capacity(aad.len() + 12 + 8 + ct.len());
    mac_input.extend_from_slice(aad);
    mac_input.extend_from_slice(&nonce);
    mac_input.extend_from_slice(&(ct.len() as u64).to_le_bytes());
    mac_input.extend_from_slice(ct);
    let actual_tag = hmac_sha256(&mac_key, &mac_input);
    secure_zero_slice(&mut mac_input);
    secure_zero_slice(&mut mac_key);

    if !constant_time_eq(&actual_tag, expected_tag) {
        secure_zero_slice(&mut enc_key);
        return Err("Encrypted vault integrity check failed (HMAC mismatch)");
    }

    let mut pt = ct.to_vec();
    chacha20_xor(&enc_key, &nonce, 1, &mut pt);
    secure_zero_slice(&mut enc_key);
    Ok(pt)
}

/// Generates 32 bytes of unpredictable entropy using OS `/dev/urandom` mixed with high-res clocks.
pub fn generate_volatile_entropy_32() -> [u8; 32] {
    let mut seed = [0u8; 64];

    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let _ = f.read_exact(&mut seed[..32]);
        }
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    seed[32..48].copy_from_slice(&nanos.to_le_bytes());
    let stack_addr = (&seed as *const [u8; 64] as usize) as u64;
    let pid = std::process::id() as u64;
    seed[48..56].copy_from_slice(&stack_addr.to_le_bytes());
    seed[56..64].copy_from_slice(&pid.to_le_bytes());

    let out = sha256(&seed);
    secure_zero_slice(&mut seed);
    out
}

pub fn generate_nonce_12(counter_hint: u64) -> [u8; 12] {
    let ent = generate_volatile_entropy_32();
    let mut buf = [0u8; 40];
    buf[..32].copy_from_slice(&ent);
    buf[32..40].copy_from_slice(&counter_hint.to_le_bytes());
    let h = sha256(&buf);
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&h[..12]);
    nonce
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fips_sha256_and_rfc8439_chacha20_and_aead_roundtrip() {
        let digest = sha256(b"abc");
        let expected_abc = [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ];
        assert_eq!(digest, expected_abc);

        // RFC 8439 Section 2.4.2 vector
        let mut key = [0u8; 32];
        for i in 0..32 {
            key[i] = i as u8;
        }
        let nonce = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00];
        let pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let mut ct = pt.to_vec();
        chacha20_xor(&key, &nonce, 1, &mut ct);
        assert_eq!(&ct[..4], &[0x6e, 0x2e, 0x35, 0x9a]);

        // AEAD seal/open + tamper rejection
        let sealed = seal_record(&key, &nonce, b"MANTADB4", b"secret session token");
        let opened = open_record(&key, b"MANTADB4", &sealed).unwrap();
        assert_eq!(&opened, b"secret session token");

        let mut tampered = sealed.clone();
        tampered[15] ^= 0xFF;
        assert!(open_record(&key, b"MANTADB4", &tampered).is_err());
    }
}
