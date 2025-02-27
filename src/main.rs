use std::io;

use bencoding::parser::Parser2;
use tokio::{fs::File, io::AsyncReadExt};

mod bencoding;
mod client;
mod torrent;

#[tokio::main]
async fn main() -> io::Result<()> {
    //let mut torrent = File::open("/home/amitay/dev/example.torrent").await?;
    let mut torrent = File::open("/home/amitay/dev/ubuntu-24.10-desktop-amd64.iso.torrent").await?;
    let mut parser = Parser2::new();
    tokio::io::copy(&mut torrent, &mut parser).await?;
    println!("@@@ result: {:?}", parser.result());
    let mut buf = Vec::new();
    torrent.read_to_end(&mut buf).await?;
    //let value = bencoding::value::Value::try_from(buf.as_slice()).unwrap();
    //println!("Hello, world! {:?}", value);
    return Ok(());
}
