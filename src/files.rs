//! Synthetic input, safe output, DICOMDIR creation and dataset comparison.

use crate::Result;
use radicom::io::file::DicomFile;
use radicom::media::DicomDirectory;
use radicom::model::item::DicomItem;
use radicom::model::uid::Uid;
use radicom::pixel::PixelData;
use radicom::{uids, Dataset, Tag, Vr};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Fixed demonstration UIDs make the local SCU/SCP walkthrough reproducible.
pub fn sample_file() -> Result<DicomFile> {
    let mut dataset = Dataset::new();
    for (tag, vr, value) in [
        (
            Tag::SOP_CLASS_UID,
            Vr::Ui,
            uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
        ),
        (Tag::SOP_INSTANCE_UID, Vr::Ui, "2.25.102"),
        (Tag::STUDY_INSTANCE_UID, Vr::Ui, "2.25.100"),
        (Tag::SERIES_INSTANCE_UID, Vr::Ui, "2.25.101"),
        (Tag::PATIENT_NAME, Vr::Pn, "SAMPLE^SYNTHETIC"),
        (Tag::PATIENT_ID, Vr::Lo, "SAMPLE001"),
        (Tag::PATIENT_BIRTH_DATE, Vr::Da, ""),
        (Tag::PATIENT_SEX, Vr::Cs, "O"),
        (Tag::STUDY_DATE, Vr::Da, "20260101"),
        (Tag::STUDY_TIME, Vr::Tm, "120000"),
        (Tag::STUDY_ID, Vr::Sh, "SAMPLE"),
        (Tag::ACCESSION_NUMBER, Vr::Sh, "SAMPLE001"),
        (Tag::REFERRING_PHYSICIAN_NAME, Vr::Pn, ""),
        (Tag::MODALITY, Vr::Cs, "OT"),
        (Tag::CONVERSION_TYPE, Vr::Cs, "SYN"),
        (Tag::SERIES_NUMBER, Vr::Is, "1"),
        (Tag::INSTANCE_NUMBER, Vr::Is, "1"),
        (Tag::IMAGE_TYPE, Vr::Cs, "DERIVED\\SECONDARY"),
        (
            Tag::IMAGE_COMMENTS,
            Vr::Lt,
            "Synthetic demonstration pixels; not for diagnosis",
        ),
        (Tag::PHOTOMETRIC_INTERPRETATION, Vr::Cs, "MONOCHROME2"),
    ] {
        dataset.try_insert_str(tag, vr, value)?;
    }
    for (tag, value) in [
        (Tag::ROWS, 16u16),
        (Tag::COLUMNS, 16),
        (Tag::SAMPLES_PER_PIXEL, 1),
        (Tag::BITS_ALLOCATED, 8),
        (Tag::BITS_STORED, 8),
        (Tag::HIGH_BIT, 7),
        (Tag::PIXEL_REPRESENTATION, 0),
    ] {
        dataset.try_insert_value(tag, value)?;
    }
    PixelData::create_new(&mut dataset)?.add_frame((0..=255).collect::<Vec<u8>>())?;
    Ok(DicomFile::from_dataset(dataset))
}

/// Built-in QR archive: the sample image plus a second instance in the same study.
pub fn sample_archive() -> Result<Vec<DicomFile>> {
    let mut files = Vec::new();
    for instance_uid in ["2.25.102", "2.25.103"] {
        let mut file = sample_file()?;
        file.dataset
            .try_insert_str(Tag::SOP_INSTANCE_UID, Vr::Ui, instance_uid)?;
        files.push(file);
    }
    Ok(files)
}

pub fn validate_uid(uid: &str) -> Result {
    if !Uid::is_valid(uid) || !uid.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(format!("Invalid DICOM UID: {uid:?}").into());
    }
    Ok(())
}

fn write_new(path: &Path, write: impl FnOnce(&mut fs::File) -> Result) -> Result {
    let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = write(&mut output).and_then(|()| Ok(output.sync_all()?));
    drop(output);
    if result.is_err() {
        // Only this call's newly created, incomplete file may be removed.
        fs::remove_file(path)?;
    }
    result
}

/// Fails if the destination already exists, including when it names the input.
pub fn save_new(file: &DicomFile, path: &Path) -> Result {
    write_new(path, |output| Ok(file.write(output, &Default::default())?))
}

pub fn save_instance(file: &DicomFile, directory: &Path) -> Result<PathBuf> {
    let uid = file.dataset.get_str(Tag::SOP_INSTANCE_UID)?;
    validate_uid(&uid)?;
    fs::create_dir_all(directory)?;
    let path = directory.join(format!("{uid}.dcm"));
    save_new(file, &path)?;
    Ok(path)
}

/// Scan only regular .dcm files; ignore symlinks and fail on corrupt input.
pub fn load_files(directory: &Path) -> Result<Vec<DicomFile>> {
    fn visit(directory: &Path, paths: &mut Vec<PathBuf>) -> Result {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                visit(&entry.path(), paths)?;
            } else if kind.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|s| s.eq_ignore_ascii_case("dcm"))
            {
                paths.push(entry.path());
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    visit(directory, &mut paths)?;
    paths.sort();
    paths
        .iter()
        .map(|path| Ok(DicomFile::open(path)?))
        .collect()
}

/// Build a new file-set, copying each instance to a valid DICOM File ID.
pub fn write_media(files: &[DicomFile], output: &Path) -> Result {
    if files.is_empty() || files.len() > 99_999_999 {
        return Err("Media requires 1..=99,999,999 instances".into());
    }
    let mut directory = DicomDirectory::try_new("RADICOM")?;
    let mut seen = BTreeSet::new();
    for (index, file) in files.iter().enumerate() {
        let uid = file.dataset.get_str(Tag::SOP_INSTANCE_UID)?;
        validate_uid(&uid)?;
        if !seen.insert(uid) {
            return Err("Duplicate SOP Instance UID in media input".into());
        }
        directory.add_file(file, Path::new("IMAGES").join(format!("{:08}", index + 1)))?;
    }
    let bytes = directory.to_bytes()?;
    fs::create_dir(output)?;
    fs::create_dir(output.join("IMAGES"))?;
    // A failed copy leaves a partial file-set for inspection; rerun into a new directory.
    for (index, file) in files.iter().enumerate() {
        save_new(
            file,
            &output.join("IMAGES").join(format!("{:08}", index + 1)),
        )?;
    }
    write_new(&output.join("DICOMDIR"), |file| Ok(file.write_all(&bytes)?))
}

/// Resolve a DICOMDIR File ID within its selected file-set.
pub fn media_reference(directory: &Path, relative: &Path) -> Result<PathBuf> {
    if relative
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("Unsafe referenced File ID".into());
    }
    let root = directory.canonicalize()?;
    let path = root.join(relative).canonicalize()?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("Referenced file is not a regular file inside the file-set".into());
    }
    Ok(path)
}

/// Compare decoded dataset contents, including every nested item and binary byte.
/// File meta information and transfer syntax are reported separately by the CLI.
pub fn differences(left: &Dataset, right: &Dataset) -> Vec<String> {
    fn walk(left: &Dataset, right: &Dataset, prefix: &str, out: &mut Vec<String>) {
        let tags: BTreeSet<_> = left
            .items()
            .chain(right.items())
            .map(|i| i.tag().clone())
            .collect();
        for tag in tags {
            let path = format!("{prefix}{tag}");
            match (left.item(&tag), right.item(&tag)) {
                (Some(DicomItem::Sequence(a)), Some(DicomItem::Sequence(b))) => {
                    if a.len() != b.len() {
                        out.push(format!(
                            "{path}: sequence length {} != {}",
                            a.len(),
                            b.len()
                        ));
                    }
                    for (index, (a, b)) in a.items().iter().zip(b.items()).enumerate() {
                        walk(a, b, &format!("{path}[{index}]/"), out);
                    }
                }
                (a, b) if a != b => out.push(format!("{path}: value, VR or presence differs")),
                _ => {}
            }
        }
    }
    let mut result = Vec::new();
    walk(left, right, "", &mut result);
    result
}
