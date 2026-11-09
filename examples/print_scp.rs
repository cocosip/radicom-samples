use clap::Parser;
use radicom_samples::{network::run_server, print::PrintProvider, Result};

#[derive(Parser)]
#[command(
    about = "Record-only grayscale Print SCP: one A4 image, no rendering or physical printing"
)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:11114")]
    listen: String,
    #[arg(long, default_value = "PRINT")]
    ae: String,
}

#[tokio::main]
async fn main() -> Result {
    let args = Args::parse();
    radicom_samples::init_logging();
    println!("Print jobs/objects are volatile and shared; trusted loopback demonstration only");
    run_server(&args.listen, &args.ae, PrintProvider::default()).await
}
