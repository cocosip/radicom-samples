use clap::Parser;
use radicom::{
    model::{tag::Tag, uid::Uid, vr::Vr},
    print::FilmSession,
};
use radicom_network::{dimse::CommandField, print_sop::uids};
use radicom_samples::{
    network::client,
    print::{image_attributes, image_reference},
    worklist::{check_n, n_request},
    Result,
};

#[derive(Parser)]
#[command(
    about = "Send a synthetic grayscale Print job and delete its objects; no physical printing"
)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:11114")]
    address: String,
    #[arg(long, default_value = "SAMPLE_SCU")]
    calling_ae: String,
    #[arg(long, default_value = "PRINT")]
    called_ae: String,
}

#[tokio::main]
async fn main() -> Result {
    let args = Args::parse();
    radicom_samples::init_logging();
    let mut client = client(&args.address, &args.calling_ae, &args.called_ae);
    let mut session = FilmSession::new();
    session.set_copies(1);
    session.set_label("Synthetic sample");
    let session_uid = session.sop_instance_uid().ok_or("missing session UID")?;
    let film_uid = Uid::generate_value();
    let mut film = session.create_film_box();
    film.set_film_size("A4")?;
    let mut film_attributes = film.into_dataset();
    // DICOM defines this field as ST; the 0.2 model setter incorrectly uses CS.
    film_attributes.insert_str(Tag::IMAGE_DISPLAY_FORMAT, Vr::St, "STANDARD\\1,1");
    check_n(
        &client
            .send_n_service(&n_request(
                CommandField::NCreateRq,
                uids::BASIC_FILM_SESSION,
                &session_uid,
                Some(session.into_dataset()),
            ))
            .await?,
    )?;
    let created = client
        .send_n_service(&n_request(
            CommandField::NCreateRq,
            uids::BASIC_FILM_BOX,
            &film_uid,
            Some(film_attributes),
        ))
        .await?;
    check_n(&created)?;
    let image_uid = image_reference(
        created
            .dataset
            .as_ref()
            .ok_or("Film Box response lacks Image Box references")?,
    )?;
    check_n(
        &client
            .send_n_service(&n_request(
                CommandField::NSetRq,
                uids::BASIC_GRAYSCALE_IMAGE_BOX,
                &image_uid,
                Some(image_attributes()?),
            ))
            .await?,
    )?;
    let mut action = n_request(
        CommandField::NActionRq,
        uids::BASIC_FILM_BOX,
        &film_uid,
        None,
    );
    action.action_type_id = Some(1);
    check_n(&client.send_n_service(&action).await?)?;
    println!("SCP recorded Film Box {film_uid}, Image Box {image_uid}; no rendering or physical printing");
    for (class, uid) in [
        (uids::BASIC_FILM_BOX, &film_uid),
        (uids::BASIC_FILM_SESSION, &session_uid),
    ] {
        check_n(
            &client
                .send_n_service(&n_request(CommandField::NDeleteRq, class, uid, None))
                .await?,
        )?;
    }
    client.release().await;
    Ok(())
}
