use clap::Parser;
use radicom::io::file::DicomFile;
use radicom_network::dimse::status;
use radicom_samples::{network::client, Result};
use std::path::PathBuf;

/// Send one or more DICOM files, checking every C-STORE status.
#[derive(Parser)]
struct Args {
    #[arg(required = true)]
    files: Vec<PathBuf>,
    #[arg(long, default_value = "127.0.0.1:11112")]
    address: String,
    #[arg(long, default_value = "SAMPLES_SCU")]
    calling_ae: String,
    #[arg(long, default_value = "SAMPLES_SCP")]
    called_ae: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    radicom_samples::init_logging();
    radicom_codecs::install();
    let args = Args::parse();
    let mut client = client(&args.address, &args.calling_ae, &args.called_ae);
    for path in args.files {
        let response = client.send_c_store(&DicomFile::open(&path)?).await?;
        println!(
            "{}: {:04X} {:?}",
            path.display(),
            response.status,
            response.error_comment
        );
        if response.status != status::SUCCESS {
            client.release().await;
            return Err("C-STORE failed".into());
        }
    }
    client.release().await;
    Ok(())
}
