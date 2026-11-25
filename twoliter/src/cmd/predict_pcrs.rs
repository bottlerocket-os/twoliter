use anyhow::{ensure, Context, Result};
use aws_config::BehaviorVersion;
use aws_sdk_ec2::types::ImageState;
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

    /// AWS region
    #[arg(long, env = "AWS_REGION")]
    region: String,
}

impl PredictPcrs {
    pub async fn run(&self) -> Result<()> {
        let config = aws_config::defaults(BehaviorVersion::latest())
            .region(aws_config::Region::new(self.region.clone()))
            .load()
            .await;

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

        let owner_id = img.owner_id().with_context(|| {
            format!(
                "AMI {} has no owner ID; cannot verify that it is in your account",
                self.ami_id
            )
        })?;

        ensure!(
            owner_id == account,
            "AMI {} is owned by account {owner_id}, not {account}; pcrsys can only read AMIs in your account.",
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
            .args(["ami", "--ami-id", &self.ami_id, "--region", &self.region])
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
