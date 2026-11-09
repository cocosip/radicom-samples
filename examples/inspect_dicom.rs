use clap::Parser;
use radicom::io::file::DicomFile;
use radicom::pixel::PixelData;
use radicom::Tag;
use radicom_samples::Result;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Inspect DICOM tags and pixel metadata without rendering")]
struct Args {
    input: PathBuf,
}

fn main() -> Result {
    let args = Args::parse();
    let mut file = DicomFile::open(&args.input)?;
    println!("Transfer syntax: {}", file.dataset.transfer_syntax());
    println!("{}", file.dataset.to_dump_string());
    if file.dataset.contains(&Tag::PIXEL_DATA) {
        let pixels = PixelData::from_dataset(&mut file.dataset)?;
        println!(
            "Pixels: {}x{}, {} frame(s), {} bit(s), {} sample(s)",
            pixels.width(),
            pixels.height(),
            pixels.number_of_frames(),
            pixels.bits_allocated()?,
            pixels.samples_per_pixel()
        );
    }
    Ok(())
}
