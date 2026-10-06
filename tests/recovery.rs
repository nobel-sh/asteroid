mod common;

use std::fs;

use asteroiddb::{Db, Error, Key, Options, WriteBatch};
use common::test_dir;

#[test]
fn batch_writes_survive_reopening() {
    let path = test_dir("reopen");
    let mut db = Db::open(path.clone()).unwrap();
    db.put("deleted", "old").unwrap();

    let mut batch = WriteBatch::new();
    batch.put("kept", "old");
    batch.put("kept", "new");
    batch.delete("deleted");
    batch.put("other", "value");
    db.write(batch).unwrap();
    let sequence = db.last_sequence();
    db.write(WriteBatch::new()).unwrap();
    assert_eq!(db.last_sequence(), sequence);
    drop(db);

    let mut db = Db::open(path.clone()).unwrap();
    assert_eq!(db.last_sequence(), sequence);
    assert_eq!(db.get(&Key::from("kept")).unwrap(), Some("new".into()));
    assert_eq!(db.get(&Key::from("other")).unwrap(), Some("value".into()));
    assert_eq!(db.get(&Key::from("deleted")).unwrap(), None);
    db.put("after-reopen", "value").unwrap();
    assert_eq!(db.last_sequence(), sequence + 1);

    drop(db);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn recovers_multiple_sstables_and_wal() {
    let path = test_dir("migrated");
    let mut db = Db::open_with_options(
        path.clone(),
        Options {
            memtable_bytes: 4 * 1024,
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
    let table_count = fs::read_dir(&path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sst"))
        .count();
    assert_eq!(table_count, 1);
    let entries = db.iter().collect::<asteroiddb::Result<Vec<_>>>().unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>(),
        ["a", "c", "d", "e", "f"].map(Key::from)
    );
    db.put("wal-only", "value").unwrap();
    drop(db);

    let db = Db::open(path.clone()).unwrap();
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
    assert_eq!(
        db.get(&Key::from("wal-only")).unwrap(),
        Some("value".into())
    );
    assert_eq!(db.last_sequence(), 9);

    drop(db);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn discards_incomplete_wal_batch_and_accepts_new_writes() {
    let path = test_dir("truncated-wal");
    let mut db = Db::open(path.clone()).unwrap();
    db.put("first", "safe").unwrap();
    let mut batch = WriteBatch::new();
    batch.put("first", "partial");
    batch.put("second", "partial");
    db.write(batch).unwrap();
    drop(db);

    let wal = fs::OpenOptions::new()
        .write(true)
        .open(path.join("wal.db"))
        .unwrap();
    wal.set_len(wal.metadata().unwrap().len() - 1).unwrap();
    drop(wal);

    let mut db = Db::open(path.clone()).unwrap();
    assert_eq!(db.get(&Key::from("first")).unwrap(), Some("safe".into()));
    assert_eq!(db.get(&Key::from("second")).unwrap(), None);
    db.put("third", "new").unwrap();
    drop(db);

    let db = Db::open(path.clone()).unwrap();
    assert_eq!(db.get(&Key::from("first")).unwrap(), Some("safe".into()));
    assert_eq!(db.get(&Key::from("third")).unwrap(), Some("new".into()));
    drop(db);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn rejects_sstables_without_manifest() {
    let path = test_dir("migrated");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("1.sst"), []).unwrap();

    assert!(matches!(
        Db::open(path.clone()),
        Err(Error::CorruptManifest(_))
    ));

    fs::remove_dir_all(path).unwrap();
}

#[test]
fn locks_database_until_closed() {
    let path = test_dir("lock");
    let db = Db::open(path.clone()).unwrap();
    assert!(matches!(Db::open(path.clone()), Err(Error::DbLocked)));
    drop(db);
    drop(Db::open(path.clone()).unwrap());
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn rejects_invalid_options() {
    let options = Options {
        memtable_bytes: 0,
        ..Options::default()
    };
    assert!(matches!(
        Db::open_with_options(test_dir("options"), options),
        Err(Error::InvalidOptions(_))
    ));
}

#[test]
fn memory_limit_counts_live_entries_and_recovered_writes() {
    let path = test_dir("memtable-limit");
    let options = Options {
        memtable_bytes: 256,
        ..Options::default()
    };
    let mut db = Db::open_with_options(path.clone(), options.clone()).unwrap();
    for n in 0..8 {
        db.put("a", vec![n; 128]).unwrap();
    }
    assert!(!path.join("00000001.sst").exists());
    drop(db);

    let mut db = Db::open_with_options(path.clone(), options.clone()).unwrap();
    db.put("b", vec![1; 128]).unwrap();
    assert!(path.join("00000001.sst").exists());
    assert_eq!(fs::metadata(path.join("wal.db")).unwrap().len(), 0);
    db.put("c", vec![2; 128]).unwrap();
    assert!(!path.join("00000002.sst").exists());
    assert!(fs::metadata(path.join("wal.db")).unwrap().len() > 0);
    drop(db);

    let db = Db::open_with_options(path.clone(), options).unwrap();
    for (key, byte) in [("a", 7), ("b", 1), ("c", 2)] {
        assert_eq!(
            db.get(&Key::from(key)).unwrap(),
            Some(vec![byte; 128].into())
        );
    }
    drop(db);
    fs::remove_dir_all(path).unwrap();
}
