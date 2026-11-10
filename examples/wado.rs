use clap::Parser;
use radicom_samples::{wado, Result};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(about = "Serve local DICOM instances at /wado (WADO-URI; no rendering)")]
struct Args {
    /// Directory containing .dcm files, read once at startup; omitted serves the built-in archive.
    #[arg(long)]
    directory: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: SocketAddr,
}

#[tokio::main]
async fn main() -> Result {
    let args = Args::parse();
    let app = match &args.directory {
        Some(directory) => wado::router(directory)?,
        None => wado::sample_router()?,
    };
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    println!("WADO listening on http://{}/wado", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
