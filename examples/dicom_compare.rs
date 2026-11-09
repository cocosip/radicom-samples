use clap::Parser;
use radicom::io::file::DicomFile;
use radicom_samples::{files::differences, Result};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Compare DICOM datasets deeply; exit 1 for differences, 2 for errors")]
struct Args {
    left: PathBuf,
    right: PathBuf,
}

fn compare(args: Args) -> Result<bool> {
    let left = DicomFile::open(args.left)?;
    let right = DicomFile::open(args.right)?;
    let differences = differences(&left.dataset, &right.dataset);
    for difference in &differences {
        println!("{difference}");
    }
    println!(
        "{} difference(s). File meta/transfer syntax excluded; pixels compared as stored.",
        differences.len()
    );
    Ok(differences.is_empty())
}

fn main() -> std::process::ExitCode {
    match compare(Args::parse()) {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::from(1),
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::from(2)
        }
    }
}
