use std::io;

use bencoding::parser::Parser;
use tokio::fs::File;

use crate::torrent::Torrent;

mod bencoding;
mod client;
mod torrent;

#[tokio::main]
async fn main() -> io::Result<()> {
    //let mut torrent = File::open("/home/amitay/dev/example.torrent").await?;
    let mut file = File::open("/home/amitay/dev/ubuntu-24.10-desktop-amd64.iso.torrent").await?;
    let mut parser = Parser::new();
    tokio::io::copy(&mut file, &mut parser).await?;
    let value = parser.result()?;
    let torrent = Torrent::try_from(value).unwrap();
    println!("@@@ result: {:?}", torrent.info.info_hash);

    return Ok(());
}
