use std::time::Instant;

use asteroiddb::Key;
use clap::Parser;

#[derive(Parser)]
pub struct Args {
    #[arg(long, default_value = "100000", value_parser = clap::value_parser!(u32).range(1..))]
    pub keys: u32,
    #[arg(long, default_value = "16", value_parser = clap::value_parser!(u32).range(8..))]
    pub key_bytes: u32,
    #[arg(long, default_value = "100", value_parser = clap::value_parser!(u32).range(1..))]
    pub value_bytes: u32,
    #[arg(long, default_value = "false", action = clap::ArgAction::Set)]
    pub sync: bool,
    /// Enable a 4 MiB block cache.
    #[arg(long, default_value = "true", action = clap::ArgAction::Set)]
    pub cache: bool,
    #[arg(long, hide = true)]
    bench: bool,
}

pub fn measure(
    name: &str,
    count: usize,
    operation: impl FnOnce() -> asteroiddb::Result<()>,
) -> asteroiddb::Result<()> {
    let start = Instant::now();
    operation()?;
    let seconds = start.elapsed().as_secs_f64();
    println!(
        "{name:<20} {:>12.0} ops/s  {:>10.2} us/op  {:>8.3} s",
        count as f64 / seconds,
        seconds * 1_000_000.0 / count as f64,
        seconds,
    );
    Ok(())
}

pub fn make_keys(count: u32, key_bytes: u32) -> Vec<Key> {
    (0..count)
        .map(|i| {
            let mut bytes = vec![0; key_bytes as usize];
            bytes[..8].copy_from_slice(&u64::from(i).to_be_bytes());
            Key::from(bytes)
        })
        .collect()
}

pub fn shuffled_order(count: usize) -> Vec<usize> {
    let mut shuffled: Vec<_> = (0..count).collect();
    let mut random = 73_u64;
    for i in (1..count).rev() {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        shuffled.swap(i, (random % (i as u64 + 1)) as usize);
    }
    shuffled
}
