use clap::{Args, Parser, Subcommand, ValueEnum};
use radicom::model::{dataset::Dataset, tag::Tag, vr::Vr};
use radicom_network::{
    cfind::{CFindRequest, QueryRetrieveLevel, QueryRoot},
    cget::CGetRequest,
    cmove::CMoveRequest,
    dimse::status,
};
use radicom_samples::{
    network::{client, receive_handler},
    Result,
};
use std::path::PathBuf;

/// Query/retrieve a small Study Root archive. Text matching supports * and ?.
#[derive(Parser)]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:11113")]
    address: String,
    #[arg(long, default_value = "QR_SCU")]
    calling_ae: String,
    #[arg(long, default_value = "QR_SCP")]
    called_ae: String,
    #[command(subcommand)]
    command: Operation,
}
#[derive(Subcommand)]
enum Operation {
    Find(Query),
    Get {
        #[command(flatten)]
        query: Query,
        #[arg(long, default_value = "retrieved")]
        output: PathBuf,
        /// Storage SOP classes to negotiate for reverse STORE (repeatable).
        #[arg(long,default_values=[radicom::uids::SECONDARY_CAPTURE_IMAGE_STORAGE,radicom::uids::CT_IMAGE_STORAGE,radicom::uids::MR_IMAGE_STORAGE])]
        sop_class: Vec<String>,
    },
    Move {
        #[command(flatten)]
        query: Query,
        /// AE configured with --destination on qr_scp.
        #[arg(long, default_value = "SAMPLES_SCP")]
        destination: String,
    },
}
#[derive(Clone, Copy, ValueEnum)]
enum Level {
    Study,
    Series,
    Image,
}
#[derive(Args)]
struct Query {
    #[arg(long, value_enum, default_value = "study")]
    level: Level,
    /// Study Instance UID; GET/MOVE default to the built-in 2.25.100.
    #[arg(long)]
    study: Option<String>,
    /// Series Instance UID; series/image GET/MOVE default to 2.25.101.
    #[arg(long)]
    series: Option<String>,
    /// SOP Instance UID; image-level GET/MOVE default to 2.25.102.
    #[arg(long)]
    instance: Option<String>,
    #[arg(long)]
    patient_name: Option<String>,
    #[arg(long)]
    patient_id: Option<String>,
    /// YYYYMMDD or START-END; either range endpoint may be empty.
    #[arg(long)]
    study_date: Option<String>,
    #[arg(long)]
    accession: Option<String>,
    #[arg(long)]
    modality: Option<String>,
}
impl Query {
    fn level(&self) -> QueryRetrieveLevel {
        match self.level {
            Level::Study => QueryRetrieveLevel::Study,
            Level::Series => QueryRetrieveLevel::Series,
            Level::Image => QueryRetrieveLevel::Image,
        }
    }
    /// Demo defaults keep GET/MOVE runnable without replaying values from FIND.
    fn with_demo_uids(mut self) -> Self {
        self.study.get_or_insert_with(|| "2.25.100".to_owned());
        if matches!(self.level, Level::Series | Level::Image) {
            self.series.get_or_insert_with(|| "2.25.101".to_owned());
        }
        if matches!(self.level, Level::Image) {
            self.instance.get_or_insert_with(|| "2.25.102".to_owned());
        }
        self
    }
    fn keys(&self, identifier: &mut Dataset) {
        for (tag, vr, value) in [
            (Tag::STUDY_INSTANCE_UID, Vr::Ui, &self.study),
            (Tag::SERIES_INSTANCE_UID, Vr::Ui, &self.series),
            (Tag::SOP_INSTANCE_UID, Vr::Ui, &self.instance),
            (Tag::PATIENT_NAME, Vr::Pn, &self.patient_name),
            (Tag::PATIENT_ID, Vr::Lo, &self.patient_id),
            (Tag::STUDY_DATE, Vr::Da, &self.study_date),
            (Tag::ACCESSION_NUMBER, Vr::Sh, &self.accession),
            (Tag::MODALITY, Vr::Cs, &self.modality),
        ] {
            if let Some(value) = value {
                identifier.insert_str(tag, vr, value);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    radicom_samples::init_logging();
    radicom_codecs::install();
    let args = Cli::parse();
    let mut client = client(&args.address, &args.calling_ae, &args.called_ae);
    let final_status = match args.command {
        Operation::Find(query) => {
            let mut request = CFindRequest::new(query.level(), QueryRoot::Study, 0);
            query.keys(&mut request.identifier);
            let responses = client.send_c_find(&request).await?;
            for response in &responses {
                println!(
                    "C-FIND {:04X} {:?}",
                    response.status, response.error_comment
                );
                if let Some(identifier) = &response.identifier {
                    for item in identifier.items() {
                        println!("  {item:?}");
                    }
                }
            }
            responses
                .last()
                .ok_or("missing C-FIND final response")?
                .status
        }
        Operation::Get {
            query,
            output,
            sop_class,
        } => {
            let query = query.with_demo_uids();
            client = client.with_reverse_store_handler(receive_handler(output));
            let mut request = CGetRequest::new(query.level(), QueryRoot::Study, 0)?;
            query.keys(&mut request.identifier);
            let responses = client.send_c_get(&request, &sop_class).await?;
            for response in &responses {
                println!(
                    "C-GET {:04X}: {:?}, remaining {}, failed UIDs {:?} {:?}",
                    response.status,
                    response.summary,
                    response.remaining,
                    response.failed_sop_instance_uids,
                    response.error_comment
                );
            }
            responses
                .last()
                .ok_or("missing C-GET final response")?
                .status
        }
        Operation::Move { query, destination } => {
            let query = query.with_demo_uids();
            let mut request = CMoveRequest::new(query.level(), QueryRoot::Study, destination, 0)?;
            query.keys(&mut request.identifier);
            let responses = client.send_c_move(&request).await?;
            for response in &responses {
                println!(
                    "C-MOVE {:04X}: {:?}, failed UIDs {:?} {:?}",
                    response.status,
                    response.summary,
                    response.failed_sop_instance_uids,
                    response.error_comment
                );
            }
            responses
                .last()
                .ok_or("missing C-MOVE final response")?
                .status
        }
    };
    client.release().await;
    if final_status != status::SUCCESS {
        return Err(format!("query/retrieve failed: {final_status:04X}").into());
    }
    Ok(())
}
