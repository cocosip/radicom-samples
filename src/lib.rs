//! Shared code used by the runnable, non-rendering DICOM examples.

pub mod files;
pub mod network;
pub mod print;
pub mod wado;
pub mod worklist;

pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
}
