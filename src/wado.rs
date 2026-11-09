//! Small WADO-URI subset: GET /wado, three exact UIDs and application/dicom.
//! Explicit VR Little Endian is the default; transferSyntax=* preserves bytes.
//! An explicit registered transfer syntax may transcode pixels (including lossy
//! conversion when explicitly requested). Rendering/anonymization controls fail.
//! Files are snapshotted in memory at startup; restart to see directory changes.

use crate::files::{load_files, validate_uid};
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use radicom::{io::file::DicomFile, model::transfer_syntax, Tag};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path, sync::Arc};

type Instances = Arc<BTreeMap<[String; 3], Bytes>>;
type HttpError = (StatusCode, &'static str);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Parameters {
    request_type: String,
    #[serde(rename = "studyUID")]
    study_uid: String,
    #[serde(rename = "seriesUID")]
    series_uid: String,
    #[serde(rename = "objectUID")]
    object_uid: String,
    content_type: Option<String>,
    transfer_syntax: Option<String>,
}

/// Read a small local directory once; malformed files or duplicate instances fail
/// startup. UIDs are lookup keys only and are never used as request-time paths.
pub fn router(directory: &Path) -> crate::Result<Router> {
    radicom_codecs::install();
    let mut instances = BTreeMap::new();
    for file in load_files(directory)? {
        let bytes = Bytes::from(std::fs::read(
            file.source().ok_or("Missing DICOM source path")?,
        )?);
        // Parse the snapshot so identifiers always describe the exact served bytes.
        let file = DicomFile::from_bytes(&bytes, &Default::default())?;
        let key = [
            file.dataset.get_str(Tag::STUDY_INSTANCE_UID)?,
            file.dataset.get_str(Tag::SERIES_INSTANCE_UID)?,
            file.dataset.get_str(Tag::SOP_INSTANCE_UID)?,
        ];
        for uid in &key {
            validate_uid(uid)?;
        }
        if instances.insert(key, bytes).is_some() {
            return Err("Duplicate Study/Series/SOP Instance UID in WADO directory".into());
        }
    }
    Ok(Router::new()
        .route("/wado", get(retrieve))
        .with_state(Arc::new(instances)))
}

async fn retrieve(
    State(instances): State<Instances>,
    Query(parameters): Query<Parameters>,
    headers: HeaderMap,
) -> Result<Response, HttpError> {
    if parameters.request_type != "WADO" {
        return Err((StatusCode::BAD_REQUEST, "requestType must be WADO"));
    }
    let key = [
        parameters.study_uid,
        parameters.series_uid,
        parameters.object_uid,
    ];
    for uid in &key {
        validate_uid(uid).map_err(|_| (StatusCode::BAD_REQUEST, "Invalid DICOM UID"))?;
    }
    if parameters.content_type.as_deref() != Some("application/dicom") || !accepts_dicom(&headers) {
        return Err((
            StatusCode::NOT_ACCEPTABLE,
            "Request application/dicom explicitly; rendering is unavailable",
        ));
    }
    let syntax_uid = parameters
        .transfer_syntax
        .as_deref()
        .unwrap_or("1.2.840.10008.1.2.1");
    let syntax = if syntax_uid == "*" {
        None
    } else {
        validate_uid(syntax_uid)
            .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid transferSyntax UID"))?;
        Some(
            transfer_syntax::query(syntax_uid)
                .ok_or((StatusCode::NOT_ACCEPTABLE, "Unknown transfer syntax"))?,
        )
    };
    let mut bytes = instances
        .get(&key)
        .cloned()
        .ok_or((StatusCode::NOT_FOUND, "Instance not found"))?;
    if let Some(syntax) = syntax {
        bytes = tokio::task::spawn_blocking(move || -> crate::Result<Bytes> {
            let file = DicomFile::from_bytes(&bytes, &Default::default())?;
            if file.dataset.transfer_syntax() == syntax {
                return Ok(bytes);
            }
            Ok(Bytes::from(
                file.clone_with_syntax(syntax, None)?
                    .to_bytes(&Default::default())?,
            ))
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Conversion task failed"))?
        .map_err(|_| {
            (
                StatusCode::NOT_ACCEPTABLE,
                "Cannot produce requested transfer syntax",
            )
        })?;
    }
    Ok((
        [
            (header::CONTENT_TYPE, "application/dicom"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}

// Only media ranges and q are supported; representation parameters would make
// promises about a representation this example does not negotiate.
fn accepts_dicom(headers: &HeaderMap) -> bool {
    if !headers.contains_key(header::ACCEPT) {
        return true;
    }
    let mut selected: Option<(u8, f32)> = None;
    for value in headers.get_all(header::ACCEPT) {
        let Ok(value) = value.to_str() else {
            return false;
        };
        for range in value.split(',') {
            let mut parts = range.split(';');
            let media = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
            let mut quality = 1.0;
            let mut has_quality = false;
            for parameter in parts {
                let Some((name, value)) = parameter.trim().split_once('=') else {
                    return false;
                };
                if !name.eq_ignore_ascii_case("q") || has_quality {
                    return false;
                }
                let Some(parsed) = quality_value(value) else {
                    return false;
                };
                quality = parsed;
                has_quality = true;
            }
            let rank = match media.as_str() {
                "application/dicom" => 2,
                "application/*" => 1,
                "*/*" => 0,
                _ => continue,
            };
            if selected.is_none_or(|(r, q)| rank > r || (rank == r && quality > q)) {
                selected = Some((rank, quality));
            }
        }
    }
    selected.is_some_and(|(_, quality)| quality > 0.0)
}

fn quality_value(value: &str) -> Option<f32> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if !matches!(whole, "0" | "1")
        || fraction.len() > 3
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (whole == "1" && fraction.bytes().any(|b| b != b'0'))
    {
        return None;
    }
    value.parse().ok()
}
