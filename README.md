# AsteroidDB

AsteroidDB is a Log-structure Merge (LSM) Tree based embedded Key-Value
storage engine.

Features:
- A write-ahead log (WAL) for recovery and atomic write batches.
- Memory tables, on-disk sorted tables (SSTables), and compaction.
- Bloom filters and a block cache for reads.
- Point lookups, overwrites, deletes, and ordered range scans.
- Database locking and process-crash recovery tests.

## Quick start

Requires a recent Rust toolchain on Unix. Run from the repository root:

```sh
cargo run --release -- --db-path=./data put name asteroid
cargo run --release -- --db-path=./data get name
cargo run --release -- --db-path=./data scan
cargo run --release -- --db-path=./data delete name
```

The database directory is created if missing. `get` writes the raw value without
a trailing newline and exits with an error if the key is missing. `scan` prints
sorted, tab-separated key/value pairs with special bytes escaped.

Build the standalone binary with `cargo build --release`; it is available at
`target/release/asteroiddb`. Use `--help` for command details.

## Rust API

```rust
use asteroiddb::{Db, Key};

fn main() -> asteroiddb::Result<()> {
    let mut db = Db::open("./data".into())?;
    db.put("name", "asteroid")?;
    assert_eq!(db.get(&Key::from("name"))?, Some("asteroid".into()));
    db.delete("name")?;
    Ok(())
}
```

## Tests

```sh
cargo test
cargo test --test crash # crash test
```