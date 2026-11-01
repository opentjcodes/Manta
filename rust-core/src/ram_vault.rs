use std::collections::HashMap;
use std::ptr;
use std::sync::atomic::{compiler_fence, Ordering};

/// A volatile, zero-disk-footprint memory buffer that cryptographically zeroes
/// its contents on drop or explicit purge.
pub struct EphemeralBuffer {
    data: Vec<u8>,
}

impl EphemeralBuffer {
    pub fn from_slice(slice: &[u8]) -> Self {
        let mut data = Vec::with_capacity(slice.len());
        data.extend_from_slice(slice);
        Self { data }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Overwrites every byte in RAM with `0x00` using volatile writes so the compiler
    /// cannot optimize away the wipe.
    pub fn zeroize(&mut self) {
        for byte in &mut self.data {
            unsafe {
                ptr::write_volatile(byte, 0u8);
            }
        }
        compiler_fence(Ordering::SeqCst);
        self.data.clear();
        self.data.shrink_to_fit();
    }
}

impl Drop for EphemeralBuffer {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// Volatile RAM Vault: replaces traditional browser disk caches, SQLite cookie jars,
/// and local storage with a strict byte-capped in-memory map that auto-evicts oldest
/// entries and wipes itself to zero on tab/app close.
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

    /// Fast FNV-1a 64-bit hash so we don't even store URL strings in the vault keys.
    pub fn hash_key(tab_id: u32, key: &str) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325 ^ (tab_id as u64);
        for b in key.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    /// Stores transient data in RAM only if it fits within the quota, evicting oldest
    /// entries first. Never touches disk.
    pub fn put_ephemeral(&mut self, tab_id: u32, key: &str, payload: &[u8]) -> bool {
        if payload.len() > self.max_bytes / 2 {
            // Refuse oversized blobs to keep RAM flat.
            return false;
        }

        let key_hash = Self::hash_key(tab_id, key);
        self.remove_by_hash(key_hash);

        while self.current_bytes + payload.len() > self.max_bytes && !self.insertion_order.is_empty()
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

    /// Immediately zeroes and drops all RAM entries belonging to a specific closed tab.
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

    /// Cryptographically zeroes every buffer in the vault and shrinks allocations to 0.
    pub fn zeroize_and_purge(&mut self) {
        for (_, (_, mut buf)) in self.entries.drain() {
            buf.zeroize();
        }
        self.insertion_order.clear();
        self.insertion_order.shrink_to_fit();
        self.entries.shrink_to_fit();
        self.current_bytes = 0;
    }

    fn remove_by_hash(&mut self, key_hash: u64) {
        if let Some((_, mut buf)) = self.entries.remove(&key_hash) {
            self.current_bytes = self.current_bytes.saturating_sub(buf.len());
            buf.zeroize();
        }
    }
}
