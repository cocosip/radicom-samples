use clap::Parser;
use radicom_samples::{network::run_server, worklist::WorklistProvider, Result};

#[derive(Parser)]
#[command(about = "Synthetic in-memory MWL/MPPS SCP; trusted loopback demonstration only")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:11115")]
    listen: String,
    #[arg(long, default_value = "WORKLIST")]
    ae: String,
}

#[tokio::main]
async fn main() -> Result {
    let args = Args::parse();
    radicom_samples::init_logging();
    println!(
        "Synthetic patient SAMPLE001, CT step SAMPLE_SPS_1, date 20261109; volatile shared state"
    );
    run_server(&args.listen, &args.ae, WorklistProvider::default()).await
}
