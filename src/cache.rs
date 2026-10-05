use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use crate::table::DataBlock;

type CacheKey = (u32, u64);

#[derive(Debug)]
struct CachedBlock {
    block: Arc<DataBlock>,
    stamp: u64,
    charge: usize,
}

#[derive(Debug, Default)]
struct State {
    blocks: HashMap<CacheKey, CachedBlock>,
    order: BTreeMap<u64, CacheKey>,
    clock: u64,
    bytes: usize,
}

impl State {
    fn stamp(&mut self) -> u64 {
        if self.clock == u64::MAX {
            self.blocks.clear();
            self.order.clear();
            self.bytes = 0;
            self.clock = 0;
        }
        self.clock += 1;
        self.clock
    }
}

#[derive(Debug, Default)]
pub(crate) struct BlockCache {
    capacity: Option<usize>,
    state: Mutex<State>,
}

impl BlockCache {
    pub(crate) fn new(capacity: Option<usize>) -> Self {
        Self {
            capacity,
            ..Self::default()
        }
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = State::default();
        }
    }

    pub(crate) fn get(&self, table_id: u32, offset: u64) -> Option<Arc<DataBlock>> {
        self.capacity?;
        let key = (table_id, offset);
        let mut state = self.state.lock().ok()?;
        let stamp = state.stamp();
        let entry = state.blocks.get_mut(&key)?;
        let old_stamp = entry.stamp;
        entry.stamp = stamp;
        let block = Arc::clone(&entry.block);
        state.order.remove(&old_stamp);
        state.order.insert(stamp, key);
        Some(block)
    }

    pub(crate) fn insert(&self, table_id: u32, offset: u64, block: Arc<DataBlock>) {
        let Some(capacity) = self.capacity else {
            return;
        };
        let charge = block.memory_usage();
        if charge > capacity {
            return;
        }
        let key = (table_id, offset);
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let stamp = state.stamp();
        if state.blocks.contains_key(&key) {
            return;
        }
        while state.bytes > capacity - charge {
            let (_, oldest) = state.order.pop_first().unwrap();
            let entry = state.blocks.remove(&oldest).unwrap();
            state.bytes -= entry.charge;
        }
        state.blocks.insert(
            key,
            CachedBlock {
                block,
                stamp,
                charge,
            },
        );
        state.order.insert(stamp, key);
        state.bytes += charge;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_operations() {
        let block = Arc::new(DataBlock {
            entries: Vec::new(),
        });
        let cache = BlockCache::new(Some(2 * block.memory_usage()));
        cache.insert(1, 0, Arc::clone(&block));
        cache.insert(1, 4096, Arc::clone(&block));
        assert!(Arc::ptr_eq(&cache.get(1, 0).unwrap(), &block));

        cache.insert(2, 0, Arc::clone(&block));
        assert!(cache.get(1, 4096).is_none());
        assert!(cache.get(1, 0).is_some());
        assert!(cache.get(2, 0).is_some());
        assert!(cache.state.lock().unwrap().bytes <= cache.capacity.unwrap());

        cache.clear();
        assert!(cache.get(1, 0).is_none());
        assert!(cache.get(2, 0).is_none());
        assert_eq!(cache.state.lock().unwrap().bytes, 0);

        let disabled = BlockCache::new(None);
        disabled.insert(1, 0, block);
        assert!(disabled.get(1, 0).is_none());
    }
}
