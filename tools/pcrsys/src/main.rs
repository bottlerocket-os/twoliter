mod error;

use crate::error::Result;

#[snafu::report]
#[tokio::main]
async fn main() -> Result<()> {
    unimplemented!()
}
