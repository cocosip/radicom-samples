use clap::Parser;
use radicom_samples::{
    network::{run_server, FileProvider},
    Result,
};
use std::{collections::HashMap, path::PathBuf};

/// Receive DICOM files without overwriting an existing SOP Instance UID.
#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:11112")]
    address: String,
    #[arg(long, default_value = "SAMPLES_SCP")]
    ae: String,
    #[arg(long, default_value = "received")]
    output: PathBuf,
    /// Additional storage SOP Class UID (repeatable); defaults include SC, CT, MR.
    #[arg(long)]
    sop_class: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    radicom_samples::init_logging();
    let args = Args::parse();
    let mut provider =
        FileProvider::new(vec![], Some(args.output), HashMap::new(), args.ae.clone())?;
    for uid in args.sop_class {
        provider.add_storage_class(uid)?;
    }
    run_server(&args.address, &args.ae, provider).await
}
