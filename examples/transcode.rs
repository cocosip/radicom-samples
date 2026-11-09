use clap::Parser;
use radicom::io::file::DicomFile;
use radicom::model::transfer_syntax;
use radicom_samples::{
    files::{save_new, validate_uid},
    Result,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Transcode DICOM to an explicit transfer-syntax UID; source is preserved")]
struct Args {
    input: PathBuf,
    output: PathBuf,
    /// Target transfer-syntax UID, e.g. 1.2.840.10008.1.2.5 (RLE Lossless).
    #[arg(long)]
    syntax: String,
    /// Acknowledge that a lossy target changes pixel values.
    #[arg(long)]
    allow_lossy: bool,
}

fn main() -> Result {
    let args = Args::parse();
    validate_uid(&args.syntax)?;
    radicom_codecs::install();
    let syntax = transfer_syntax::query(&args.syntax).ok_or("Unknown target transfer syntax")?;
    if syntax.is_lossy() && !args.allow_lossy {
        return Err("Lossy target requires --allow-lossy; pixel values will change".into());
    }
    let file = DicomFile::open(args.input)?;
    let transcoded = file.clone_with_syntax(syntax, None)?;
    save_new(&transcoded, &args.output)?;
    println!(
        "Wrote {} ({})",
        args.output.display(),
        transcoded.dataset.transfer_syntax()
    );
    Ok(())
}
