mod common;

use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use asteroiddb::{Db, Key, Options, Value, WriteBatch, WriteOptions};
use common::test_dir;

fn assert_contents(db: &Db, expected: &[(&str, Vec<u8>)]) {
    let expected: Vec<(Key, Value)> = expected
        .iter()
        .map(|(key, value)| (Key::from(*key), Value::from(value.clone())))
        .collect();
    assert_eq!(
        db.iter().collect::<asteroiddb::Result<Vec<_>>>().unwrap(),
        expected
    );
    for (key, value) in expected {
        assert_eq!(db.get(&key).unwrap(), Some(value));
    }
    assert_eq!(db.get(&Key::from("deleted")).unwrap(), None);
}

#[test]
fn acknowledged_writes_survive_process_crash_matrix() {
    for storage in ["wal", "flush", "compact"] {
        for durability in ["sync", "async", "manual-sync"] {
            let path = test_dir(&format!("crash-{storage}-{durability}"));
            eprintln!("crash case: {storage}/{durability} at {}", path.display());
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "crash_worker", "--ignored", "--nocapture"])
                .env("ASTEROIDDB_CRASH_DIR", &path)
                .env("ASTEROIDDB_CRASH_STORAGE", storage)
                .env("ASTEROIDDB_CRASH_DURABILITY", durability)
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            while !path.join("READY").exists() {
                if let Some(status) = child.try_wait().unwrap() {
                    panic!("{storage}/{durability}: worker exited before acknowledgment: {status}");
                }
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("{storage}/{durability}: timed out waiting for acknowledgment");
                }
                thread::sleep(Duration::from_millis(5));
            }
            child.kill().unwrap();
            assert!(!child.wait().unwrap().success());

            let tables = fs::read_dir(&path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "sst"))
                .count();
            assert_eq!(
                tables,
                match storage {
                    "wal" => 0,
                    "flush" => 2,
                    _ => 1,
                }
            );
            assert!(fs::metadata(path.join("wal.db")).unwrap().len() > 0);
            let mut expected = vec![
                ("other", b"value".to_vec()),
                ("pad-a", vec![1; 4096]),
                ("pad-b", vec![2; 4096]),
                ("stable", b"safe".to_vec()),
                ("updated", b"new".to_vec()),
                ("wal-only", b"tail".to_vec()),
            ];
            let mut db = Db::open(path.clone()).unwrap();
            assert_eq!(db.last_sequence(), 9);
            assert_contents(&db, &expected);
            db.put("z-after", "recovered").unwrap();
            drop(db);
            expected.push(("z-after", b"recovered".to_vec()));
            let db = Db::open(path.clone()).unwrap();
            assert_eq!(db.last_sequence(), 10);
            assert_contents(&db, &expected);
            drop(db);
            fs::remove_dir_all(path).unwrap();
        }
    }
}

#[test]
#[ignore = "subprocess entry point for the crash matrix"]
fn crash_worker() {
    let path =
        PathBuf::from(std::env::var_os("ASTEROIDDB_CRASH_DIR").expect("run the crash matrix"));
    let storage = std::env::var("ASTEROIDDB_CRASH_STORAGE").unwrap();
    let durability = std::env::var("ASTEROIDDB_CRASH_DURABILITY").unwrap();
    let mut db = Db::open_with_options(
        path.clone(),
        Options {
            memtable_bytes: if storage == "wal" { usize::MAX } else { 1024 },
            compaction_trigger: if storage == "compact" { 2 } else { 4 },
            ..Options::default()
        },
    )
    .unwrap();
    let options = WriteOptions {
        sync: durability == "sync",
    };
    let mut first = WriteBatch::new();
    first.put("stable", "safe");
    first.put("updated", "old");
    first.put("deleted", "old");
    first.put("pad-a", vec![1; 4096]);
    db.write_with_options(first, options).unwrap();
    let mut second = WriteBatch::new();
    second.put("updated", "new");
    second.delete("deleted");
    second.put("other", "value");
    second.put("pad-b", vec![2; 4096]);
    db.write_with_options(second, options).unwrap();
    db.put_with_options("wal-only", "tail", options).unwrap();
    if durability == "manual-sync" {
        db.sync_wal().unwrap();
    }
    fs::write(path.join("READY"), []).unwrap();
    loop {
        thread::park();
    }
}

#[test]
fn every_final_wal_batch_truncation_is_atomic() {
    let path = test_dir("crash-torn-wal");
    let wal = path.join("wal.db");
    let mut db = Db::open(path.clone()).unwrap();
    let mut baseline = WriteBatch::new();
    baseline.put("updated", "old");
    baseline.put("deleted", "old");
    db.write(baseline).unwrap();
    let prefix_len = fs::metadata(&wal).unwrap().len() as usize;
    let mut batch = WriteBatch::new();
    batch.put("updated", "intermediate");
    batch.delete("deleted");
    batch.put("new", "value");
    batch.put("updated", "final");
    db.write(batch).unwrap();
    drop(db);
    let bytes = fs::read(&wal).unwrap();

    // shortcut: byte truncation models a torn tail only; use filesystem fault injection for reordered writes.
    for end in prefix_len..=bytes.len() {
        eprintln!("WAL cut: {end}/{} at {}", bytes.len(), path.display());
        fs::write(&wal, &bytes[..end]).unwrap();
        let complete = end == bytes.len();
        let mut db = Db::open(path.clone()).unwrap();
        let sequence = if complete { 6 } else { 2 };
        assert_eq!(db.last_sequence(), sequence);
        if complete {
            assert_contents(
                &db,
                &[("new", b"value".to_vec()), ("updated", b"final".to_vec())],
            );
        } else {
            assert_eq!(db.get(&Key::from("deleted")).unwrap(), Some("old".into()));
            assert_eq!(db.get(&Key::from("updated")).unwrap(), Some("old".into()));
            assert_eq!(db.get(&Key::from("new")).unwrap(), None);
            assert_eq!(db.iter().count(), 2);
            assert_eq!(fs::metadata(&wal).unwrap().len(), prefix_len as u64);
        }
        db.put("z-after", "recovered").unwrap();
        drop(db);
        let db = Db::open(path.clone()).unwrap();
        assert_eq!(db.last_sequence(), sequence + 1);
        assert_eq!(
            db.get(&Key::from("z-after")).unwrap(),
            Some("recovered".into())
        );
        assert_eq!(
            db.get(&Key::from("updated")).unwrap(),
            Some(if complete { "final" } else { "old" }.into())
        );
        drop(db);
    }
    fs::remove_dir_all(path).unwrap();
}
