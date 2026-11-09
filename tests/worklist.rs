use radicom::model::{dataset::Dataset, tag::Tag, vr::Vr};
use radicom_network::{
    client::DicomClient,
    server::{DicomServer, ServerOptions},
};
use radicom_samples::worklist::{
    mpps_create, mpps_set, query, WorklistProvider, MPPS_STATUS, SPS, SPS_DATE,
};
use std::sync::Arc;

#[tokio::test]
async fn worklist_tcp_matches_nested_keys_and_projects_requested_fields() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let provider = Arc::new(WorklistProvider::default());
        let server = DicomServer::bind("127.0.0.1:0", provider, ServerOptions::default())
            .await
            .unwrap();
        let address = server.local_addr();
        let (stop, receiver) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(server.run_until(receiver));
        let mut client = DicomClient::new("SAMPLE_SCU", "WORKLIST", address.to_string());
        let request = query("SAMPLE*", "CT", "20261101-20261130");
        let responses = client.send_c_find(&request).await.unwrap();
        assert_eq!(responses.last().unwrap().status, 0);
        let matches: Vec<_> = responses
            .iter()
            .filter_map(|r| r.identifier.as_ref())
            .collect();
        assert_eq!(matches.len(), 1);
        let matched = matches[0];
        assert_eq!(matched.get_str(Tag::PATIENT_ID).unwrap(), "SAMPLE001");
        assert!(!matched.contains(&Tag::PATIENT_BIRTH_DATE));
        assert_eq!(
            matched.get_sequence(SPS).unwrap().items()[0]
                .get_str(SPS_DATE)
                .unwrap(),
            "20261109"
        );
        assert!(!matched.get_sequence(SPS).unwrap().items()[0].contains(&Tag::new(0x0040, 0x0007)));
        for request in [query("*", "MR", ""), query("*", "CT", "20270101-")] {
            let responses = client.send_c_find(&request).await.unwrap();
            assert_eq!(responses.len(), 1);
            assert_eq!(responses[0].status, 0);
        }
        let mut bad = query("*", "CT", "");
        bad.identifier
            .insert_empty(Tag::new(0x7777, 0x0010), Vr::Lo);
        assert_ne!(
            client
                .send_c_find(&bad)
                .await
                .unwrap()
                .last()
                .unwrap()
                .status,
            0
        );
        assert_ne!(
            client
                .send_c_find(&query("*", "CT", "20260230"))
                .await
                .unwrap()
                .last()
                .unwrap()
                .status,
            0
        );
        client.release().await;
        stop.send(true).unwrap();
        task.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn mpps_tcp_checks_references_existence_and_final_states() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let provider = Arc::new(WorklistProvider::default());
        let server = DicomServer::bind("127.0.0.1:0", provider, ServerOptions::default())
            .await
            .unwrap();
        let address = server.local_addr();
        let (stop, receiver) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(server.run_until(receiver));
        let mut client = DicomClient::new("SAMPLE_SCU", "WORKLIST", address.to_string());
        assert_ne!(
            client
                .send_n_service(&mpps_set("2.25.501", "COMPLETED"))
                .await
                .unwrap()
                .status,
            0
        );
        let mut invalid = mpps_create("2.25.501");
        invalid
            .dataset
            .as_mut()
            .unwrap()
            .insert_str(MPPS_STATUS, Vr::Cs, "COMPLETED");
        assert_ne!(client.send_n_service(&invalid).await.unwrap().status, 0);
        let mut invalid = mpps_create("2.25.501");
        invalid
            .dataset
            .as_mut()
            .unwrap()
            .sequence_mut(Tag::new(0x0040, 0x0270))
            .unwrap()
            .items_mut()[0]
            .insert_str(Tag::STUDY_INSTANCE_UID, Vr::Ui, "2.25.999");
        assert_ne!(client.send_n_service(&invalid).await.unwrap().status, 0);
        for (uid, final_state) in [("2.25.501", "COMPLETED"), ("2.25.502", "DISCONTINUED")] {
            assert_eq!(
                client
                    .send_n_service(&mpps_create(uid))
                    .await
                    .unwrap()
                    .status,
                0
            );
            assert_ne!(
                client
                    .send_n_service(&mpps_create(uid))
                    .await
                    .unwrap()
                    .status,
                0
            );
            assert_ne!(
                client
                    .send_n_service(&mpps_set(uid, "IN PROGRESS"))
                    .await
                    .unwrap()
                    .status,
                0
            );
            let mut empty = mpps_set(uid, final_state);
            empty.dataset = Some(Dataset::new());
            assert_ne!(client.send_n_service(&empty).await.unwrap().status, 0);
            let mut changed_reference = mpps_set(uid, final_state);
            let mut reference = Dataset::new();
            reference.insert_str(Tag::STUDY_INSTANCE_UID, Vr::Ui, "2.25.999");
            changed_reference
                .dataset
                .as_mut()
                .unwrap()
                .insert_sequence(Tag::SCHEDULED_STEP_ATTRIBUTES_SEQUENCE)
                .push(reference);
            assert_ne!(
                client
                    .send_n_service(&changed_reference)
                    .await
                    .unwrap()
                    .status,
                0
            );
            assert_eq!(
                client
                    .send_n_service(&mpps_set(uid, final_state))
                    .await
                    .unwrap()
                    .status,
                0
            );
            assert_ne!(
                client
                    .send_n_service(&mpps_set(uid, final_state))
                    .await
                    .unwrap()
                    .status,
                0
            );
        }
        client.release().await;
        stop.send(true).unwrap();
        task.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}
