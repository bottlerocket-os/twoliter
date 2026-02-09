mod diskfs;
mod efi;
mod error;
mod gpt;
mod parsers;
mod pe;
mod platform;

use crate::error::Result;

#[snafu::report]
#[tokio::main]
async fn main() -> Result<()> {
    unimplemented!()
}
