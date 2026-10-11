mod common;

use std::{
    fs,
    hint::black_box,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use asteroiddb::{Db, Options, Value, WriteOptions};
use clap::Parser;
use common::{make_keys, measure, shuffled_order, Args};

fn run_benchmarks(args: &Args, path: &Path) -> asteroiddb::Result<()> {
    let count = args.keys as usize;
    let options = Options {
        block_cache_bytes: args.cache.then_some(4 * 1024 * 1024),
        ..Options::default()
    };
    let keys = make_keys(args.keys, args.key_bytes);
    let value = Value::from(vec![0x5a; args.value_bytes as usize]);
    let sequential: Vec<_> = (0..count).collect();
    let shuffled = shuffled_order(count);
    for (name, order) in [("Sequential", &sequential), ("Random", &shuffled)] {
        let mut db = Db::open_with_options(path.join(name), options.clone())?;
        measure(&format!("{name} writes"), count, || {
            for &i in order {
                db.put_with_options(
                    keys[i].clone(),
                    value.clone(),
                    WriteOptions { sync: args.sync },
                )?;
            }
            Ok(())
        })?;
        db.sync_wal()?;
        for &i in order {
            assert_eq!(db.get(&keys[i])?.as_ref(), Some(&value));
        }
        measure(&format!("{name} reads"), count, || {
            if name == "Sequential" {
                for entry in db.iter() {
                    black_box(entry?);
                }
            } else {
                for &i in order {
                    black_box(db.get(black_box(&keys[i]))?);
                }
            }
            Ok(())
        })?;
        let mut visited = 0;
        for entry in db.iter() {
            let (key, found) = entry?;
            assert_eq!(key, keys[visited]);
            assert_eq!(found, value);
            visited += 1;
        }
        assert_eq!(visited, count);
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let path = Path::new("target/bench-data").join(format!("basic-{}-{stamp}", std::process::id()));
    fs::create_dir_all(path.parent().unwrap())?;
    fs::create_dir(&path)?;
    println!("Data: {} (removed on success)", path.display());
    println!(
        "{} keys; {}-byte keys; {}-byte values; sync={}; cache={}",
        args.keys, args.key_bytes, args.value_bytes, args.sync, args.cache
    );
    run_benchmarks(&args, &path)?;
    fs::remove_dir_all(path)?;
    Ok(())
}
