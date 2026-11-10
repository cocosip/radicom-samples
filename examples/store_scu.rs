use clap::Parser;
use radicom::io::file::DicomFile;
use radicom_network::dimse::status;
use radicom_samples::{files::sample_file, network::client, Result};
use std::path::PathBuf;

/// Send DICOM files, or the built-in sample instance when no path is given.
#[derive(Parser)]
struct Args {
    /// Files to send; omitted sends the built-in 16x16 sample.
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
    let mut files = Vec::new();
    for path in &args.files {
        files.push((path.display().to_string(), DicomFile::open(path)?));
    }
    if files.is_empty() {
        files.push(("<built-in sample>".to_owned(), sample_file()?));
    }
    let mut client = client(&args.address, &args.calling_ae, &args.called_ae);
    for (label, file) in files {
        let response = client.send_c_store(&file).await?;
        println!(
            "{}: {:04X} {:?}",
            label, response.status, response.error_comment
        );
        if response.status != status::SUCCESS {
            client.release().await;
            return Err("C-STORE failed".into());
        }
    }
    client.release().await;
    Ok(())
}
