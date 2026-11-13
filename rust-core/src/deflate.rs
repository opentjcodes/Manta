const MAX_BITS: usize = 15;

struct HuffmanTable {
    counts: [u16; MAX_BITS + 1],
    symbols: [u16; 320],
}

impl HuffmanTable {
    fn from_lengths(lengths: &[u8]) -> Result<Self, &'static str> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &len in lengths {
            let l = len as usize;
            if l > MAX_BITS {
                return Err("Invalid Huffman code length");
            }
            counts[l] += 1;
        }

        let mut offsets = [0u16; MAX_BITS + 1];
        for len in 1..MAX_BITS {
            offsets[len + 1] = offsets[len] + counts[len];
        }

        let mut symbols = [0u16; 320];
        for (sym, &len) in lengths.iter().enumerate() {
            if len != 0 {
                let idx = offsets[len as usize] as usize;
                if idx >= symbols.len() {
                    return Err("Huffman symbol table overflow");
                }
                symbols[idx] = sym as u16;
                offsets[len as usize] += 1;
            }
        }

        Ok(Self { counts, symbols })
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_buf: u32,
    bit_cnt: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_buf: 0,
            bit_cnt: 0,
        }
    }

    #[inline]
    fn ensure_bits(&mut self, n: u8) -> Result<(), &'static str> {
        while self.bit_cnt < n {
            if self.byte_pos >= self.data.len() {
                return Err("Unexpected end of deflate stream");
            }
            self.bit_buf |= (self.data[self.byte_pos] as u32) << self.bit_cnt;
            self.byte_pos += 1;
            self.bit_cnt += 8;
        }
        Ok(())
    }

    #[inline]
    fn read_bits(&mut self, n: u8) -> Result<u32, &'static str> {
        if n == 0 {
            return Ok(0);
        }
        self.ensure_bits(n)?;
        let val = self.bit_buf & ((1u32 << n) - 1);
        self.bit_buf >>= n;
        self.bit_cnt -= n;
        Ok(val)
    }

    #[inline]
    fn align_byte(&mut self) {
        self.bit_buf = 0;
        self.bit_cnt = 0;
    }

    fn read_u16_aligned(&mut self) -> Result<u16, &'static str> {
        self.align_byte();
        if self.byte_pos + 2 > self.data.len() {
            return Err("Truncated stored block");
        }
        let val = u16::from_le_bytes([self.data[self.byte_pos], self.data[self.byte_pos + 1]]);
        self.byte_pos += 2;
        Ok(val)
    }

    fn decode_symbol(&mut self, table: &HuffmanTable) -> Result<u16, &'static str> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for len in 1..=MAX_BITS {
            code |= self.read_bits(1)? as i32;
            let count = table.counts[len] as i32;
            if code - count < first {
                let sym_idx = (index + (code - first)) as usize;
                return table
                    .symbols
                    .get(sym_idx)
                    .copied()
                    .ok_or("Invalid Huffman symbol index");
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("Invalid Huffman code in deflate stream")
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn build_fixed_tables() -> (HuffmanTable, HuffmanTable) {
    let mut lit_lens = [0u8; 288];
    for item in lit_lens.iter_mut().take(144) {
        *item = 8;
    }
    for item in lit_lens.iter_mut().take(256).skip(144) {
        *item = 9;
    }
    for item in lit_lens.iter_mut().take(280).skip(256) {
        *item = 7;
    }
    for item in lit_lens.iter_mut().take(288).skip(280) {
        *item = 8;
    }
    let dist_lens = [5u8; 30];
    (
        HuffmanTable::from_lengths(&lit_lens).unwrap(),
        HuffmanTable::from_lengths(&dist_lens).unwrap(),
    )
}

fn decode_dynamic_tables(reader: &mut BitReader<'_>) -> Result<(HuffmanTable, HuffmanTable), &'static str> {
    let hlit = (reader.read_bits(5)? as usize) + 257;
    let hdist = (reader.read_bits(5)? as usize) + 1;
    let hclen = (reader.read_bits(4)? as usize) + 4;
    if hlit > 286 || hdist > 30 || hclen > 19 {
        return Err("Invalid dynamic Huffman header counts");
    }

    let mut cl_lens = [0u8; 19];
    for i in 0..hclen {
        cl_lens[CL_ORDER[i]] = reader.read_bits(3)? as u8;
    }
    let cl_table = HuffmanTable::from_lengths(&cl_lens)?;

    let total = hlit + hdist;
    let mut lengths = Vec::with_capacity(total);
    while lengths.len() < total {
        let sym = reader.decode_symbol(&cl_table)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let prev = *lengths.last().ok_or("Invalid repeat code 16")?;
                let repeat = (reader.read_bits(2)? as usize) + 3;
                for _ in 0..repeat {
                    if lengths.len() >= total {
                        return Err("Dynamic Huffman length overflow");
                    }
                    lengths.push(prev);
                }
            }
            17 => {
                let repeat = (reader.read_bits(3)? as usize) + 3;
                for _ in 0..repeat {
                    if lengths.len() >= total {
                        return Err("Dynamic Huffman length overflow");
                    }
                    lengths.push(0);
                }
            }
            18 => {
                let repeat = (reader.read_bits(7)? as usize) + 11;
                for _ in 0..repeat {
                    if lengths.len() >= total {
                        return Err("Dynamic Huffman length overflow");
                    }
                    lengths.push(0);
                }
            }
            _ => return Err("Invalid code-length symbol"),
        }
    }

    let lit_table = HuffmanTable::from_lengths(&lengths[..hlit])?;
    let dist_table = HuffmanTable::from_lengths(&lengths[hlit..])?;
    Ok((lit_table, dist_table))
}

/// Pure-Rust RFC 1951 Inflate with strict `max_out_bytes` zip-bomb defense.
pub fn inflate_raw_deflate(input: &[u8], max_out_bytes: usize) -> Result<Vec<u8>, &'static str> {
    let mut reader = BitReader::new(input);
    let mut out = Vec::with_capacity((input.len() * 2).min(max_out_bytes));

    loop {
        let bfinal = reader.read_bits(1)?;
        let btype = reader.read_bits(2)?;

        match btype {
            0 => {
                let len = reader.read_u16_aligned()? as usize;
                let nlen = reader.read_u16_aligned()?;
                if (len as u16) != !nlen {
                    return Err("Corrupt stored DEFLATE block");
                }
                if out.len() + len > max_out_bytes {
                    return Err("Decompressed payload exceeds max allowed bytes");
                }
                if reader.byte_pos + len > reader.data.len() {
                    return Err("Truncated stored DEFLATE data");
                }
                out.extend_from_slice(&reader.data[reader.byte_pos..reader.byte_pos + len]);
                reader.byte_pos += len;
            }
            1 | 2 => {
                let (lit_table, dist_table) = if btype == 1 {
                    build_fixed_tables()
                } else {
                    decode_dynamic_tables(&mut reader)?
                };

                loop {
                    let sym = reader.decode_symbol(&lit_table)?;
                    if sym < 256 {
                        if out.len() >= max_out_bytes {
                            return Err("Decompressed payload exceeds max allowed bytes");
                        }
                        out.push(sym as u8);
                    } else if sym == 256 {
                        break;
                    } else {
                        let len_idx = (sym - 257) as usize;
                        if len_idx >= LEN_BASE.len() {
                            return Err("Invalid DEFLATE length code");
                        }
                        let length =
                            LEN_BASE[len_idx] as usize + reader.read_bits(LEN_EXTRA[len_idx])? as usize;

                        let dist_sym = reader.decode_symbol(&dist_table)? as usize;
                        if dist_sym >= DIST_BASE.len() {
                            return Err("Invalid DEFLATE distance code");
                        }
                        let dist =
                            DIST_BASE[dist_sym] as usize + reader.read_bits(DIST_EXTRA[dist_sym])? as usize;

                        if dist == 0 || dist > out.len() {
                            return Err("Invalid DEFLATE back-reference distance");
                        }
                        if out.len() + length > max_out_bytes {
                            return Err("Decompressed payload exceeds max allowed bytes");
                        }
                        for _ in 0..length {
                            let b = out[out.len() - dist];
                            out.push(b);
                        }
                    }
                }
            }
            _ => return Err("Reserved DEFLATE block type"),
        }

        if bfinal == 1 {
            break;
        }
    }

    Ok(out)
}

/// Decodes RFC 1952 GZIP format (with optional FEXTRA/FNAME/FCOMMENT/FHCRC headers).
pub fn inflate_gzip(input: &[u8], max_out_bytes: usize) -> Result<Vec<u8>, &'static str> {
    if input.len() < 18 || input[0] != 0x1F || input[1] != 0x8B || input[2] != 8 {
        return Err("Invalid GZIP header");
    }
    let flags = input[3];
    let mut pos = 10usize;

    if (flags & 0x04) != 0 {
        if pos + 2 > input.len() {
            return Err("Truncated GZIP FEXTRA");
        }
        let xlen = u16::from_le_bytes([input[pos], input[pos + 1]]) as usize;
        pos += 2 + xlen;
    }
    if (flags & 0x08) != 0 {
        while pos < input.len() && input[pos] != 0 {
            pos += 1;
        }
        pos += 1;
    }
    if (flags & 0x10) != 0 {
        while pos < input.len() && input[pos] != 0 {
            pos += 1;
        }
        pos += 1;
    }
    if (flags & 0x02) != 0 {
        pos += 2;
    }

    if pos + 8 > input.len() {
        return Err("Truncated GZIP payload");
    }
    let deflate_slice = &input[pos..input.len() - 8];
    inflate_raw_deflate(deflate_slice, max_out_bytes)
}

/// Decodes RFC 1950 ZLIB stream (or falls back to raw DEFLATE).
pub fn inflate_zlib(input: &[u8], max_out_bytes: usize) -> Result<Vec<u8>, &'static str> {
    if input.len() >= 6 && (input[0] & 0x0F) == 8 {
        let cmf_flg = ((input[0] as u16) << 8) | (input[1] as u16);
        if cmf_flg % 31 == 0 {
            let fdict = (input[1] & 0x20) != 0;
            let start = if fdict { 6 } else { 2 };
            let end = input.len().saturating_sub(4);
            if start <= end {
                return inflate_raw_deflate(&input[start..end], max_out_bytes);
            }
        }
    }
    inflate_raw_deflate(input, max_out_bytes)
}

pub fn decompress_http_encoding(
    content_encoding: &str,
    data: &[u8],
    max_out_bytes: usize,
) -> Result<Vec<u8>, &'static str> {
    let enc = content_encoding.trim().to_ascii_lowercase();
    if enc.contains("gzip") {
        inflate_gzip(data, max_out_bytes)
    } else if enc.contains("deflate") {
        inflate_zlib(data, max_out_bytes)
    } else {
        if data.len() > max_out_bytes {
            return Err("Payload exceeds max allowed bytes");
        }
        Ok(data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inflate_stored_and_fixed_huffman_and_gzip() {
        // Real gzip stream for "Manta Fast Engine"
        // GZIP header (10 bytes) + stored deflate block ("Manta Fast Engine") + CRC + ISIZE
        let msg = b"Manta Fast Engine";
        let len = msg.len() as u16;
        let nlen = !len;
        let mut gz = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 3];
        gz.push(0x01); // BFINAL=1, BTYPE=00 (stored)
        gz.extend_from_slice(&len.to_le_bytes());
        gz.extend_from_slice(&nlen.to_le_bytes());
        gz.extend_from_slice(msg);
        gz.extend_from_slice(&[0u8; 8]);

        let decoded = inflate_gzip(&gz, 1024).unwrap();
        assert_eq!(&decoded, msg);

        // Zip-bomb max_out_bytes check
        assert!(inflate_gzip(&gz, 4).is_err());

        // Fixed Huffman deflate of "HelloHelloHello" -> [0xF3, 0x48, 0xCD, 0xC9, 0xC9, 0xF7, 0x00, 0x22, 0x99, 0x01, 0x00]
        let fixed = [0xF3, 0x48, 0xCD, 0xC9, 0xC9, 0xF7, 0x00, 0x22, 0x99, 0x01, 0x00];
        let out = inflate_raw_deflate(&fixed, 1024).unwrap();
        assert_eq!(&out, b"HelloHelloHello");
    }
}
