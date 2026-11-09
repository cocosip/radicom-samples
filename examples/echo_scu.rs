use clap::Parser;
use radicom_network::{association::sop_class, dimse::status};
use radicom_samples::{network::client, Result};

/// Send C-ECHO and require a successful DICOM status.
#[derive(Parser)]
struct Args {
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
    let args = Args::parse();
    let mut client = client(&args.address, &args.calling_ae, &args.called_ae);
    let response = client.send_c_echo(sop_class::VERIFICATION).await?;
    client.release().await;
    println!("C-ECHO status: {:04X}", response.status);
    if response.status != status::SUCCESS {
        return Err("C-ECHO failed".into());
    }
    Ok(())
}
