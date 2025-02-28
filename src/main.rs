use bencoding::parser::Parser;
use tokio::fs::File;

use crate::torrent::Torrent;

mod bencoding;
mod client;
mod crypto;
mod torrent;
mod tracker;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let mut file = File::open(&args[1]).await?;
    let mut parser = Parser::new();
    tokio::io::copy(&mut file, &mut parser).await?;
    let value = parser.result()?;
    let torrent = Torrent::try_from(value)?;
    let result = tracker::request(&torrent).await?;
    dbg!(&result);

    return Ok(());
}
