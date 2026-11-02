use std::collections::HashMap;

/// Fast, 100% safe Rust in-memory buffer for transient tab snapshots.
/// Uses O(1) memory reclamation without slow volatile byte-by-byte wipe loops.
pub struct EphemeralBuffer {
    data: Vec<u8>,
}

impl EphemeralBuffer {
    pub fn from_slice(slice: &[u8]) -> Self {
        let mut data = Vec::with_capacity(slice.len());
        data.extend_from_slice(slice);
        Self { data }
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn release(&mut self) {
        self.data.clear();
        self.data.shrink_to_fit();
    }
}

/// High-speed in-memory RAM pool for compressed background tab states and transient buffers.
/// Strictly capped at `max_bytes` with automatic FIFO/LRU eviction.
pub struct VolatileRamVault {
    entries: HashMap<u64, (u32, EphemeralBuffer)>, // key_hash -> (tab_id, buffer)
    insertion_order: Vec<u64>,
    max_bytes: usize,
    current_bytes: usize,
}

impl VolatileRamVault {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            insertion_order: Vec::new(),
            max_bytes,
            current_bytes: 0,
        }
    }

    #[inline]
    pub fn used_bytes(&self) -> usize {
        self.current_bytes
    }

    #[inline]
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Fast FNV-1a 64-bit hash key.
    pub fn hash_key(tab_id: u32, key: &str) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325 ^ (tab_id as u64);
        for b in key.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    pub fn put_ephemeral(&mut self, tab_id: u32, key: &str, payload: &[u8]) -> bool {
        if payload.len() > self.max_bytes / 2 {
            return false;
        }

        let key_hash = Self::hash_key(tab_id, key);
        self.remove_by_hash(key_hash);

        while self.current_bytes + payload.len() > self.max_bytes
            && !self.insertion_order.is_empty()
        {
            let oldest = self.insertion_order.remove(0);
            self.remove_by_hash(oldest);
        }

        let buf = EphemeralBuffer::from_slice(payload);
        self.current_bytes += buf.len();
        self.entries.insert(key_hash, (tab_id, buf));
        self.insertion_order.push(key_hash);
        true
    }

    pub fn get_ephemeral(&self, tab_id: u32, key: &str) -> Option<&[u8]> {
        let key_hash = Self::hash_key(tab_id, key);
        self.entries.get(&key_hash).map(|(_, buf)| buf.as_slice())
    }

    pub fn purge_tab(&mut self, tab_id: u32) {
        let mut to_remove = Vec::new();
        for (&k, (owner_tab, _)) in &self.entries {
            if *owner_tab == tab_id {
                to_remove.push(k);
            }
        }
        for k in to_remove {
            self.remove_by_hash(k);
        }
        self.insertion_order.retain(|k| self.entries.contains_key(k));
    }

    pub fn clear_all(&mut self) {
        for (_, (_, mut buf)) in self.entries.drain() {
            buf.release();
        }
        self.insertion_order.clear();
        self.insertion_order.shrink_to_fit();
        self.entries.shrink_to_fit();
        self.current_bytes = 0;
    }

    fn remove_by_hash(&mut self, key_hash: u64) {
        if let Some((_, mut buf)) = self.entries.remove(&key_hash) {
            self.current_bytes = self.current_bytes.saturating_sub(buf.len());
            buf.release();
        }
    }
}
