use std::io;

use bencoding::parser::Parser;
use tokio::fs::File;

mod bencoding;
mod client;
mod torrent;

#[tokio::main]
async fn main() -> io::Result<()> {
    //let mut torrent = File::open("/home/amitay/dev/example.torrent").await?;
    let mut torrent = File::open("/home/amitay/dev/ubuntu-24.10-desktop-amd64.iso.torrent").await?;
    let mut parser = Parser::new();
    tokio::io::copy(&mut torrent, &mut parser).await?;
    println!("@@@ result: {:?}", parser.result());
    return Ok(());
}
