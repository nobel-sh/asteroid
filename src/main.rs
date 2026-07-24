use ligma::{Key, Memtable};

fn main() {
    let mut memtable = Memtable::new();
    let key = Key::from("foo");

    memtable.put(key.clone(), "bar");
    println!("{:?}", memtable.get(&key));
}
