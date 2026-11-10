use radicom_samples::{files, wado};
use std::{net::SocketAddr, path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const QUERY: &str = "requestType=WADO&studyUID=2.25.100&seriesUID=2.25.101&objectUID=2.25.102&contentType=application%2Fdicom";

async fn serve(app: axum::Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, task)
}

async fn server(directory: &Path) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    serve(wado::router(directory).unwrap()).await
}

async fn request(address: SocketAddr, query: &str, accept: &str) -> (u16, String, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(format!("GET /wado?{query} HTTP/1.1\r\nHost: localhost\r\nAccept: {accept}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        let end = bytes.windows(4).position(|x| x == b"\r\n\r\n").unwrap();
        let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, headers, bytes[end + 4..].to_vec())
    }).await.unwrap()
}

fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    files::save_new(
        &files::sample_file().unwrap(),
        &directory.path().join("synthetic.dcm"),
    )
    .unwrap();
    directory
}

#[tokio::test]
async fn returns_original_dicom_bytes_and_media_type() {
    let directory = fixture();
    let expected = std::fs::read(directory.path().join("synthetic.dcm")).unwrap();
    let (address, task) = server(directory.path()).await;
    std::fs::remove_file(directory.path().join("synthetic.dcm")).unwrap();
    for accept in [
        "application/dicom",
        "*/*",
        "application/*",
        "image/jpeg;q=0.8, application/dicom;q=0.5",
    ] {
        let (status, headers, body) =
            request(address, &format!("{QUERY}&transferSyntax=*"), accept).await;
        assert_eq!(status, 200, "{accept}");
        assert!(headers
            .to_lowercase()
            .contains("content-type: application/dicom\r\n"));
        assert_eq!(body, expected);
    }
    task.abort();
}

#[tokio::test]
async fn requires_all_three_identifiers_to_match() {
    let directory = fixture();
    let (address, task) = server(directory.path()).await;
    for uid in ["2.25.100", "2.25.101", "2.25.102"] {
        let query = QUERY.replace(uid, "2.25.999");
        assert_eq!(request(address, &query, "application/dicom").await.0, 404);
    }
    task.abort();
}

#[tokio::test]
async fn built_in_archive_needs_no_directory() {
    let (address, task) = serve(wado::sample_router().unwrap()).await;
    for object in ["2.25.102", "2.25.103"] {
        let query = QUERY.replace("2.25.102", object);
        let (status, _, body) = request(
            address,
            &format!("{query}&transferSyntax=*"),
            "application/dicom",
        )
        .await;
        assert_eq!(status, 200, "{object}");
        let file = radicom::io::file::DicomFile::from_bytes(&body, &Default::default()).unwrap();
        assert_eq!(
            file.dataset
                .get_str(radicom::Tag::SOP_INSTANCE_UID)
                .unwrap(),
            object
        );
    }
    task.abort();
}

#[tokio::test]
async fn rejects_rendering_and_conflicting_accept() {
    let directory = fixture();
    let (address, task) = server(directory.path()).await;
    for query in [
        QUERY.replace("application%2Fdicom", "image%2Fjpeg"),
        QUERY.replace("&contentType=application%2Fdicom", ""),
    ] {
        assert_eq!(request(address, &query, "*/*").await.0, 406);
    }
    for accept in [
        "image/jpeg",
        "application/dicom;q=0, */*;q=1",
        "application/*;q=0, */*;q=1",
        "application/dicom;q=banana",
        "application/dicom;transfer-syntax=1.2.3",
    ] {
        assert_eq!(request(address, QUERY, accept).await.0, 406, "{accept}");
    }
    task.abort();
}

#[test]
fn rejects_ambiguous_duplicate_instances_at_startup() {
    let directory = fixture();
    std::fs::copy(
        directory.path().join("synthetic.dcm"),
        directory.path().join("duplicate.dcm"),
    )
    .unwrap();
    assert!(wado::router(directory.path()).is_err());
}

#[tokio::test]
async fn rejects_malformed_uids_duplicate_and_unsupported_controls() {
    let directory = fixture();
    let (address, task) = server(directory.path()).await;
    let cases = [
        QUERY.replace("requestType=WADO", "requestType=OTHER"),
        QUERY.replace("2.25.102", "..%2Fsecret"),
        QUERY.replace("2.25.102", "2..25"),
        QUERY.replace("&seriesUID=2.25.101", ""),
        format!("{QUERY}&objectUID=2.25.103"),
        format!("{QUERY}&rows=64"),
        format!("{QUERY}&transferSyntax=..%2Fsecret"),
        format!("{QUERY}&anonymize=yes"),
    ];
    for query in cases {
        assert_eq!(
            request(address, &query, "application/dicom").await.0,
            400,
            "{query}"
        );
    }
    task.abort();
}

#[tokio::test]
async fn defaults_to_explicit_little_endian_and_supports_requested_lossless_syntax() {
    use radicom::{io::file::DicomFile, model::transfer_syntax::syntaxes, pixel::PixelDataRef};
    let directory = tempfile::tempdir().unwrap();
    let source = files::sample_file()
        .unwrap()
        .clone_with_syntax(syntaxes::IMPLICIT_VR_LITTLE_ENDIAN.clone(), None)
        .unwrap();
    files::save_new(&source, &directory.path().join("implicit.dcm")).unwrap();
    let expected = (0..=255).collect::<Vec<u8>>();
    let (address, task) = server(directory.path()).await;
    for (parameter, syntax) in [
        ("", "1.2.840.10008.1.2.1"),
        ("&transferSyntax=1.2.840.10008.1.2.5", "1.2.840.10008.1.2.5"),
    ] {
        let (status, _, body) =
            request(address, &format!("{QUERY}{parameter}"), "application/dicom").await;
        assert_eq!(status, 200, "{parameter}");
        let result = DicomFile::from_bytes(&body, &Default::default()).unwrap();
        assert_eq!(result.dataset.transfer_syntax().uid(), syntax);
        let decoded = result
            .clone_with_syntax(syntaxes::EXPLICIT_VR_LITTLE_ENDIAN.clone(), None)
            .unwrap();
        assert_eq!(
            PixelDataRef::from_dataset(&decoded.dataset)
                .unwrap()
                .get_frame(0)
                .unwrap()
                .data()
                .unwrap()
                .as_ref(),
            expected
        );
    }
    assert_eq!(
        request(
            address,
            &format!("{QUERY}&transferSyntax=2.25.999"),
            "application/dicom"
        )
        .await
        .0,
        406
    );
    task.abort();
}
