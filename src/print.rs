//! Single-image grayscale Print Management protocol, recording jobs without rendering.
use async_trait::async_trait;
use radicom::{
    model::{dataset::Dataset, tag::Tag},
    print::{FilmBox, FilmSession, ImageBox},
};
use radicom_network::{
    nservice::{NServiceRequest, NServiceResponse},
    print_sop::uids,
    services::ServiceProvider,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

#[derive(Default)]
struct Objects {
    sessions: HashSet<String>,
    films: HashMap<String, Film>,
    jobs: Vec<String>,
}

struct Film {
    session: String,
    image_uid: String,
    image: Option<Dataset>,
}

/// shortcut: volatile objects are shared across associations because hooks have no association ID; use only trusted peers until an owning service is added.
#[derive(Default)]
pub struct PrintProvider {
    objects: Mutex<Objects>,
}

impl PrintProvider {
    pub fn recorded_jobs(&self) -> Vec<String> {
        self.objects.lock().unwrap().jobs.clone()
    }
}

pub fn image_reference(dataset: &Dataset) -> crate::Result<String> {
    let sequence = dataset.get_sequence(Tag::REFERENCED_IMAGE_BOX_SEQUENCE)?;
    if sequence.len() != 1 {
        return Err("expected one referenced grayscale Image Box".into());
    }
    let reference = &sequence.items()[0];
    if reference.get_str(Tag::REFERENCED_SOP_CLASS_UID)? != uids::BASIC_GRAYSCALE_IMAGE_BOX {
        return Err("unexpected Image Box class".into());
    }
    let uid = reference.get_str(Tag::REFERENCED_SOP_INSTANCE_UID)?;
    crate::files::validate_uid(&uid)?;
    Ok(uid)
}

/// The Basic Grayscale Image Sequence contains only image/pixel attributes.
pub fn image_attributes() -> crate::Result<Dataset> {
    let source = crate::files::sample_file()?.dataset;
    let mut pixels = Dataset::new();
    source.copy_tags(
        &mut pixels,
        &[
            Tag::ROWS,
            Tag::COLUMNS,
            Tag::SAMPLES_PER_PIXEL,
            Tag::PHOTOMETRIC_INTERPRETATION,
            Tag::BITS_ALLOCATED,
            Tag::BITS_STORED,
            Tag::HIGH_BIT,
            Tag::PIXEL_REPRESENTATION,
            Tag::PIXEL_DATA,
        ],
    );
    let mut image = ImageBox::new(1, false);
    image.push_image(pixels);
    let mut attributes = image.dataset().clone();
    attributes.remove(Tag::SOP_CLASS_UID);
    attributes.remove(Tag::SOP_INSTANCE_UID);
    Ok(attributes)
}

fn valid_image(attributes: &Dataset) -> bool {
    if attributes.try_get::<u16>(Tag::IMAGE_BOX_POSITION) != Some(1) {
        return false;
    }
    let Some(sequence) = attributes
        .try_get_sequence(Tag::BASIC_GRAYSCALE_IMAGE_SEQUENCE)
        .filter(|s| s.len() == 1)
    else {
        return false;
    };
    if attributes.contains(&Tag::BASIC_COLOR_IMAGE_SEQUENCE) {
        return false;
    }
    let image = &sequence.items()[0];
    let (Some(rows), Some(columns)) = (
        image.try_get::<u16>(Tag::ROWS),
        image.try_get::<u16>(Tag::COLUMNS),
    ) else {
        return false;
    };
    if rows == 0
        || columns == 0
        || image.try_get::<u16>(Tag::BITS_ALLOCATED) != Some(8)
        || image.try_get::<u16>(Tag::BITS_STORED) != Some(8)
        || image.try_get::<u16>(Tag::HIGH_BIT) != Some(7)
        || image.try_get::<u16>(Tag::PIXEL_REPRESENTATION) != Some(0)
        || image.try_get::<u16>(Tag::SAMPLES_PER_PIXEL) != Some(1)
        || !matches!(
            image.try_get_str_ref(Tag::PHOTOMETRIC_INTERPRETATION),
            Some("MONOCHROME1" | "MONOCHROME2")
        )
    {
        return false;
    }
    let Some(buffer) = image
        .get_element(Tag::PIXEL_DATA)
        .ok()
        .and_then(|e| e.value().as_buffer())
    else {
        return false;
    };
    let expected = u64::from(rows) * u64::from(columns);
    buffer.size() == expected || (expected % 2 == 1 && buffer.size() == expected + 1)
}

fn identity(request: &NServiceRequest) -> Option<(&str, &str)> {
    let class = request.sop_class_uid.as_deref()?;
    let uid = request.sop_instance_uid.as_deref()?;
    crate::files::validate_uid(uid).ok()?;
    Some((class, uid))
}

#[async_trait]
impl ServiceProvider for PrintProvider {
    fn supported_sop_classes(&self) -> Vec<String> {
        [
            uids::BASIC_FILM_SESSION,
            uids::BASIC_FILM_BOX,
            uids::BASIC_GRAYSCALE_IMAGE_BOX,
        ]
        .map(String::from)
        .to_vec()
    }

    async fn on_n_create(
        &self,
        request: &NServiceRequest,
    ) -> radicom_network::Result<NServiceResponse> {
        let Some((class, uid)) = identity(request) else {
            return Ok(NServiceResponse::failure(request, 0x0106));
        };
        let Some(dataset) = &request.dataset else {
            return Ok(NServiceResponse::failure(request, 0x0120));
        };
        if dataset
            .try_get_str_ref(Tag::SOP_INSTANCE_UID)
            .is_some_and(|value| value != uid)
            || dataset
                .try_get_str_ref(Tag::SOP_CLASS_UID)
                .is_some_and(|value| value != class)
        {
            return Ok(NServiceResponse::failure(request, 0x0106));
        }
        let mut objects = self.objects.lock().unwrap();
        if objects.sessions.contains(uid)
            || objects.films.contains_key(uid)
            || objects.films.values().any(|f| f.image_uid == uid)
        {
            return Ok(NServiceResponse::failure(request, 0x0111));
        }
        match class {
            uids::BASIC_FILM_SESSION => {
                let session = FilmSession::from_dataset(dataset.clone());
                if session.copies() != Some(1) {
                    return Ok(NServiceResponse::failure(request, 0x0106));
                }
                objects.sessions.insert(uid.to_owned());
                Ok(NServiceResponse::success(request, None))
            }
            uids::BASIC_FILM_BOX => {
                let Some(sequence) = dataset
                    .try_get_sequence(Tag::REFERENCED_FILM_SESSION_SEQUENCE)
                    .filter(|s| s.len() == 1)
                else {
                    return Ok(NServiceResponse::failure(request, 0x0106));
                };
                let reference = &sequence.items()[0];
                let Some(session) = reference.try_get_str_ref(Tag::REFERENCED_SOP_INSTANCE_UID)
                else {
                    return Ok(NServiceResponse::failure(request, 0x0106));
                };
                if reference.try_get_str_ref(Tag::REFERENCED_SOP_CLASS_UID)
                    != Some(uids::BASIC_FILM_SESSION)
                    || !objects.sessions.contains(session)
                    || dataset.try_get_str_ref(Tag::IMAGE_DISPLAY_FORMAT) != Some("STANDARD\\1,1")
                    || dataset.try_get_str_ref(Tag::FILM_SIZE_ID) != Some("A4")
                {
                    return Ok(NServiceResponse::failure(request, 0x0106));
                }
                let mut film = FilmBox::from_dataset(dataset.clone());
                film.initialize_image_boxes(1);
                let image_uid = image_reference(film.dataset())
                    .expect("the data model creates one grayscale Image Box");
                objects.films.insert(
                    uid.to_owned(),
                    Film {
                        session: session.to_owned(),
                        image_uid,
                        image: None,
                    },
                );
                Ok(NServiceResponse::success(
                    request,
                    Some(film.into_dataset()),
                ))
            }
            _ => Ok(NServiceResponse::failure(request, 0x0118)),
        }
    }

    async fn on_n_set(
        &self,
        request: &NServiceRequest,
    ) -> radicom_network::Result<NServiceResponse> {
        let Some((uids::BASIC_GRAYSCALE_IMAGE_BOX, uid)) = identity(request) else {
            return Ok(NServiceResponse::failure(request, 0x0118));
        };
        let mut objects = self.objects.lock().unwrap();
        let Some(film) = objects
            .films
            .values_mut()
            .find(|film| film.image_uid == uid)
        else {
            return Ok(NServiceResponse::failure(request, 0x0112));
        };
        let Some(attributes) = request.dataset.as_ref().filter(|d| valid_image(d)) else {
            return Ok(NServiceResponse::failure(request, 0x0106));
        };
        film.image = Some(attributes.clone());
        Ok(NServiceResponse::success(request, None))
    }

    async fn on_n_action(
        &self,
        request: &NServiceRequest,
    ) -> radicom_network::Result<NServiceResponse> {
        let Some((uids::BASIC_FILM_BOX, uid)) = identity(request) else {
            return Ok(NServiceResponse::failure(request, 0x0118));
        };
        if request.action_type_id != Some(1) {
            return Ok(NServiceResponse::failure(request, 0x0123));
        }
        let mut objects = self.objects.lock().unwrap();
        let Some(film) = objects.films.get(uid) else {
            return Ok(NServiceResponse::failure(request, 0x0112));
        };
        if film.image.is_none() {
            return Ok(NServiceResponse::failure(request, 0xC600));
        }
        objects.jobs.push(uid.to_owned());
        println!(
            "Recorded synthetic print job for Film Box {uid}; no rendering or physical printing"
        );
        Ok(NServiceResponse::success(request, None))
    }

    async fn on_n_delete(
        &self,
        request: &NServiceRequest,
    ) -> radicom_network::Result<NServiceResponse> {
        let Some((class, uid)) = identity(request) else {
            return Ok(NServiceResponse::failure(request, 0x0106));
        };
        let mut objects = self.objects.lock().unwrap();
        let removed = match class {
            uids::BASIC_FILM_BOX => objects.films.remove(uid).is_some(),
            uids::BASIC_FILM_SESSION => {
                if objects.films.values().any(|film| film.session == uid) {
                    return Ok(NServiceResponse::failure(request, 0x0110));
                }
                objects.sessions.remove(uid)
            }
            _ => return Ok(NServiceResponse::failure(request, 0x0118)),
        };
        Ok(if removed {
            NServiceResponse::success(request, None)
        } else {
            NServiceResponse::failure(request, 0x0112)
        })
    }
}
