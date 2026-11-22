#[path = "engine.rs"]
mod engine;

use std::{
    fs::{self, File},
    io,
    ops::RangeBounds,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    cache::BlockCache, manifest::Manifest, memtable::Memtable, table::SSTable, wal::Wal,
    DbIterator, Error, Key, Options, Result, Value, WriteBatch, WriteOptions,
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
    requires_recovery: bool,
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

    /// Scans keys beginning with the given bytes, in ascending key order.
    pub fn scan_prefix(&self, prefix: impl AsRef<[u8]>) -> DbIterator<'_> {
        let prefix = prefix.as_ref();
        let start = Key::from(prefix);
        // Carry past trailing 0xff bytes to find the exclusive upper bound.
        if let Some(i) = prefix.iter().rposition(|&byte| byte != u8::MAX) {
            let mut end = prefix[..=i].to_vec();
            end[i] += 1;
            self.range(start..Key::from(end))
        } else {
            self.range(start..)
        }
    }

    /// Copies the current database, including pending WAL writes, to a new directory.
    /// Its parent must already exist outside the source database. Writes pause during the copy.
    pub fn checkpoint(&mut self, destination: impl AsRef<Path>) -> Result<()> {
        let destination = destination.as_ref();
        let name = destination.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "checkpoint needs a new directory name",
            )
        })?;
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()?;
        if parent.starts_with(self.db_dir.canonicalize()?) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "checkpoint must be outside the database directory",
            )
            .into());
        }
        let destination = parent.join(name);
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "checkpoint destination exists",
                )
                .into())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        self.sync_wal()?;
        let staging = tempfile::Builder::new()
            .prefix(".asteroiddb-checkpoint-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&parent)?;
        let copy = |source: &Path, target: PathBuf| -> io::Result<()> {
            fs::copy(source, &target)?;
            File::open(target)?.sync_all()
        };
        for table in &self.sstables {
            copy(&table.path(&self.db_dir), table.path(staging.path()))?;
        }
        // The on-disk manifest records the flushed sequence. Newer writes live in the WAL.
        for name in ["wal.db", "MANIFEST"] {
            copy(&self.db_dir.join(name), staging.path().join(name))?;
        }
        File::open(staging.path())?.sync_all()?;
        fs::rename(staging.path(), destination)?;
        let _ = staging.keep();
        File::open(parent)?.sync_all()?;
        Ok(())
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
        if self.requires_recovery {
            return Err(Error::DbRequiresRecovery);
        }
        self.wal.sync()
    }

    pub fn last_sequence(&self) -> u64 {
        self.manifest.last_sequence
    }
}
