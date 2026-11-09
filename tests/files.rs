use radicom::io::file::{DicomFile, ReadOptions};
use radicom::model::item::{DicomItem, DicomSequence};
use radicom::model::transfer_syntax::syntaxes;
use radicom::pixel::PixelData;
use radicom::{Dataset, Tag, Vr};
use radicom_samples::files::*;
use radicom_samples::Result;
use std::path::Path;

#[test]
fn synthetic_file_preserves_pixels_without_overwriting() -> Result {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("sample.dcm");
    save_new(&sample_file()?, &path)?;
    let original = std::fs::read(&path)?;
    assert!(save_new(&sample_file()?, &path).is_err());
    assert_eq!(std::fs::read(&path)?, original);
    let file = DicomFile::open(&path)?;
    assert_eq!(file.dataset.get_str(Tag::PATIENT_NAME)?, "SAMPLE^SYNTHETIC");
    let pixels = PixelData::from_dataset(&mut file.dataset.clone())?.get_frame(0)?;
    assert_eq!(pixels.data()?.as_ref(), (0..=255).collect::<Vec<u8>>());
    Ok(())
}

#[test]
fn received_uid_cannot_escape_storage_or_replace_a_file() -> Result {
    let directory = tempfile::tempdir()?;
    for uid in ["../x", "", "1..2", "1.02.3", "1/2", "1.2\\3"] {
        assert!(validate_uid(uid).is_err(), "accepted {uid}");
    }
    let mut file = sample_file()?;
    file.dataset
        .insert_str(Tag::SOP_INSTANCE_UID, Vr::Ui, "../x");
    assert!(save_instance(&file, directory.path()).is_err());
    let file = sample_file()?;
    let path = save_instance(&file, directory.path())?;
    assert_eq!(path.file_name().unwrap(), "2.25.102.dcm");
    assert!(save_instance(&file, directory.path()).is_err());
    Ok(())
}

#[test]
fn media_references_resolve_to_copied_instances() -> Result {
    let parent = tempfile::tempdir()?;
    let output = parent.path().join("MEDIA");
    write_media(&[sample_file()?], &output)?;
    let directory = radicom::media::DicomDirectory::open(output.join("DICOMDIR"))?;
    assert_eq!(directory.walk().len(), 4);
    let leaves: Vec<_> = directory
        .records()
        .iter()
        .filter_map(|r| r.referenced_file_id())
        .collect();
    assert_eq!(leaves.len(), 1);
    let file = DicomFile::open(output.join(&leaves[0]))?;
    assert_eq!(file.dataset.get_str(Tag::SOP_INSTANCE_UID)?, "2.25.102");
    assert!(write_media(&[sample_file()?], &output).is_err());
    Ok(())
}

#[test]
fn comparison_reports_nested_and_binary_changes() -> Result {
    let mut left = Dataset::new();
    left.insert_value(Tag::PIXEL_DATA, vec![0u8, 1]);
    let mut child = Dataset::new();
    child.insert_str(Tag::PATIENT_ID, Vr::Lo, "LEFT");
    left.insert(DicomItem::Sequence(DicomSequence::with_items(
        Tag::REFERENCED_PATIENT_SEQUENCE,
        vec![child],
    )))?;
    let mut right = left.clone();
    assert!(differences(&left, &right).is_empty());
    right.insert_value(Tag::PIXEL_DATA, vec![0u8, 2]);
    right
        .sequence_mut(Tag::REFERENCED_PATIENT_SEQUENCE)
        .unwrap()
        .items_mut()[0]
        .insert_str(Tag::PATIENT_ID, Vr::Lo, "RIGHT");
    let changes = differences(&left, &right);
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().any(|s| s.contains("7FE0")));
    assert!(changes
        .iter()
        .any(|s| s.contains("[0]") && s.contains("0020")));
    Ok(())
}

#[test]
fn all_lossless_codec_families_restore_original_pixels() -> Result {
    radicom_codecs::install();
    let source = sample_file()?;
    for syntax in [
        syntaxes::RLE_LOSSLESS.clone(),
        syntaxes::JPEG_LOSSLESS_PROCESS_14_SV1.clone(),
        syntaxes::JPEG_LS_LOSSLESS.clone(),
        syntaxes::JPEG_2000_LOSSLESS.clone(),
        syntaxes::HTJ2K_LOSSLESS.clone(),
    ] {
        let compressed = source.clone_with_syntax(syntax.clone(), None)?;
        let bytes = compressed.to_bytes(&Default::default())?;
        let persisted = DicomFile::from_bytes(&bytes, &ReadOptions::default())?;
        assert_eq!(persisted.dataset.transfer_syntax().uid(), syntax.uid());
        let mut restored =
            persisted.clone_with_syntax(syntaxes::EXPLICIT_VR_LITTLE_ENDIAN.clone(), None)?;
        let pixels = PixelData::from_dataset(&mut restored.dataset)?.get_frame(0)?;
        assert_eq!(
            pixels.data()?.as_ref(),
            (0..=255).collect::<Vec<u8>>(),
            "{}",
            syntax.uid()
        );
    }
    Ok(())
}

#[test]
fn discovery_skips_unrelated_files_but_rejects_corrupt_dicom() -> Result {
    let directory = tempfile::tempdir()?;
    save_new(&sample_file()?, &directory.path().join("ok.DCM"))?;
    std::fs::write(directory.path().join("notes.txt"), "not DICOM")?;
    assert_eq!(load_files(directory.path())?.len(), 1);
    std::fs::write(directory.path().join("bad.dcm"), "broken")?;
    assert!(load_files(directory.path()).is_err());
    Ok(())
}

#[test]
fn media_reader_rejects_external_links_and_lexical_traversal() -> Result {
    let directory = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    std::fs::write(outside.path().join("00000001"), b"outside the file-set")?;
    assert!(media_reference(directory.path(), Path::new("../00000001")).is_err());
    assert!(media_reference(directory.path(), outside.path()).is_err());
    let link = directory.path().join("IMAGES");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), &link)?;
    #[cfg(windows)]
    {
        // Junction creation needs no symlink privilege on ordinary Windows CI.
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:RADICOM_TEST_LINK -Value $env:RADICOM_TEST_TARGET | Out-Null"])
            .env("RADICOM_TEST_LINK", &link).env("RADICOM_TEST_TARGET", outside.path())
            .status()?;
        assert!(status.success(), "could not create the test junction");
    }
    assert!(media_reference(directory.path(), Path::new("IMAGES/00000001")).is_err());
    Ok(())
}
