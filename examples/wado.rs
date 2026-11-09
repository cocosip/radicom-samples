use clap::Parser;
use radicom_samples::{wado, Result};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(about = "Serve local DICOM instances at /wado (WADO-URI; no rendering)")]
struct Args {
    /// Directory containing .dcm files; read once at startup.
    #[arg(long)]
    directory: PathBuf,
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: SocketAddr,
}

#[tokio::main]
async fn main() -> Result {
    let args = Args::parse();
    let app = wado::router(&args.directory)?;
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    println!("WADO listening on http://{}/wado", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
