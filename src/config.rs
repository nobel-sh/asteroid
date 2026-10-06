use crate::{Error, Result};

#[derive(Clone, Debug)]
pub struct Options {
    /// Flush after a write brings the WAL to at least this many bytes
    pub wal_size_limit: u64,
    /// Compact after a flush leaves at least this many SSTables
    pub compaction_trigger: usize,
    /// Bits per key for Bloom filters
    pub bloom_bits_per_key: Option<u8>,
    /// Budget for admitted decoded blocks
    pub block_cache_bytes: Option<usize>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            wal_size_limit: 4 * 1024 * 1024,
            compaction_trigger: 4,
            bloom_bits_per_key: Some(10),
            block_cache_bytes: Some(16 * 1024 * 1024),
        }
    }
}

impl Options {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.wal_size_limit == 0 {
            return Err(Error::InvalidOptions("wal_size_limit must be positive"));
        }
        if self.compaction_trigger < 2 {
            return Err(Error::InvalidOptions(
                "compaction_trigger must be at least two",
            ));
        }
        if self
            .bloom_bits_per_key
            .is_some_and(|bits| !(1..=64).contains(&bits))
        {
            return Err(Error::InvalidOptions(
                "bloom_bits_per_key must be between 1 and 64 when enabled",
            ));
        }
        if self.block_cache_bytes == Some(0) {
            return Err(Error::InvalidOptions(
                "block_cache_bytes must be positive when enabled",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WriteOptions {
    /// Wait for WAL durability before returning. If false, recent writes can be
    /// lost after a machine crash until synced or durably flushed.
    pub sync: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self { sync: true }
    }
}

#[cfg(test)]
mod tests {
    use super::Options;

    #[test]
    fn validates_options() {
        let defaults = Options::default();
        assert!(defaults.validate().is_ok());
        for options in [
            Options {
                wal_size_limit: 0,
                ..defaults.clone()
            },
            Options {
                compaction_trigger: 1,
                ..defaults.clone()
            },
            Options {
                bloom_bits_per_key: Some(0),
                ..defaults.clone()
            },
            Options {
                bloom_bits_per_key: Some(65),
                ..defaults.clone()
            },
            Options {
                block_cache_bytes: Some(0),
                ..defaults.clone()
            },
        ] {
            assert!(options.validate().is_err());
        }
    }
}
