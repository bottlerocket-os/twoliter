mod error;
mod platform;

use crate::error::Result;

#[snafu::report]
#[tokio::main]
async fn main() -> Result<()> {
    unimplemented!()
}
