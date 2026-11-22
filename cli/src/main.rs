use std::{
    error::Error,
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

use asteroiddb::{Db, Key};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Read and write an AsteroidDB database")]
struct Cli {
    /// Database directory (created if missing)
    #[arg(long, value_name = "PATH")]
    db_path: PathBuf,
    #[command(subcommand)]
    command: DbCommand,
}

#[derive(Subcommand)]
enum DbCommand {
    /// Store a key/value pair and sync the write
    Put { key: OsString, value: OsString },
    /// Write the raw value without a trailing newline; fail if the key is missing
    Get { key: OsString },
    /// Delete a key and sync the write
    Delete { key: OsString },
    /// Print sorted keys and values separated by tabs, with special bytes escaped
    Scan {
        /// Only print keys starting with these bytes
        #[arg(long)]
        prefix: Option<OsString>,
    },
    /// Create an independent checkpoint in a new directory
    Checkpoint { destination: PathBuf },
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    let mut db = Db::open(cli.db_path)?;
    let mut output = io::stdout().lock();
    match cli.command {
        DbCommand::Put { key, value } => {
            db.put(key.as_encoded_bytes(), value.as_encoded_bytes())?;
        }
        DbCommand::Get { key } => {
            let value = db
                .get(&Key::from(key.as_encoded_bytes()))?
                .ok_or("key not found")?;
            output.write_all(value.as_bytes())?;
        }
        DbCommand::Delete { key } => {
            db.delete(key.as_encoded_bytes())?;
        }
        DbCommand::Checkpoint { destination } => db.checkpoint(destination)?,
        DbCommand::Scan { prefix } => {
            let entries = match prefix {
                Some(prefix) => db.scan_prefix(prefix.as_encoded_bytes()),
                None => db.iter(),
            };
            for entry in entries {
                let (key, value) = entry?;
                writeln!(
                    output,
                    "{}\t{}",
                    key.as_bytes().escape_ascii(),
                    value.as_bytes().escape_ascii()
                )?;
            }
        }
    }
    output.flush()?;
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("asteroiddb: {error}");
            ExitCode::FAILURE
        }
    }
}
