mod common;

use std::fs;

use asteroiddb::{Db, Key};
use common::test_dir;

#[test]
fn puts_overwrites_and_deletes() {
    let path = test_dir("writes");
    let mut db = Db::open(path.clone()).unwrap();

    assert_eq!(db.get(&Key::from("missing")).unwrap(), None);
    db.put("key", "old").unwrap();
    assert_eq!(db.get(&Key::from("key")).unwrap(), Some("old".into()));
    db.put("key", "new").unwrap();
    assert_eq!(db.get(&Key::from("key")).unwrap(), Some("new".into()));
    db.delete("key").unwrap();
    db.delete("missing").unwrap();
    assert_eq!(db.get(&Key::from("key")).unwrap(), None);
    assert!(db.iter().next().is_none());

    drop(db);
    fs::remove_dir_all(path).unwrap();
}
