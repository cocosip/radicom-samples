use clap::Parser;
use radicom::Tag;
use radicom_samples::{
    files::{sample_file, save_new, validate_uid},
    Result,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Write a synthetic 16x16 DICOM image (no patient data, no overwrite)")]
struct Args {
    output: PathBuf,
    #[arg(long, default_value = "2.25.102")]
    instance_uid: String,
}

fn main() -> Result {
    let args = Args::parse();
    validate_uid(&args.instance_uid)?;
    let mut file = sample_file()?;
    file.dataset
        .try_insert_value(Tag::SOP_INSTANCE_UID, args.instance_uid)?;
    save_new(&file, &args.output)?;
    println!("Created {}", args.output.display());
    Ok(())
}
