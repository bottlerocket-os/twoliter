use async_trait::async_trait;
use log::{debug, warn};
use snafu::Snafu;
use std::env;
use tough::key_source::KeySource;
use tough::sign::{parse_keypair, Sign};

#[derive(Debug)]
pub struct EnvKeySource {
    pub var_name: String,
}

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Environment variable '{}' not found", var_name))]
    EnvVarNotFound { var_name: String },

    #[snafu(display(
        "Failed to parse key from environment variable '{}': {}",
        var_name,
        source
    ))]
    KeyParse {
        var_name: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

#[async_trait]
impl KeySource for EnvKeySource {
    async fn as_sign(
        &self,
    ) -> std::result::Result<Box<dyn Sign>, Box<dyn std::error::Error + Send + Sync>> {
        debug!("Reading key from environment variable: {}", self.var_name);

        // Get the key data from the environment variable
        let key_data = env::var(&self.var_name).map_err(|_| Error::EnvVarNotFound {
            var_name: self.var_name.clone(),
        })?;

        // Parse the key data into a signer
        let key = parse_keypair(key_data.as_bytes()).map_err(|e| Error::KeyParse {
            var_name: self.var_name.clone(),
            source: Box::new(e),
        })?;

        Ok(Box::new(key))
    }

    async fn write(
        &self,
        _value: &str,
        _key_id_hex: &str,
    ) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // We don't support writing keys back to environment variables
        // as this wouldn't persist beyond the current process
        warn!("Writing keys back to environment variables is not supported");
        Ok(())
    }
}
