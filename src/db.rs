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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{Db, Options};
    use crate::{Error, Key, WriteBatch};

    fn path() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("db-{}-{unique}", std::process::id()))
    }

    #[test]
    fn searches_sstable_key_bounds() {
        use std::ops::Bound::{Excluded, Included, Unbounded};

        let path = path();
        let options = Options {
            wal_size_limit: 1,
            compaction_trigger: 4,
            bloom_bits_per_key: None,
            block_cache_bytes: None,
        };
        let mut db = Db::open_with_options(path.clone(), options.clone()).unwrap();
        let mut batch = WriteBatch::new();
        batch.put("dog", "old");
        batch.put("fox", "value");
        batch.put("zebra", "value");
        db.write(batch).unwrap();
        db.delete("dog").unwrap();
        drop(db);

        let db = Db::open_with_options(path.clone(), options).unwrap();
        for key in ["apple", "dog", "zz"] {
            assert!(db.get(&Key::from(key)).unwrap().is_none());
        }
        for key in ["fox", "zebra"] {
            assert!(db.get(&Key::from(key)).unwrap().is_some());
        }
        let scan = |start, end| {
            db.range((start, end))
                .map(|entry| entry.unwrap().0)
                .collect::<Vec<_>>()
        };
        assert!(scan(Unbounded, Excluded(Key::from("dog"))).is_empty());
        assert!(scan(Excluded(Key::from("zebra")), Unbounded).is_empty());
        assert_eq!(
            scan(Included(Key::from("fox")), Included(Key::from("zebra"))),
            [Key::from("fox"), Key::from("zebra")]
        );
        assert_eq!(
            scan(Excluded(Key::from("fox")), Included(Key::from("zebra"))),
            [Key::from("zebra")]
        );
        assert!(scan(Included(Key::from("dog")), Included(Key::from("dog"))).is_empty());
        drop(db);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn recovers_multiple_sstables_and_wal() {
        let path = path();
        let mut db = Db::open_with_options(
            path.clone(),
            Options {
                wal_size_limit: 4 * 1024,
                compaction_trigger: 2,
                ..Options::default()
            },
        )
        .unwrap();

        let mut first = WriteBatch::new();
        first.put("a", "old");
        first.put("b", "value");
        first.put("c", vec![0_u8; 4096]);
        first.put("d", "value");
        db.write(first).unwrap();

        let mut second = WriteBatch::new();
        second.put("a", "new");
        second.delete("b");
        second.put("e", vec![1_u8; 4096]);
        second.put("f", "value");
        db.write(second).unwrap();
        assert_eq!(db.sstables.len(), 1);
        let entries = db.sstables[0]
            .iter()
            .collect::<crate::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>(),
            ["a", "c", "d", "e", "f"].map(Key::from)
        );
        drop(db);

        let mut db = Db::open(path.clone()).unwrap();
        assert_eq!(db.get(&Key::from("a")).unwrap(), Some("new".into()));
        assert_eq!(db.get(&Key::from("b")).unwrap(), None);
        assert_eq!(
            db.get(&Key::from("c")).unwrap(),
            Some(vec![0_u8; 4096].into())
        );
        assert_eq!(
            db.get(&Key::from("e")).unwrap(),
            Some(vec![1_u8; 4096].into())
        );
        assert_eq!(db.last_sequence(), 8);

        db.put("wal-only", "value").unwrap();
        drop(db);

        let db = Db::open(path.clone()).unwrap();
        assert_eq!(
            db.get(&Key::from("wal-only")).unwrap(),
            Some("value".into())
        );
        assert_eq!(db.last_sequence(), 9);

        drop(db);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn rejects_sstables_without_manifest() {
        let path = path();
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("1.sst"), []).unwrap();

        assert!(matches!(
            Db::open(path.clone()),
            Err(Error::CorruptManifest(_))
        ));

        fs::remove_dir_all(path).unwrap();
    }
}
