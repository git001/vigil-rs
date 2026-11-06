// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Content hashes to detect changed files (`watch-files`, `--watch-layers`,
//! re-rendered templates).

use std::hash::Hasher;

/// Hash used to detect content changes (not cryptographic; only compared
/// with earlier hashes within one process). Feeding the same bytes in any
/// chunks gives the same hash, so a streamed file matches [`content_hash`]
/// of its content.
#[derive(Default)]
pub struct ContentHasher(std::collections::hash_map::DefaultHasher);

impl ContentHasher {
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    pub fn finish(&self) -> u64 {
        self.0.finish()
    }
}

impl std::io::Write for ContentHasher {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h = ContentHasher::default();
    h.update(bytes);
    h.finish()
}

/// Feed everything `reader` yields into `h` (`Interrupted` is retried).
pub fn hash_into(h: &mut ContentHasher, reader: &mut impl std::io::Read) -> std::io::Result<()> {
    std::io::copy(reader, h).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_hash_equals_whole_hash() {
        let data = vec![7u8; 100_000];
        let mut h = ContentHasher::default();
        hash_into(&mut h, &mut &data[..]).unwrap();
        assert_eq!(h.finish(), content_hash(&data));
    }
}
