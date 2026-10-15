//! Reproducibility checks for Bottlerocket AMIs. Subcommands print JSON for twoliter.

mod ami;
mod download;
mod error;

use crate::error::Result;
use clap::{Args as ClapArgs, Parser, Subcommand};
use pcrsys::aws::build_client_config;
use serde::Serialize;
use snafu::{OptionExt, ResultExt};
use std::path::PathBuf;
use std::process::ExitCode;
use tokio::signal::unix::{signal, SignalKind};

#[derive(Parser)]
#[command(
    version,
    about = "Check that a Bottlerocket AMI can be reproduced from source"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(ClapArgs)]
struct AwsArgs {
    /// AMI ID (e.g., ami-0123456789abcdef0)
    #[arg(long)]
    ami_id: String,

    /// AWS region to use
    #[arg(long)]
    region: Option<String>,

    /// AWS profile to use
    #[arg(long)]
    profile: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Print the variant, architecture, version and build ID that produced an AMI
    Resolve {
        #[command(flatten)]
        aws: AwsArgs,
    },

    /// Download an AMI's root and data snapshots as raw images
    Download {
        #[command(flatten)]
        aws: AwsArgs,

        /// Directory to write root.img and data.img into
        #[arg(long)]
        out_dir: PathBuf,

        /// Keep snapshot copies made for snapshots owned by another account
        #[arg(long)]
        keep_copies: bool,
    },
}

#[derive(Serialize)]
struct DownloadOutput {
    ami: ami::AmiInfo,
    #[serde(flatten)]
    images: download::Downloaded,
}

struct Clients {
    ec2: aws_sdk_ec2::Client,
    ebs: aws_sdk_ebs::Client,
    region: String,
    account: String,
}

impl Clients {
    async fn new(aws: &AwsArgs) -> Result<Self> {
        let config = build_client_config(aws.region.as_ref(), aws.profile.as_ref()).await;
        let region = config
            .region()
            .map(|r| r.to_string())
            .whatever_context("no AWS region configured; pass --region")?;
        let identity = aws_sdk_sts::Client::new(&config)
            .get_caller_identity()
            .send()
            .await
            .whatever_context("failed to get caller identity; check AWS credentials")?;
        let account = identity
            .account()
            .whatever_context("caller identity has no account")?
            .to_string();
        Ok(Self {
            ec2: aws_sdk_ec2::Client::new(&config),
            ebs: aws_sdk_ebs::Client::new(&config),
            region,
            account,
        })
    }

    async fn resolve(&self, ami_id: &str) -> Result<ami::AmiInfo> {
        ami::resolve(&self.ec2, &self.account, ami_id).await
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<String> {
    let json =
        serde_json::to_string_pretty(value).whatever_context("failed to serialize output")?;
    println!("{json}");
    Ok(json)
}

async fn run(args: Args) -> Result<ExitCode> {
    match args.command {
        Command::Resolve { aws } => {
            let clients = Clients::new(&aws).await?;
            print_json(&clients.resolve(&aws.ami_id).await?)?;
        }
        Command::Download {
            aws,
            out_dir,
            keep_copies,
        } => {
            let clients = Clients::new(&aws).await?;
            let info = clients.resolve(&aws.ami_id).await?;
            let downloader =
                download::Downloader::new(clients.ec2, clients.ebs, clients.region, keep_copies);
            // Delete snapshot copies when interrupted.
            let mut sigint =
                signal(SignalKind::interrupt()).whatever_context("failed to handle SIGINT")?;
            let mut sigterm =
                signal(SignalKind::terminate()).whatever_context("failed to handle SIGTERM")?;
            let images = tokio::select! {
                images = downloader.download(&info, &out_dir) => images?,
                _ = sigint.recv() => {
                    downloader.cleanup().await;
                    snafu::whatever!("interrupted");
                }
                _ = sigterm.recv() => {
                    downloader.cleanup().await;
                    snafu::whatever!("terminated");
                }
            };
            print_json(&DownloadOutput { ami: info, images })?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {}", snafu::Report::from_error(e));
            ExitCode::FAILURE
        }
    }
}
