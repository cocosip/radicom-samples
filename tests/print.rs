use radicom::model::{dataset::Dataset, tag::Tag, vr::Vr};
use radicom::print::models::FilmSession;
use radicom_network::{
    client::DicomClient,
    dimse::CommandField,
    print_sop::uids,
    server::{DicomServer, ServerOptions},
};
use radicom_samples::{
    print::{image_attributes, image_reference, PrintProvider},
    worklist::n_request,
};
use std::sync::Arc;

#[tokio::test]
async fn print_tcp_validates_references_and_records_one_image_job() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let provider = Arc::new(PrintProvider::default());
        let server = DicomServer::bind("127.0.0.1:0", provider.clone(), ServerOptions::default())
            .await
            .unwrap();
        let address = server.local_addr();
        let (stop, receiver) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(server.run_until(receiver));
        let mut client = DicomClient::new("SAMPLE_SCU", "PRINT", address.to_string());
        let mut session = FilmSession::new();
        session.set_sop_instance_uid("2.25.601");
        session.set_copies(1);
        let mut film = session.create_film_box();
        film.set_film_size("A4").unwrap();
        let mut film_data = film.into_dataset();
        film_data.insert_str(Tag::IMAGE_DISPLAY_FORMAT, Vr::St, "STANDARD\\1,1");
        let session_create = n_request(
            CommandField::NCreateRq,
            uids::BASIC_FILM_SESSION,
            "2.25.601",
            Some(session.into_dataset()),
        );
        let film_create = n_request(
            CommandField::NCreateRq,
            uids::BASIC_FILM_BOX,
            "2.25.602",
            Some(film_data.clone()),
        );
        assert_ne!(client.send_n_service(&film_create).await.unwrap().status, 0);
        assert_eq!(
            client.send_n_service(&session_create).await.unwrap().status,
            0
        );
        assert_ne!(
            client.send_n_service(&session_create).await.unwrap().status,
            0
        );
        let mut wrong_class = film_create.clone();
        wrong_class
            .dataset
            .as_mut()
            .unwrap()
            .sequence_mut(Tag::REFERENCED_FILM_SESSION_SEQUENCE)
            .unwrap()
            .items_mut()[0]
            .insert_str(Tag::REFERENCED_SOP_CLASS_UID, Vr::Ui, uids::BASIC_FILM_BOX);
        assert_ne!(client.send_n_service(&wrong_class).await.unwrap().status, 0);
        let response = client.send_n_service(&film_create).await.unwrap();
        assert_eq!(response.status, 0);
        let image_uid = image_reference(response.dataset.as_ref().unwrap()).unwrap();
        let mut action = n_request(
            CommandField::NActionRq,
            uids::BASIC_FILM_BOX,
            "2.25.602",
            None,
        );
        action.action_type_id = Some(1);
        assert_ne!(client.send_n_service(&action).await.unwrap().status, 0);
        let delete_session = n_request(
            CommandField::NDeleteRq,
            uids::BASIC_FILM_SESSION,
            "2.25.601",
            None,
        );
        assert_ne!(
            client.send_n_service(&delete_session).await.unwrap().status,
            0
        );
        let mut set = n_request(
            CommandField::NSetRq,
            uids::BASIC_GRAYSCALE_IMAGE_BOX,
            "2.25.999",
            Some(image_attributes().unwrap()),
        );
        assert_ne!(client.send_n_service(&set).await.unwrap().status, 0);
        set.sop_instance_uid = Some(image_uid.clone());
        set.dataset = Some(Dataset::new());
        assert_ne!(client.send_n_service(&set).await.unwrap().status, 0);
        set.dataset = Some(image_attributes().unwrap());
        assert_eq!(client.send_n_service(&set).await.unwrap().status, 0);
        action.action_type_id = Some(2);
        assert_ne!(client.send_n_service(&action).await.unwrap().status, 0);
        action.action_type_id = Some(1);
        assert_eq!(client.send_n_service(&action).await.unwrap().status, 0);
        assert_eq!(provider.recorded_jobs(), vec!["2.25.602"]);
        let delete_film = n_request(
            CommandField::NDeleteRq,
            uids::BASIC_FILM_BOX,
            "2.25.602",
            None,
        );
        assert_eq!(client.send_n_service(&delete_film).await.unwrap().status, 0);
        assert_ne!(client.send_n_service(&set).await.unwrap().status, 0);
        assert_ne!(client.send_n_service(&action).await.unwrap().status, 0);
        assert_ne!(client.send_n_service(&delete_film).await.unwrap().status, 0);
        assert_eq!(
            client.send_n_service(&delete_session).await.unwrap().status,
            0
        );
        client.release().await;
        stop.send(true).unwrap();
        task.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}
