use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn test_dir(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("asteroiddb-{name}-{}-{stamp}", std::process::id()))
}
