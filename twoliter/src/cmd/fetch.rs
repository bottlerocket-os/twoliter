use crate::lock::Lock;
use crate::project;
use anyhow::Result;
use clap::Parser;
use log::warn;
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub(crate) struct Fetch {
    /// Path to Twoliter.toml. Will search for Twoliter.toml when absent
    #[clap(long = "project-path")]
    pub(crate) project_path: Option<PathBuf>,

    #[clap(long = "arch", default_value = "x86_64")]
    pub(crate) arch: String,

    #[clap(long = "kit-override", short = 'K', value_parser = parse_key_val::<String, PathBuf>)]
    pub(crate) kit_override: Option<Vec<(String, PathBuf)>>,
}

/// Parse a single key-value pair
fn parse_key_val<T, U>(s: &str) -> Result<(T, U), Box<dyn Error + Send + Sync + 'static>>
where
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
    U: std::str::FromStr,
    U::Err: Error + Send + Sync + 'static,
{
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;
    Ok((s[..pos].parse()?, s[pos + 1..].parse()?))
}

impl Fetch {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let lock_file = Lock::load(&project).await?;
        if self.kit_override.is_some() {
            warn!(
                r#"
!!!
Bottlerocket is being built with an overwritten kit.
This means that the resulting variant images are not based on a remotely
hosted and officially tagged version of kits.         
!!!
"#
            );
        }
        lock_file
            .fetch(
                &project,
                self.arch.as_str(),
                self.kit_override
                    .clone()
                    .map(|x| crate::lock::LockOverrides {
                        kit: HashMap::from_iter(x),
                    }),
            )
            .await?;
        Ok(())
    }
}
