use crate::{
    codec::{Decoder, CHECKSUM_SIZE},
    Error, Result,
};

const VERSION: u8 = 2;

#[derive(Debug)]
pub(crate) struct BloomFilter {
    bits: Vec<u8>,
    probes: u8,
}

pub(crate) use xxhash_rust::xxh3::xxh3_64 as hash;

fn positions(hash: u64, probes: u8, bytes: usize) -> impl Iterator<Item = usize> {
    let step = hash.rotate_right(17) | 1;
    let bit_count = bytes as u64 * 8;
    (0..probes).map(move |probe| {
        (hash.wrapping_add(u64::from(probe).wrapping_mul(step)) % bit_count) as usize
    })
}

impl BloomFilter {
    pub(crate) fn build(hashes: &[u64], bits_per_key: u8) -> Result<Option<Self>> {
        if bits_per_key == 0 {
            return Ok(None);
        }
        let bit_count = hashes
            .len()
            .checked_mul(usize::from(bits_per_key))
            .ok_or(Error::RecordTooLarge)?
            .max(64);
        let mut filter = Self {
            bits: vec![0; bit_count.div_ceil(8)],
            probes: ((u16::from(bits_per_key) * 69 + 50) / 100).clamp(1, 30) as u8,
        };
        for &hash in hashes {
            for bit in positions(hash, filter.probes, filter.bits.len()) {
                filter.bits[bit / 8] |= 1 << (bit % 8);
            }
        }
        Ok(Some(filter))
    }

    pub(crate) fn may_contain(&self, key: &[u8]) -> bool {
        positions(hash(key), self.probes, self.bits.len())
            .all(|bit| self.bits[bit / 8] & (1 << (bit % 8)) != 0)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        let size = u32::try_from(self.bits.len()).map_err(|_| Error::RecordTooLarge)?;
        let mut bytes = vec![VERSION, self.probes];
        bytes.extend_from_slice(&size.to_be_bytes());
        bytes.extend_from_slice(&self.bits);
        bytes.extend_from_slice(&crc32fast::hash(&bytes).to_be_bytes());
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        let checksum_at = bytes
            .len()
            .checked_sub(CHECKSUM_SIZE)
            .ok_or_else(|| Error::CorruptSstable("truncated Bloom filter".into()))?;
        let checksum = u32::from_be_bytes(bytes[checksum_at..].try_into().unwrap());
        if crc32fast::hash(&bytes[..checksum_at]) != checksum {
            return Err(Error::CorruptSstable(
                "Bloom filter checksum mismatch".into(),
            ));
        }
        let mut decoder = Decoder::new(&bytes[..checksum_at]);
        let version = decoder.read_u8()?;
        let probes = decoder.read_u8()?;
        let size = decoder.read_u32()? as usize;
        if version != VERSION || !(1..=30).contains(&probes) || size < 8 {
            return Err(Error::CorruptSstable(
                "invalid Bloom filter parameters".into(),
            ));
        }
        let bits = decoder.take(size)?;
        decoder.finish()?;
        Ok(Self {
            bits: bits.to_vec(),
            probes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloom_filter_operations() {
        let hashes: Vec<_> = (0u64..100).map(|key| hash(&key.to_be_bytes())).collect();
        let filter = BloomFilter::build(&hashes, 10).unwrap().unwrap();
        let mut bytes = filter.encode().unwrap();
        let decoded = BloomFilter::decode(&bytes).unwrap();

        for key in 0u64..100 {
            assert!(filter.may_contain(&key.to_be_bytes()));
            assert!(decoded.may_contain(&key.to_be_bytes()));
        }
        assert!((100u64..200).any(|key| !decoded.may_contain(&key.to_be_bytes())));

        bytes[6] ^= 1;
        assert!(BloomFilter::decode(&bytes).is_err());
        assert!(BloomFilter::build(&hashes, 0).unwrap().is_none());
    }
}
