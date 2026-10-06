mod common;

use std::fs;

use asteroiddb::{Db, Key, Options, Value, WriteBatch};
use common::test_dir;

#[test]
fn scans_match_a_map_after_flushes_compaction_and_reopening() {
    let path = test_dir("scans");
    let options = Options {
        memtable_bytes: 1,
        compaction_trigger: 2,
        ..Options::default()
    };
    let mut db = Db::open_with_options(path.clone(), options.clone()).unwrap();
    let mut expected = std::collections::BTreeMap::<Key, Value>::new();

    for round in 0..3 {
        let mut batch = WriteBatch::new();
        for number in (0..8).rev() {
            let key = Key::from(format!("key-{number}").as_str());
            if (round + number) % 3 == 0 {
                batch.delete(key.clone());
                expected.remove(&key);
            } else {
                let value = Value::from(format!("value-{round}-{number}").as_str());
                batch.put(key.clone(), value.clone());
                expected.insert(key, value);
            }
        }
        db.write(batch).unwrap();
        let key = Key::from("single");
        if round % 2 == 0 {
            db.put(key.clone(), "value").unwrap();
            expected.insert(key, Value::from("value"));
        } else {
            db.delete(key.clone()).unwrap();
            expected.remove(&key);
        }
        assert_eq!(
            db.iter().collect::<asteroiddb::Result<Vec<_>>>().unwrap(),
            expected.clone().into_iter().collect::<Vec<_>>()
        );
        drop(db);
        db = Db::open_with_options(path.clone(), options.clone()).unwrap();
    }
    drop(db);

    let db = Db::open_with_options(path.clone(), options).unwrap();
    assert_eq!(
        db.iter().collect::<asteroiddb::Result<Vec<_>>>().unwrap(),
        expected.clone().into_iter().collect::<Vec<_>>()
    );
    for number in 0..8 {
        let key = Key::from(format!("key-{number}").as_str());
        assert_eq!(db.get(&key).unwrap().as_ref(), expected.get(&key));
    }
    let bounds = Key::from("key-2")..Key::from("key-6");
    assert_eq!(
        db.range(bounds.clone())
            .collect::<asteroiddb::Result<Vec<_>>>()
            .unwrap(),
        expected
            .range(bounds)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>()
    );

    drop(db);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn searches_sstable_key_bounds() {
    use std::ops::Bound::{Excluded, Included, Unbounded};

    let path = test_dir("migrated");
    let options = Options {
        memtable_bytes: 1,
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
fn reads_across_tables_and_memtable() {
    let path = test_dir("layers");
    let options = Options {
        memtable_bytes: 256,
        compaction_trigger: 10,
        ..Options::default()
    };
    let mut db = Db::open_with_options(path.clone(), options.clone()).unwrap();
    let mut expected = std::collections::BTreeMap::<Key, Value>::new();
    for round in 0..4 {
        let mut batch = WriteBatch::new();
        for n in 0..6 {
            let key = Key::from(format!("k{n}").as_str());
            if (round + n) % 3 == 0 {
                batch.delete(key.clone());
                expected.remove(&key);
            } else {
                let value = Value::from(vec![round as u8; 32]);
                batch.put(key.clone(), value.clone());
                expected.insert(key, value);
            }
        }
        db.write(batch).unwrap();
    }
    db.delete("k0").unwrap();
    expected.remove(&Key::from("k0"));
    db.put("k1", "memtable").unwrap();
    expected.insert(Key::from("k1"), Value::from("memtable"));
    for pass in 0..2 {
        for n in 0..6 {
            let key = Key::from(format!("k{n}").as_str());
            assert_eq!(db.get(&key).unwrap().as_ref(), expected.get(&key));
        }
        assert_eq!(
            db.iter().collect::<asteroiddb::Result<Vec<_>>>().unwrap(),
            expected.clone().into_iter().collect::<Vec<_>>()
        );
        if pass == 0 {
            drop(db);
            db = Db::open_with_options(path.clone(), options.clone()).unwrap();
        }
    }
    drop(db);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn empty_and_reversed_ranges_are_empty() {
    use std::ops::Bound::{Excluded, Included};
    let path = test_dir("empty-ranges");
    let mut db = Db::open(path.clone()).unwrap();
    db.put("a", "value").unwrap();
    let a = Key::from("a");
    let z = Key::from("z");
    assert!(db.range(z..a.clone()).next().is_none());
    assert!(db
        .range((Excluded(a.clone()), Included(a.clone())))
        .next()
        .is_none());
    assert!(db
        .range((Included(a.clone()), Excluded(a)))
        .next()
        .is_none());
    drop(db);
    fs::remove_dir_all(path).unwrap();
}
