use ligma::{Db, Key};

fn main() {
    let mut db = Db::open("db.wal").expect("failed to open database");
    let key = Key::from("foo");

    db.put(key.clone(), "bar").expect("put failed");
    println!("{:?}", db.get(&key));
}
