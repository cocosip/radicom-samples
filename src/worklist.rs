//! Synthetic MWL and MPPS, with a deliberately small declared query contract.
use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use radicom::model::{dataset::Dataset, dicom_datetime::parse_da, tag::Tag, vr::Vr};
use radicom::rules::dicom_wildcard_match;
use radicom_network::{
    cfind::{CFindRequest, CFindResponse},
    dimse::CommandField,
    nservice::{NServiceRequest, NServiceResponse},
    services::ServiceProvider,
};
use std::{collections::HashMap, sync::Mutex};

pub const MWL: &str = radicom::uids::MODALITY_WORKLIST_INFORMATION_MODEL_FIND;
pub const MPPS: &str = radicom::uids::MODALITY_PERFORMED_PROCEDURE_STEP;
pub const SPS: Tag = Tag::SCHEDULED_PROCEDURE_STEP_SEQUENCE;
pub const SPS_DATE: Tag = Tag::SCHEDULED_PROCEDURE_STEP_START_DATE;
pub const SPS_ID: Tag = Tag::SCHEDULED_PROCEDURE_STEP_ID;
pub const MPPS_STATUS: Tag = Tag::PERFORMED_PROCEDURE_STEP_STATUS;
const SCHEDULED_ATTRIBUTES: Tag = Tag::SCHEDULED_STEP_ATTRIBUTES_SEQUENCE;

/// shortcut: state is volatile and shared across associations because provider hooks carry no association ID; add an owning service for production.
#[derive(Default)]
pub struct WorklistProvider {
    performed: Mutex<HashMap<String, String>>,
}

pub fn synthetic_workitem() -> Dataset {
    let mut dataset = Dataset::new();
    dataset.insert_str(Tag::PATIENT_ID, Vr::Lo, "SAMPLE001");
    dataset.insert_str(Tag::PATIENT_NAME, Vr::Pn, "SYNTHETIC^WORKLIST");
    dataset.insert_str(Tag::PATIENT_BIRTH_DATE, Vr::Da, "20000101");
    dataset.insert_str(Tag::PATIENT_SEX, Vr::Cs, "O");
    dataset.insert_str(Tag::STUDY_INSTANCE_UID, Vr::Ui, "2.25.100");
    dataset.insert_str(Tag::ACCESSION_NUMBER, Vr::Sh, "SAMPLE_ACC_1");
    dataset.insert_str(Tag::REQUESTED_PROCEDURE_ID, Vr::Sh, "SAMPLE_RP_1");
    let mut step = Dataset::new();
    step.insert_str(Tag::MODALITY, Vr::Cs, "CT");
    step.insert_str(Tag::SCHEDULED_STATION_AE_TITLE, Vr::Ae, "SAMPLE_CT");
    step.insert_str(SPS_DATE, Vr::Da, "20261109");
    step.insert_str(Tag::SCHEDULED_PROCEDURE_STEP_START_TIME, Vr::Tm, "090000");
    step.insert_str(SPS_ID, Vr::Sh, "SAMPLE_SPS_1");
    step.insert_str(
        Tag::SCHEDULED_PROCEDURE_STEP_DESCRIPTION,
        Vr::Lo,
        "Synthetic CT procedure",
    );
    dataset.insert_sequence(SPS).push(step);
    dataset
}

/// Return only explicitly requested keys, including keys inside SPS.
pub fn query(patient: &str, modality: &str, date: &str) -> CFindRequest {
    let mut request = CFindRequest::worklist_query(0);
    // MWL has no Query/Retrieve Level (unlike hierarchical QR).
    request.identifier.clear();
    request
        .identifier
        .insert_str(Tag::PATIENT_ID, Vr::Lo, patient);
    request.identifier.insert_empty(Tag::PATIENT_NAME, Vr::Pn);
    request
        .identifier
        .insert_empty(Tag::STUDY_INSTANCE_UID, Vr::Ui);
    let mut step = Dataset::new();
    step.insert_str(Tag::MODALITY, Vr::Cs, modality);
    step.insert_str(SPS_DATE, Vr::Da, date);
    step.insert_empty(SPS_ID, Vr::Sh);
    request.identifier.insert_sequence(SPS).push(step);
    request
}

fn validate_keys(query: &Dataset, nested: bool) -> crate::Result<()> {
    let allowed = if nested {
        vec![
            Tag::MODALITY,
            Tag::SCHEDULED_STATION_AE_TITLE,
            SPS_DATE,
            Tag::SCHEDULED_PROCEDURE_STEP_START_TIME,
            SPS_ID,
            Tag::SCHEDULED_PROCEDURE_STEP_DESCRIPTION,
        ]
    } else {
        vec![
            Tag::PATIENT_ID,
            Tag::PATIENT_NAME,
            Tag::PATIENT_BIRTH_DATE,
            Tag::PATIENT_SEX,
            Tag::STUDY_INSTANCE_UID,
            Tag::ACCESSION_NUMBER,
            Tag::REQUESTED_PROCEDURE_ID,
        ]
    };
    for item in query.items() {
        let tag = item.tag();
        if *tag == SPS && !nested {
            let sequence = query.get_sequence(SPS)?;
            if sequence.len() != 1 {
                return Err("SPS query requires exactly one item".into());
            }
            validate_keys(&sequence.items()[0], true)?;
        } else {
            if !allowed.contains(tag) {
                return Err(format!("unsupported worklist key {tag}").into());
            }
            let value = query.get_str(tag)?;
            if (*tag == SPS_DATE || *tag == Tag::PATIENT_BIRTH_DATE) && !value.is_empty() {
                validate_date_query(&value)?;
            }
            if *tag == Tag::STUDY_INSTANCE_UID && !value.is_empty() {
                crate::files::validate_uid(&value)?;
            }
        }
    }
    Ok(())
}

fn validate_date_query(query: &str) -> crate::Result<()> {
    let (start, end) = query.split_once('-').unwrap_or((query, query));
    if start.is_empty() && end.is_empty() {
        return Err("date range needs a bound".into());
    }
    for bound in [start, end].into_iter().filter(|s| !s.is_empty()) {
        if bound.len() != 8 || !bound.bytes().all(|b| b.is_ascii_digit()) {
            return Err("date must use YYYYMMDD".into());
        }
        parse_da(bound)?;
    }
    if !start.is_empty() && !end.is_empty() && start > end {
        return Err("date range ends before it starts".into());
    }
    Ok(())
}

fn project_match(query: &Dataset, source: &Dataset) -> Option<Dataset> {
    let mut result = Dataset::new();
    for item in query.items() {
        let tag = item.tag();
        if *tag == SPS {
            let keys = &query.try_get_sequence(SPS)?.items()[0];
            let step = &source.try_get_sequence(SPS)?.items()[0];
            result.insert_sequence(SPS).push(project_match(keys, step)?);
        } else {
            let pattern = query.try_get_str(tag)?;
            let value = source.try_get_str(tag).unwrap_or_default();
            let matches = if pattern.is_empty() {
                true
            } else if *tag == SPS_DATE || *tag == Tag::PATIENT_BIRTH_DATE {
                let (start, end) = pattern.split_once('-').unwrap_or((&pattern, &pattern));
                (start.is_empty() || value.as_str() >= start)
                    && (end.is_empty() || value.as_str() <= end)
            } else if *tag == Tag::STUDY_INSTANCE_UID {
                value == pattern
            } else {
                dicom_wildcard_match(&value, &pattern)
            };
            if !matches {
                return None;
            }
            if let Some(value) = source.item(tag) {
                result.insert_or_update(value.clone());
            }
        }
    }
    Some(result)
}

/// Explicit UIDs keep the example's object references visible on the wire.
pub fn n_request(
    field: CommandField,
    class: &str,
    instance: &str,
    dataset: Option<Dataset>,
) -> NServiceRequest {
    let mut request = NServiceRequest::new(field, 0);
    request.sop_class_uid = Some(class.to_owned());
    request.sop_instance_uid = Some(instance.to_owned());
    request.dataset = dataset;
    request
}

pub fn mpps_create(uid: &str) -> NServiceRequest {
    let mut dataset = Dataset::new();
    dataset.insert_str(MPPS_STATUS, Vr::Cs, "IN PROGRESS");
    dataset.insert_str(Tag::PERFORMED_PROCEDURE_STEP_ID, Vr::Sh, "SAMPLE_MPPS_1");
    dataset.insert_str(Tag::PERFORMED_STATION_AE_TITLE, Vr::Ae, "SAMPLE_CT");
    dataset.insert_str(Tag::PERFORMED_PROCEDURE_STEP_START_DATE, Vr::Da, "20261109");
    dataset.insert_str(Tag::PERFORMED_PROCEDURE_STEP_START_TIME, Vr::Tm, "090000");
    let mut reference = Dataset::new();
    reference.insert_str(Tag::STUDY_INSTANCE_UID, Vr::Ui, "2.25.100");
    reference.insert_str(SPS_ID, Vr::Sh, "SAMPLE_SPS_1");
    dataset
        .insert_sequence(SCHEDULED_ATTRIBUTES)
        .push(reference);
    n_request(CommandField::NCreateRq, MPPS, uid, Some(dataset))
}

pub fn mpps_set(uid: &str, state: &str) -> NServiceRequest {
    let mut dataset = Dataset::new();
    dataset.insert_str(MPPS_STATUS, Vr::Cs, state);
    dataset.insert_str(Tag::PERFORMED_PROCEDURE_STEP_END_DATE, Vr::Da, "20261109");
    dataset.insert_str(Tag::PERFORMED_PROCEDURE_STEP_END_TIME, Vr::Tm, "091000");
    n_request(CommandField::NSetRq, MPPS, uid, Some(dataset))
}

pub fn check_n(response: &NServiceResponse) -> crate::Result<()> {
    if response.status != 0 {
        return Err(format!("DIMSE-N failed with status {:04X}", response.status).into());
    }
    Ok(())
}

#[async_trait]
impl ServiceProvider for WorklistProvider {
    fn supported_sop_classes(&self) -> Vec<String> {
        vec![MWL.into(), MPPS.into()]
    }

    fn on_c_find<'a>(
        &'a self,
        request: &'a CFindRequest,
    ) -> BoxStream<'a, radicom_network::Result<CFindResponse>> {
        let mut responses = Vec::new();
        if request.sop_class_uid != MWL || validate_keys(&request.identifier, false).is_err() {
            responses.push(Ok(CFindResponse::failure(
                request,
                0xA900,
                "Unsupported/malformed MWL key",
            )));
        } else {
            let result = project_match(&request.identifier, &synthetic_workitem());
            let count = u16::from(result.is_some());
            if let Some(dataset) = result {
                responses.push(Ok(CFindResponse::pending(request, dataset)));
            }
            responses.push(Ok(CFindResponse::success(request, count)));
        }
        Box::pin(stream::iter(responses))
    }

    async fn on_n_create(
        &self,
        request: &NServiceRequest,
    ) -> radicom_network::Result<NServiceResponse> {
        if request.sop_class_uid.as_deref() != Some(MPPS) {
            return Ok(NServiceResponse::failure(request, 0x0118));
        }
        let Some(uid) = request
            .sop_instance_uid
            .as_deref()
            .filter(|uid| crate::files::validate_uid(uid).is_ok())
        else {
            return Ok(NServiceResponse::failure(request, 0x0106));
        };
        let Some(dataset) = &request.dataset else {
            return Ok(NServiceResponse::failure(request, 0x0120));
        };
        let reference = dataset
            .try_get_sequence(SCHEDULED_ATTRIBUTES)
            .filter(|s| s.len() == 1)
            .and_then(|s| s.items().first());
        if dataset.try_get_str_ref(MPPS_STATUS) != Some("IN PROGRESS")
            || reference.and_then(|d| d.try_get_str_ref(Tag::STUDY_INSTANCE_UID))
                != Some("2.25.100")
            || reference.and_then(|d| d.try_get_str_ref(SPS_ID)) != Some("SAMPLE_SPS_1")
        {
            return Ok(NServiceResponse::failure(request, 0x0106));
        }
        let mut instances = self.performed.lock().unwrap();
        if instances.contains_key(uid) {
            return Ok(NServiceResponse::failure(request, 0x0111));
        }
        instances.insert(uid.to_owned(), "IN PROGRESS".into());
        Ok(NServiceResponse::success(request, None))
    }

    async fn on_n_set(
        &self,
        request: &NServiceRequest,
    ) -> radicom_network::Result<NServiceResponse> {
        if request.sop_class_uid.as_deref() != Some(MPPS) {
            return Ok(NServiceResponse::failure(request, 0x0118));
        }
        let mut instances = self.performed.lock().unwrap();
        let Some(state) = request
            .sop_instance_uid
            .as_ref()
            .and_then(|uid| instances.get_mut(uid))
        else {
            return Ok(NServiceResponse::failure(request, 0x0112));
        };
        let Some(dataset) = &request.dataset else {
            return Ok(NServiceResponse::failure(request, 0x0120));
        };
        // A final transition cannot replace the scheduled object reference.
        if dataset.items().any(|item| {
            ![
                MPPS_STATUS,
                Tag::PERFORMED_PROCEDURE_STEP_END_DATE,
                Tag::PERFORMED_PROCEDURE_STEP_END_TIME,
            ]
            .contains(item.tag())
        }) {
            return Ok(NServiceResponse::failure(request, 0x0105));
        }
        let next = request
            .dataset
            .as_ref()
            .and_then(|d| d.try_get_str_ref(MPPS_STATUS));
        if state != "IN PROGRESS" || !matches!(next, Some("COMPLETED" | "DISCONTINUED")) {
            return Ok(NServiceResponse::failure(request, 0x0106));
        }
        *state = next.unwrap().to_owned();
        Ok(NServiceResponse::success(request, None))
    }
}
