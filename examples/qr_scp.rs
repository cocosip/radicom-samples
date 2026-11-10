use clap::Parser;
use radicom_samples::{
    files::{load_files, sample_archive},
    network::{run_server, FileProvider},
    Result,
};
use std::{collections::HashMap, path::PathBuf};

/// Serve a startup snapshot of .dcm files, or the built-in archive, via Study Root FIND/GET/MOVE.
#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:11113")]
    address: String,
    #[arg(long, default_value = "QR_SCP")]
    ae: String,
    /// Directory snapshot; omitted serves the built-in two-instance archive.
    #[arg(long)]
    input: Option<PathBuf>,
    /// Trusted C-MOVE mapping AE=host:port (repeatable); default SAMPLES_SCP=127.0.0.1:11112.
    #[arg(long, value_name = "AE=ADDRESS")]
    destination: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    radicom_samples::init_logging();
    radicom_codecs::install();
    let args = Args::parse();
    let mut destinations = HashMap::new();
    if args.destination.is_empty() {
        destinations.insert("SAMPLES_SCP".to_owned(), "127.0.0.1:11112".to_owned());
    }
    for entry in args.destination {
        let (ae, address) = entry
            .split_once('=')
            .ok_or("destination must be AE=host:port")?;
        if ae.is_empty()
            || ae.len() > 16
            || !ae.bytes().all(|c| (b' '..=b'~').contains(&c) && c != b'\\')
            || address.is_empty()
        {
            return Err("invalid destination mapping".into());
        }
        if destinations.insert(ae.into(), address.into()).is_some() {
            return Err("duplicate destination AE".into());
        }
    }
    let files = match args.input {
        Some(directory) => load_files(&directory)?,
        None => sample_archive()?,
    };
    println!("Loaded {} instances", files.len());
    let provider = FileProvider::new(files, None, destinations, args.ae.clone())?;
    run_server(&args.address, &args.ae, provider).await
}
