use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use radicom::{
    io::file::DicomFile,
    model::{tag::Tag, transfer_syntax::syntaxes, vr::Vr},
    pixel::PixelData,
};
use radicom_network::{
    association::sop_class,
    cfind::{CFindRequest, QueryRetrieveLevel, QueryRoot},
    cget::CGetRequest,
    cmove::CMoveRequest,
    dimse::status,
    server::{DicomServer, ServerOptions},
};
use radicom_samples::{
    files::{sample_file, save_instance},
    network::{client, receive_handler, FileProvider},
};

fn assert_synthetic_pixels(path: &Path) {
    let mut file = DicomFile::open(path).unwrap();
    let frame = PixelData::from_dataset(&mut file.dataset)
        .unwrap()
        .get_frame(0)
        .unwrap();
    assert_eq!(
        frame.data().unwrap().as_ref(),
        (0..=255).collect::<Vec<u8>>()
    );
}

async fn server(
    files: Vec<DicomFile>,
    storage: Option<PathBuf>,
    destinations: HashMap<String, String>,
) -> (
    String,
    tokio::task::JoinHandle<radicom_network::error::Result<()>>,
) {
    let provider = FileProvider::new(files, storage, destinations, "SAMPLES".into()).unwrap();
    let server = DicomServer::bind(
        "127.0.0.1:0",
        Arc::new(provider),
        ServerOptions {
            called_ae_title: Some("SAMPLES".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let address = server.local_addr().to_string();
    (address, tokio::spawn(server.run()))
}

#[tokio::test]
async fn echo_and_store_acknowledge_only_new_persisted_instances() {
    let directory = tempfile::tempdir().unwrap();
    let (address, task) = server(vec![], Some(directory.path().into()), HashMap::new()).await;
    let mut scu = client(&address, "TEST", "SAMPLES");
    assert_eq!(
        scu.send_c_echo(sop_class::VERIFICATION)
            .await
            .unwrap()
            .status,
        status::SUCCESS
    );
    let file = sample_file().unwrap();
    assert_eq!(
        scu.send_c_store(&file).await.unwrap().status,
        status::SUCCESS
    );
    let stored = DicomFile::open(directory.path().join("2.25.102.dcm")).unwrap();
    assert!(stored.dataset.content_equals(&file.dataset));
    assert_synthetic_pixels(&directory.path().join("2.25.102.dcm"));
    assert_ne!(
        scu.send_c_store(&file).await.unwrap().status,
        status::SUCCESS
    );
    scu.release().await;
    task.abort();

    let blocked = directory.path().join("not-a-directory");
    std::fs::write(&blocked, b"occupied").unwrap();
    let (address, task) = server(vec![], Some(blocked), HashMap::new()).await;
    let mut scu = client(&address, "TEST", "SAMPLES");
    assert_ne!(
        scu.send_c_store(&file).await.unwrap().status,
        status::SUCCESS
    );
    scu.release().await;
    task.abort();
}

#[tokio::test]
async fn find_levels_deduplicate_and_validate_keys_and_matching() {
    let mut first = sample_file().unwrap();
    first
        .dataset
        .insert_str(Tag::PATIENT_NAME, Vr::Pn, "SAMPLE^ONE");
    first
        .dataset
        .insert_str(Tag::STUDY_DATE, Vr::Da, "20261109");
    let mut second = first.clone();
    second
        .dataset
        .insert_str(Tag::SOP_INSTANCE_UID, Vr::Ui, "2.25.103");
    let (address, task) = server(vec![first, second], None, HashMap::new()).await;
    let mut scu = client(&address, "TEST", "SAMPLES");
    for (level, count) in [
        (QueryRetrieveLevel::Study, 1),
        (QueryRetrieveLevel::Series, 1),
        (QueryRetrieveLevel::Image, 2),
    ] {
        let mut query = CFindRequest::new(level, QueryRoot::Study, 0)
            .with_key(0x0010, 0x0010, Vr::Pn, "SAMPLE^*")
            .with_key(0x0008, 0x0020, Vr::Da, "20261101-20261130");
        if level != QueryRetrieveLevel::Study {
            query = query.with_key(0x0020, 0x000d, Vr::Ui, "2.25.100");
        }
        if level == QueryRetrieveLevel::Image {
            query = query.with_key(0x0020, 0x000e, Vr::Ui, "2.25.101");
        }
        let responses = scu.send_c_find(&query).await.unwrap();
        assert_eq!(responses.iter().filter(|r| r.is_pending()).count(), count);
        assert_eq!(responses.last().unwrap().status, status::SUCCESS);
    }
    let unsupported = CFindRequest::study_query(0).with_key(0x0010, 0x0040, Vr::Cs, "M");
    assert_eq!(
        scu.send_c_find(&unsupported)
            .await
            .unwrap()
            .last()
            .unwrap()
            .status,
        status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS
    );
    let invalid = CFindRequest::study_query(0).with_key(0x0020, 0x000d, Vr::Ui, "../unsafe");
    assert_eq!(
        scu.send_c_find(&invalid)
            .await
            .unwrap()
            .last()
            .unwrap()
            .status,
        status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS
    );
    let invalid_date = CFindRequest::study_query(0).with_key(0x0008, 0x0020, Vr::Da, "20260230");
    assert_eq!(
        scu.send_c_find(&invalid_date)
            .await
            .unwrap()
            .last()
            .unwrap()
            .status,
        status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS
    );
    scu.release().await;
    task.abort();
}

#[tokio::test]
async fn get_transcodes_rle_persists_reverse_stores_and_counts_duplicate_failures() {
    radicom_codecs::install();
    let directory = tempfile::tempdir().unwrap();
    let compressed = sample_file()
        .unwrap()
        .clone_with_syntax(syntaxes::RLE_LOSSLESS.clone(), None)
        .unwrap();
    let (address, task) = server(vec![compressed], None, HashMap::new()).await;
    let mut scu = client(&address, "TEST", "SAMPLES")
        .with_reverse_store_handler(receive_handler(directory.path().into()));
    let classes = vec!["1.2.840.10008.5.1.4.1.1.7".into()];
    for success in [true, false] {
        let responses = scu
            .send_c_get(&CGetRequest::study("2.25.100", 0), &classes)
            .await
            .unwrap();
        let final_response = responses.last().unwrap();
        assert_eq!(final_response.summary.completed, u16::from(success));
        assert_eq!(final_response.summary.failed, u16::from(!success));
        assert_eq!(final_response.status == status::SUCCESS, success);
        assert_eq!(final_response.remaining, 0);
    }
    assert_synthetic_pixels(&directory.path().join("2.25.102.dcm"));
    scu.release().await;
    task.abort();
}

#[tokio::test]
async fn move_uses_explicit_destination_and_reports_failures() {
    let directory = tempfile::tempdir().unwrap();
    let (destination, destination_task) =
        server(vec![], Some(directory.path().into()), HashMap::new()).await;
    let (address, task) = server(
        vec![sample_file().unwrap()],
        None,
        HashMap::from([("SAMPLES".into(), destination)]),
    )
    .await;
    let mut scu = client(&address, "TEST", "SAMPLES");
    let unknown = scu
        .send_c_move(&CMoveRequest::study("2.25.100", "UNKNOWN", 0))
        .await
        .unwrap();
    assert_eq!(
        unknown.last().unwrap().status,
        status::MOVE_DESTINATION_UNKNOWN
    );
    for success in [true, false] {
        let responses = scu
            .send_c_move(&CMoveRequest::study("2.25.100", "SAMPLES", 0))
            .await
            .unwrap();
        assert!(responses.iter().any(|response| response.is_pending()));
        let final_response = responses.last().unwrap();
        assert_eq!(final_response.summary.completed, u16::from(success));
        assert_eq!(final_response.summary.failed, u16::from(!success));
        assert_eq!(final_response.status == status::SUCCESS, success);
        assert_eq!(
            final_response.failed_sop_instance_uids.len(),
            usize::from(!success)
        );
    }
    assert_synthetic_pixels(&directory.path().join("2.25.102.dcm"));
    scu.release().await;
    task.abort();
    destination_task.abort();
}

#[tokio::test]
async fn retrieve_rejects_unsupported_keys_and_missing_hierarchy() {
    let (address, task) = server(
        vec![sample_file().unwrap()],
        None,
        HashMap::from([("SAMPLES".into(), "127.0.0.1:1".into())]),
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let mut scu = client(&address, "TEST", "SAMPLES")
        .with_reverse_store_handler(receive_handler(directory.path().into()));
    let get = CGetRequest::study("2.25.100", 0).with_key(0x0010, 0x0040, Vr::Cs, "O");
    let responses = scu
        .send_c_get(&get, &["1.2.840.10008.5.1.4.1.1.7".into()])
        .await
        .unwrap();
    assert_eq!(
        responses.last().unwrap().status,
        status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS
    );
    let demographic_retrieve =
        CGetRequest::study("2.25.100", 0).with_key(0x0010, 0x0010, Vr::Pn, "SAMPLE*");
    let responses = scu
        .send_c_get(&demographic_retrieve, &["1.2.840.10008.5.1.4.1.1.7".into()])
        .await
        .unwrap();
    assert_eq!(
        responses.last().unwrap().status,
        status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS
    );
    let request =
        CMoveRequest::study("2.25.100", "SAMPLES", 0).with_key(0x0010, 0x0040, Vr::Cs, "O");
    assert_eq!(
        scu.send_c_move(&request)
            .await
            .unwrap()
            .last()
            .unwrap()
            .status,
        status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS
    );
    let request =
        CMoveRequest::new(QueryRetrieveLevel::Series, QueryRoot::Study, "SAMPLES", 0).unwrap();
    assert_eq!(
        scu.send_c_move(&request)
            .await
            .unwrap()
            .last()
            .unwrap()
            .status,
        status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS
    );
    scu.release().await;
    task.abort();
}

#[tokio::test]
async fn series_and_image_retrieve_report_mixed_suboperation_outcomes() {
    let first = sample_file().unwrap();
    let mut second = first.clone();
    second
        .dataset
        .insert_str(Tag::SOP_INSTANCE_UID, Vr::Ui, "2.25.103");
    let destination_dir = tempfile::tempdir().unwrap();
    save_instance(&first, destination_dir.path()).unwrap();
    let (destination, destination_task) =
        server(vec![], Some(destination_dir.path().into()), HashMap::new()).await;
    let (address, task) = server(
        vec![first.clone(), second],
        None,
        HashMap::from([("SAMPLES".into(), destination)]),
    )
    .await;
    let get_dir = tempfile::tempdir().unwrap();
    save_instance(&first, get_dir.path()).unwrap();
    let mut scu = client(&address, "TEST", "SAMPLES")
        .with_reverse_store_handler(receive_handler(get_dir.path().into()));
    let responses = scu
        .send_c_get(
            &CGetRequest::series("2.25.100", "2.25.101", 0),
            &["1.2.840.10008.5.1.4.1.1.7".into()],
        )
        .await
        .unwrap();
    let final_response = responses.last().unwrap();
    assert_eq!(
        final_response.status,
        status::SUBOPERATIONS_COMPLETED_WITH_WARNINGS
    );
    assert_eq!(final_response.summary.completed, 1);
    assert_eq!(final_response.summary.failed, 1);
    assert_eq!(final_response.failed_sop_instance_uids, vec!["2.25.102"]);
    assert!(get_dir.path().join("2.25.103.dcm").exists());
    assert_synthetic_pixels(&get_dir.path().join("2.25.103.dcm"));
    let responses = scu
        .send_c_move(&CMoveRequest::series("2.25.100", "2.25.101", "SAMPLES", 0))
        .await
        .unwrap();
    let final_response = responses.last().unwrap();
    assert_eq!(
        final_response.status,
        status::SUBOPERATIONS_COMPLETED_WITH_WARNINGS
    );
    assert_eq!(final_response.summary.completed, 1);
    assert_eq!(final_response.summary.failed, 1);
    assert_eq!(final_response.failed_sop_instance_uids, vec!["2.25.102"]);
    assert!(destination_dir.path().join("2.25.103.dcm").exists());
    assert_synthetic_pixels(&destination_dir.path().join("2.25.103.dcm"));
    let image_dir = tempfile::tempdir().unwrap();
    scu.release().await;
    scu = client(&address, "TEST", "SAMPLES")
        .with_reverse_store_handler(receive_handler(image_dir.path().into()));
    let responses = scu
        .send_c_get(
            &CGetRequest::image("2.25.100", "2.25.101", "2.25.103", 0),
            &["1.2.840.10008.5.1.4.1.1.7".into()],
        )
        .await
        .unwrap();
    assert_eq!(responses.last().unwrap().summary.completed, 1);
    assert_eq!(responses.last().unwrap().status, status::SUCCESS);
    assert_synthetic_pixels(&image_dir.path().join("2.25.103.dcm"));
    let responses = scu
        .send_c_move(&CMoveRequest::image(
            "2.25.100", "2.25.101", "2.25.103", "SAMPLES", 0,
        ))
        .await
        .unwrap();
    assert_eq!(responses.last().unwrap().summary.failed, 1);
    assert_eq!(responses.last().unwrap().summary.completed, 0);
    scu.release().await;
    task.abort();
    destination_task.abort();
}
