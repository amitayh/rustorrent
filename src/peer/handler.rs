#![allow(dead_code)]

use crate::message::Block;
use crate::message::Message;

struct EventLoop;

impl EventLoop {
    fn keep_alive(&self) {}

    fn run_chokig_algorithm(&self) {}

    fn request(&self, _block: Block) {}

    fn handle(&self, _message: Message) {}

    fn connect(&self) {}

    fn disconnect(&self) {}
}
