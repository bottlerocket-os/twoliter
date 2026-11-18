use anyhow::{ensure, Context, Result};
use aws_config::BehaviorVersion;
use aws_sdk_ec2::types::{BootModeValues, ImageState};
use clap::Parser;
use error_utils::AwsSdkError;
use std::io::Write as _;
use std::process::Stdio;
use tempfile::TempDir;
use tokio::process::Command;
use tracing::info;

/// Predict PCR values for a Bottlerocket AMI
#[derive(Debug, Parser)]
pub struct PredictPcrs {
    /// AMI ID to analyze
    #[arg(long)]
    ami_id: String,

    /// AWS region (or set AWS_REGION)
    #[arg(long)]
    region: Option<String>,
}

impl PredictPcrs {
    pub async fn run(&self) -> Result<()> {
        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if let Some(region) = &self.region {
            loader = loader.region(aws_config::Region::new(region.clone()));
        }
        let config = loader.load().await;

        let region = config
            .region()
            .context("No AWS region; pass --region or set AWS_REGION")?
            .as_ref();

        let sts = aws_sdk_sts::Client::new(&config);
        let identity = sts
            .get_caller_identity()
            .send()
            .await
            .map_err(AwsSdkError::from)
            .context("Unable to verify AWS credentials (sts:GetCallerIdentity)")?;

        let account = identity
            .account()
            .context("GetCallerIdentity returned no account")?;

        let ec2 = aws_sdk_ec2::Client::new(&config);
        let resp = ec2
            .describe_images()
            .image_ids(&self.ami_id)
            .send()
            .await
            .map_err(AwsSdkError::from)
            .with_context(|| format!("Unable to describe AMI {}", self.ami_id))?;

        let img = resp
            .images()
            .first()
            .with_context(|| format!("AMI {} not found", self.ami_id))?;

        ensure!(
            img.state() == Some(&ImageState::Available),
            "AMI {} is not available",
            self.ami_id
        );

        ensure!(
            matches!(
                img.boot_mode(),
                Some(BootModeValues::Uefi | BootModeValues::UefiPreferred)
            ),
            "AMI {} does not boot with UEFI",
            self.ami_id
        );

        let owner_id = img.owner_id().with_context(|| {
            format!(
                "AMI {} has no owner ID; cannot verify that it is in your account",
                self.ami_id
            )
        })?;

        ensure!(
            owner_id == account,
            "AMI {} is owned by account {owner_id}, not {account}; pcrsys can only read AMIs in your account. \
            Copy it first, then run this command on the copy:\n  \
            aws ec2 copy-image --source-image-id {} --source-region {region} --region {region} --name <name-of-your-choice>",
            self.ami_id,
            self.ami_id
        );

        info!(
            "AMI {} is owned by this account, reading it directly",
            self.ami_id
        );

        let tools = TempDir::new().context("Unable to create a directory for tools")?;
        crate::tools::install_tools(tools.path()).await?;
        let pcrsys = tools.path().join("pcrsys");

        info!("Running pcrsys on AMI {}", self.ami_id);

        let output = Command::new(&pcrsys)
            .args(["ami", "--ami-id", &self.ami_id, "--region", region])
            .env("TMPDIR", tools.path())
            .stderr(Stdio::inherit())
            .output()
            .await
            .context("Unable to run pcrsys")?;

        ensure!(output.status.success(), "pcrsys failed ({})", output.status);

        std::io::stdout().write_all(&output.stdout)?;

        Ok(())
    }
}
