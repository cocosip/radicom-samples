//! Small file-backed Study Root SCP. Identifiers support only the keys below.
//!
//! Text matching is case-sensitive (`*`/`?`); UIDs accept exact values or a
//! backslash-separated list. Study Date accepts YYYYMMDD and open/closed ranges.
//! This is a startup snapshot, not an indexed or dynamically updated PACS.

use crate::{
    files::{save_instance, validate_uid},
    Result,
};
use async_trait::async_trait;
use futures::{stream, stream::BoxStream, StreamExt};
use radicom::{
    io::file::DicomFile,
    model::{dataset::Dataset, dicom_datetime::parse_da, tag::Tag, vr::Vr},
    rules::dicom_wildcard_match,
    uids,
};
use radicom_network::{
    association::sop_class,
    cfind::{CFindRequest, CFindResponse, QueryRetrieveLevel},
    cget::{CGetEvent, CGetRequest, CGetResponse},
    client::DicomClient,
    cmove::{store_to_destination_with_failures, CMoveRequest, CMoveResponse, SubOperationSummary},
    dimse::status,
    server::{ClientOptions, DicomServer, ReverseStoreHandler, ServerOptions},
    services::{CEchoRequest, CEchoResponse, CStoreRequest, CStoreResponse, ServiceProvider},
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

/// Finite transport timeouts keep a failed peer from hanging a demo forever.
pub fn client(address: &str, calling_ae: &str, called_ae: &str) -> DicomClient {
    let mut client = DicomClient::new(calling_ae, called_ae, address);
    client.options.connection = client_options();
    client
}

fn client_options() -> ClientOptions {
    ClientOptions {
        connect_timeout: Some(Duration::from_secs(5)),
        request_timeout: Some(Duration::from_secs(15)),
        ..Default::default()
    }
}

pub async fn run_server<P: ServiceProvider + 'static>(
    address: &str,
    ae: &str,
    provider: P,
) -> Result<()> {
    let server = DicomServer::bind(
        address,
        Arc::new(provider),
        ServerOptions {
            called_ae_title: Some(ae.into()),
            request_timeout: Some(Duration::from_secs(15)),
            ..Default::default()
        },
    )
    .await?;
    println!("Listening at {} as {ae}; Ctrl+C stops", server.local_addr());
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(server.run_until(receiver));
    tokio::signal::ctrl_c().await?;
    shutdown.send(true)?;
    task.await??;
    Ok(())
}

/// A STORE is acknowledged only after UID validation and create-new persistence.
fn persist(request: &CStoreRequest, directory: &std::path::Path) -> CStoreResponse {
    let result = (|| -> Result<()> {
        let uid = request.file.dataset.get_str(Tag::SOP_INSTANCE_UID)?;
        let class = request.file.dataset.get_str(Tag::SOP_CLASS_UID)?;
        validate_uid(&uid)?;
        validate_uid(&class)?;
        if uid != request.affected_sop_instance_uid || class != request.affected_sop_class_uid {
            return Err("command and dataset SOP UIDs differ".into());
        }
        save_instance(&request.file, directory)?;
        Ok(())
    })();
    match result {
        Ok(()) => CStoreResponse::success(request),
        Err(error) => CStoreResponse::failure(request, status::STORAGE_FAILURE, error.to_string()),
    }
}

pub fn receive_handler(directory: PathBuf) -> ReverseStoreHandler {
    Arc::new(move |request| Ok(persist(request, &directory)))
}

pub struct FileProvider {
    files: Vec<DicomFile>,
    storage: Option<PathBuf>,
    destinations: HashMap<String, String>,
    ae: String,
    storage_classes: Vec<String>,
}

impl FileProvider {
    pub fn new(
        files: Vec<DicomFile>,
        storage: Option<PathBuf>,
        destinations: HashMap<String, String>,
        ae: String,
    ) -> Result<Self> {
        if files.len() > u16::MAX as usize {
            return Err("sample supports at most 65535 instances".into());
        }
        let mut seen = HashSet::new();
        let mut storage_classes = vec![
            uids::SECONDARY_CAPTURE_IMAGE_STORAGE.into(),
            uids::CT_IMAGE_STORAGE.into(),
            uids::MR_IMAGE_STORAGE.into(),
        ];
        for file in &files {
            for tag in [
                Tag::SOP_CLASS_UID,
                Tag::SOP_INSTANCE_UID,
                Tag::STUDY_INSTANCE_UID,
                Tag::SERIES_INSTANCE_UID,
            ] {
                validate_uid(&file.dataset.get_str(tag)?)?;
            }
            if !seen.insert(file.dataset.get_str(Tag::SOP_INSTANCE_UID)?) {
                return Err("duplicate SOP Instance UID in archive".into());
            }
            storage_classes.push(file.dataset.get_str(Tag::SOP_CLASS_UID)?);
        }
        storage_classes.sort();
        storage_classes.dedup();
        Ok(Self {
            files,
            storage,
            destinations,
            ae,
            storage_classes,
        })
    }

    /// Add a storage class explicitly when receiving a modality not in the demo defaults.
    pub fn add_storage_class(&mut self, uid: String) -> Result<()> {
        validate_uid(&uid)?;
        if !self.storage_classes.contains(&uid) {
            self.storage_classes.push(uid);
        }
        Ok(())
    }

    fn select(
        &self,
        identifier: &Dataset,
        retrieve: bool,
    ) -> Result<(QueryRetrieveLevel, Vec<&DicomFile>)> {
        let level = QueryRetrieveLevel::parse(&identifier.get_str(Tag::QUERY_RETRIEVE_LEVEL)?)
            .ok_or("invalid query level")?;
        if !matches!(
            level,
            QueryRetrieveLevel::Study | QueryRetrieveLevel::Series | QueryRetrieveLevel::Image
        ) {
            return Err("only Study Root STUDY/SERIES/IMAGE supported".into());
        }
        for item in identifier.items() {
            let tag = item.tag();
            if tag == &Tag::QUERY_RETRIEVE_LEVEL {
                continue;
            }
            if retrieve && !uid_key(tag) {
                return Err("GET/MOVE identifiers accept only unique UID keys".into());
            }
            if !KEYS.iter().any(|(key, _)| key == tag) {
                return Err(format!("unsupported query key {tag}").into());
            }
            let value = identifier.get_str(tag)?;
            if value.is_empty() {
                continue;
            }
            if uid_key(tag) {
                for uid in value.split('\\') {
                    validate_uid(uid)?;
                }
            } else if tag == &Tag::STUDY_DATE {
                validate_date_query(&value)?;
            } else if !text_key(tag) {
                return Err(format!("key {tag} supports return only").into());
            }
        }
        for required in [
            Tag::STUDY_INSTANCE_UID,
            Tag::SERIES_INSTANCE_UID,
            Tag::SOP_INSTANCE_UID,
        ]
        .into_iter()
        .take(match level {
            QueryRetrieveLevel::Study => usize::from(retrieve),
            QueryRetrieveLevel::Series => {
                if retrieve {
                    2
                } else {
                    1
                }
            }
            _ => {
                if retrieve {
                    3
                } else {
                    2
                }
            }
        }) {
            let value = identifier.get_str(required)?;
            validate_uid(&value)?;
        }
        let selected = self
            .files
            .iter()
            .filter(|file| {
                identifier.items().all(|item| {
                    let tag = item.tag();
                    if tag == &Tag::QUERY_RETRIEVE_LEVEL {
                        return true;
                    }
                    let pattern = identifier.try_get_str(tag).unwrap_or_default();
                    if pattern.is_empty() {
                        return true;
                    }
                    let value = file.dataset.try_get_str(tag).unwrap_or_default();
                    if uid_key(tag) {
                        pattern.split('\\').any(|uid| uid == value)
                    } else if tag == &Tag::STUDY_DATE {
                        date_matches(&pattern, &value)
                    } else {
                        dicom_wildcard_match(&value, &pattern)
                    }
                })
            })
            .collect();
        Ok((level, selected))
    }
}

// Return keys mirror the released CFindRequest constructors; nonempty matching
// keys are limited to UIDs, PatientName/ID, AccessionNumber, Modality, StudyDate.
const KEYS: &[(Tag, Vr)] = &[
    (Tag::PATIENT_NAME, Vr::Pn),
    (Tag::PATIENT_ID, Vr::Lo),
    (Tag::STUDY_INSTANCE_UID, Vr::Ui),
    (Tag::SERIES_INSTANCE_UID, Vr::Ui),
    (Tag::SOP_INSTANCE_UID, Vr::Ui),
    (Tag::STUDY_DATE, Vr::Da),
    (Tag::STUDY_TIME, Vr::Tm),
    (Tag::ACCESSION_NUMBER, Vr::Sh),
    (Tag::MODALITY, Vr::Cs),
    (Tag::MODALITIES_IN_STUDY, Vr::Cs),
    (Tag::REFERRING_PHYSICIAN_NAME, Vr::Pn),
    (Tag::STUDY_ID, Vr::Sh),
    (Tag::SERIES_NUMBER, Vr::Is),
    (Tag::INSTANCE_NUMBER, Vr::Is),
];

fn uid_key(tag: &Tag) -> bool {
    [
        Tag::STUDY_INSTANCE_UID,
        Tag::SERIES_INSTANCE_UID,
        Tag::SOP_INSTANCE_UID,
    ]
    .contains(tag)
}
fn text_key(tag: &Tag) -> bool {
    [
        Tag::PATIENT_NAME,
        Tag::PATIENT_ID,
        Tag::ACCESSION_NUMBER,
        Tag::MODALITY,
    ]
    .contains(tag)
}

fn valid_date(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|c| c.is_ascii_digit()) && parse_da(value).is_ok()
}
fn validate_date_query(pattern: &str) -> Result<()> {
    if let Some((start, end)) = pattern.split_once('-') {
        if (start.is_empty() && end.is_empty())
            || (!start.is_empty() && !valid_date(start))
            || (!end.is_empty() && !valid_date(end))
            || (!start.is_empty() && !end.is_empty() && start > end)
        {
            return Err("invalid Study Date range".into());
        }
    } else if !valid_date(pattern) {
        return Err("Study Date must be YYYYMMDD or a date range".into());
    }
    Ok(())
}
fn date_matches(pattern: &str, value: &str) -> bool {
    if !valid_date(value) {
        return false;
    }
    if let Some((start, end)) = pattern.split_once('-') {
        (start.is_empty() || value >= start) && (end.is_empty() || value <= end)
    } else {
        pattern == value
    }
}
#[async_trait]
impl ServiceProvider for FileProvider {
    fn supported_sop_classes(&self) -> Vec<String> {
        let mut classes: Vec<String> = [
            sop_class::VERIFICATION,
            sop_class::STUDY_ROOT_QR_FIND,
            sop_class::STUDY_ROOT_QR_GET,
            sop_class::STUDY_ROOT_QR_MOVE,
        ]
        .into_iter()
        .map(String::from)
        .collect();
        // GET needs storage presentation contexts even when this archive is read-only.
        classes.extend(self.storage_classes.clone());
        classes
    }
    async fn on_c_echo(
        &self,
        request: &CEchoRequest,
    ) -> radicom_network::error::Result<CEchoResponse> {
        Ok(CEchoResponse::success(request))
    }
    async fn on_c_store(
        &self,
        request: &CStoreRequest,
    ) -> radicom_network::error::Result<CStoreResponse> {
        Ok(match &self.storage {
            Some(directory) => persist(request, directory),
            None => CStoreResponse::failure(request, status::STORAGE_FAILURE, "read-only archive"),
        })
    }
    fn on_c_find<'a>(
        &'a self,
        request: &'a CFindRequest,
    ) -> BoxStream<'a, radicom_network::error::Result<CFindResponse>> {
        let mut responses = Vec::new();
        match self.select(&request.identifier, false) {
            Err(error) => responses.push(Ok(CFindResponse::failure(
                request,
                status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS,
                error.to_string(),
            ))),
            Ok((level, files)) => {
                let entity = match level {
                    QueryRetrieveLevel::Study => Tag::STUDY_INSTANCE_UID,
                    QueryRetrieveLevel::Series => Tag::SERIES_INSTANCE_UID,
                    _ => Tag::SOP_INSTANCE_UID,
                };
                let mut seen = HashSet::new();
                for file in files {
                    if !seen.insert(file.dataset.try_get_str(&entity).unwrap_or_default()) {
                        continue;
                    }
                    let mut identifier = Dataset::new();
                    identifier.insert_str(Tag::QUERY_RETRIEVE_LEVEL, Vr::Cs, level.as_str());
                    for (tag, vr) in KEYS {
                        if request.identifier.contains(tag)
                            || tag == &Tag::STUDY_INSTANCE_UID
                            || (level != QueryRetrieveLevel::Study
                                && tag == &Tag::SERIES_INSTANCE_UID)
                            || (level == QueryRetrieveLevel::Image && tag == &Tag::SOP_INSTANCE_UID)
                        {
                            identifier.insert_str(
                                tag.clone(),
                                *vr,
                                file.dataset.try_get_str(tag).unwrap_or_default(),
                            );
                        }
                    }
                    responses.push(Ok(CFindResponse::pending(request, identifier)));
                }
                responses.push(Ok(CFindResponse::success(request, seen.len() as u16)));
            }
        }
        Box::pin(stream::iter(responses))
    }
    fn on_c_get<'a>(
        &'a self,
        request: &'a CGetRequest,
    ) -> BoxStream<'a, radicom_network::error::Result<CGetEvent>> {
        let mut events = Vec::new();
        match self.select(&request.identifier, true) {
            Err(error) => events.push(Ok(CGetEvent::Response(CGetResponse::failure(
                request,
                status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS,
                error.to_string(),
            )))),
            Ok((_, files)) => {
                events.push(Ok(CGetEvent::Response(CGetResponse::pending(
                    request,
                    files.len() as u16,
                    SubOperationSummary::default(),
                ))));
                for (index, file) in files.iter().enumerate() {
                    events.push(Ok(CGetEvent::instance((*file).clone())));
                    events.push(Ok(CGetEvent::Response(CGetResponse::pending(
                        request,
                        (files.len() - index - 1) as u16,
                        SubOperationSummary::default(),
                    ))));
                }
                // The association engine replaces these counters with actual STORE outcomes.
                events.push(Ok(CGetEvent::Response(CGetResponse::completed(
                    request,
                    SubOperationSummary::default(),
                ))));
            }
        }
        Box::pin(stream::iter(events))
    }
    fn on_c_move<'a>(
        &'a self,
        request: &'a CMoveRequest,
    ) -> BoxStream<'a, radicom_network::error::Result<CMoveResponse>> {
        let Some(destination) = self.destinations.get(&request.destination_ae) else {
            return Box::pin(stream::iter([Ok(CMoveResponse::failure(
                request,
                status::MOVE_DESTINATION_UNKNOWN,
                "destination AE is not configured",
            ))]));
        };
        let files = match self.select(&request.identifier, true) {
            Ok((_, files)) => files.into_iter().cloned().collect::<Vec<_>>(),
            Err(error) => {
                return Box::pin(stream::iter([Ok(CMoveResponse::failure(
                    request,
                    status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS,
                    error.to_string(),
                ))]))
            }
        };
        let pending =
            CMoveResponse::pending(request, files.len() as u16, SubOperationSummary::default());
        Box::pin(stream::iter([Ok(pending)]).chain(stream::once(async move {
            let result = store_to_destination_with_failures(
                destination.as_str(),
                &self.ae,
                &request.destination_ae,
                &files,
                // shortcut: provider API omits requester AE; add association context before tagging MOVE originator.
                None,
                client_options(),
            )
            .await;
            let mut response = match result {
                Ok((summary, failed, error)) => {
                    let mut response = CMoveResponse::completed(request, summary);
                    response.failed_sop_instance_uids = failed;
                    response.error_comment = error.map(|error| error.to_string());
                    response
                }
                Err(error) => {
                    let mut response = CMoveResponse::completed(
                        request,
                        SubOperationSummary {
                            failed: files.len() as u16,
                            ..Default::default()
                        },
                    );
                    response.error_comment = Some(error.to_string());
                    response.failed_sop_instance_uids = files
                        .iter()
                        .filter_map(|file| file.dataset.try_get_str(Tag::SOP_INSTANCE_UID))
                        .collect();
                    response
                }
            };
            response.remaining = 0;
            Ok(response)
        })))
    }
}
