#[path = "engine.rs"]
mod engine;

use std::{fs::File, ops::RangeBounds, path::PathBuf, sync::Arc};

use crate::{
    cache::BlockCache, manifest::Manifest, memtable::Memtable, table::SSTable, wal::Wal,
    DbIterator, Key, Options, Result, Value, WriteBatch, WriteOptions,
};

#[derive(Debug)]
pub struct Db {
    wal: Wal,
    memtable: Memtable,
    manifest: Manifest,
    sstables: Vec<SSTable>,
    db_dir: PathBuf,
    _lockfile: File,
    options: Options,
    cache: Arc<BlockCache>,
}

impl Db {
    pub fn open(db_dir: PathBuf) -> Result<Self> {
        Self::open_with_options(db_dir, Options::default())
    }

    pub fn open_with_options(db_dir: PathBuf, options: Options) -> Result<Self> {
        Self::open_inner(db_dir, options)
    }

    pub fn get(&self, key: &Key) -> Result<Option<Value>> {
        self.get_inner(key)
    }

    /// Iterates over live entries in ascending byte order. Stops after the first error.
    pub fn iter(&self) -> DbIterator<'_> {
        self.range(..)
    }

    /// Scans live entries within the given bounds. Empty or reversed ranges yield nothing.
    /// The iterator borrows the database, preventing writes until it is dropped.
    pub fn range(&self, range: impl RangeBounds<Key>) -> DbIterator<'_> {
        DbIterator::new(&self.memtable, &self.sstables, range)
    }

    pub fn put(&mut self, key: impl Into<Key>, value: impl Into<Value>) -> Result<()> {
        self.put_with_options(key, value, WriteOptions::default())
    }

    pub fn put_with_options(
        &mut self,
        key: impl Into<Key>,
        value: impl Into<Value>,
        options: WriteOptions,
    ) -> Result<()> {
        let mut batch = WriteBatch::new();
        batch.put(key, value);
        self.write_with_options(batch, options)
    }

    pub fn delete(&mut self, key: impl Into<Key>) -> Result<()> {
        self.delete_with_options(key, WriteOptions::default())
    }

    pub fn delete_with_options(
        &mut self,
        key: impl Into<Key>,
        options: WriteOptions,
    ) -> Result<()> {
        let mut batch = WriteBatch::new();
        batch.delete(key);
        self.write_with_options(batch, options)
    }

    pub fn write(&mut self, batch: WriteBatch) -> Result<()> {
        self.write_with_options(batch, WriteOptions::default())
    }

    pub fn write_with_options(&mut self, batch: WriteBatch, options: WriteOptions) -> Result<()> {
        self.write_inner(batch, options)
    }

    /// Makes all preceding writes durable. Does not flush the memtable to an SSTable.
    pub fn sync_wal(&mut self) -> Result<()> {
        self.wal.sync()
    }

    pub fn last_sequence(&self) -> u64 {
        self.manifest.last_sequence
    }
}
