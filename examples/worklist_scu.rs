use clap::{Parser, Subcommand, ValueEnum};
use radicom::model::{tag::Tag, uid::Uid};
use radicom_samples::{
    network::client,
    worklist::{check_n, mpps_create, mpps_set, query, SPS, SPS_DATE, SPS_ID},
    Result,
};

#[derive(Parser)]
#[command(about = "Query the synthetic MWL or perform an MPPS start/final transition")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:11115")]
    address: String,
    #[arg(long, default_value = "SAMPLE_SCU")]
    calling_ae: String,
    #[arg(long, default_value = "WORKLIST")]
    called_ae: String,
    #[command(subcommand)]
    command: Operation,
}

#[derive(Subcommand)]
enum Operation {
    Find {
        #[arg(long, default_value = "*")]
        patient: String,
        #[arg(long, default_value = "CT")]
        modality: String,
        /// YYYYMMDD or inclusive start-end; either bound may be omitted.
        #[arg(long, default_value = "")]
        date: String,
    },
    Mpps {
        /// Omit to generate a new MPPS SOP Instance UID.
        #[arg(long)]
        instance_uid: Option<String>,
        #[arg(long, value_enum, default_value = "completed")]
        final_status: FinalStatus,
    },
}

#[derive(Clone, ValueEnum)]
enum FinalStatus {
    Completed,
    Discontinued,
}

#[tokio::main]
async fn main() -> Result {
    let args = Args::parse();
    radicom_samples::init_logging();
    let mut client = client(&args.address, &args.calling_ae, &args.called_ae);
    match args.command {
        Operation::Find {
            patient,
            modality,
            date,
        } => {
            let responses = client
                .send_c_find(&query(&patient, &modality, &date))
                .await?;
            if responses.last().is_none_or(|r| r.status != 0) {
                return Err("MWL query failed".into());
            }
            for response in responses {
                if let Some(dataset) = response.identifier {
                    let step = &dataset.get_sequence(SPS)?.items()[0];
                    println!(
                        "{} {}: {} {} {}",
                        dataset.get_str(Tag::PATIENT_ID)?,
                        dataset.get_str(Tag::PATIENT_NAME)?,
                        step.get_str(Tag::MODALITY)?,
                        step.get_str(SPS_DATE)?,
                        step.get_str(SPS_ID)?
                    );
                }
            }
        }
        Operation::Mpps {
            instance_uid,
            final_status,
        } => {
            let uid = instance_uid.unwrap_or_else(Uid::generate_value);
            radicom_samples::files::validate_uid(&uid)?;
            check_n(&client.send_n_service(&mpps_create(&uid)).await?)?;
            let state = match final_status {
                FinalStatus::Completed => "COMPLETED",
                FinalStatus::Discontinued => "DISCONTINUED",
            };
            check_n(&client.send_n_service(&mpps_set(&uid, state)).await?)?;
            println!("MPPS {uid}: IN PROGRESS -> {state}");
        }
    }
    client.release().await;
    Ok(())
}
