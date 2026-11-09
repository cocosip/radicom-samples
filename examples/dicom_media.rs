use clap::{Parser, Subcommand};
use radicom::media::DicomDirectory;
use radicom::Tag;
use radicom_samples::{
    files::{load_files, media_reference, write_media},
    Result,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Create or read a DICOMDIR file-set without icons/rendering")]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Copy .dcm inputs into a NEW media directory, then write DICOMDIR.
    Write { input: PathBuf, output: PathBuf },
    /// Print the record hierarchy and verify each referenced file exists.
    Read { directory: PathBuf },
}

fn main() -> Result {
    match Args::parse().command {
        Command::Write { input, output } => {
            write_media(&load_files(&input)?, &output)?;
            println!("Created {}", output.join("DICOMDIR").display());
        }
        Command::Read { directory } => {
            let media = DicomDirectory::open(directory.join("DICOMDIR"))?;
            for (index, depth) in media.walk() {
                let record = &media.records()[index];
                println!(
                    "{}{} {}",
                    "  ".repeat(depth),
                    record.record_type().as_str(),
                    record
                        .dataset
                        .try_get_str(Tag::PATIENT_NAME)
                        .or_else(|| record.sop_instance_uid())
                        .unwrap_or_default()
                );
                if let Some(relative) = record.referenced_file_id() {
                    let path = media_reference(&directory, &relative)?;
                    println!("{}{}", "  ".repeat(depth + 1), path.display());
                }
            }
        }
    }
    Ok(())
}
