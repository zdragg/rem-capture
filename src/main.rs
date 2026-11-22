mod picker;
mod stream;

use std::{thread, time::Duration};

use crate::stream::Recorder;

fn main() {
    let content_filter = picker::pick().unwrap();
    let recorder = Recorder::new(content_filter);
    recorder.start();
    thread::sleep(Duration::from_secs(5));
    recorder.stop();
    let frames: Vec<_> = recorder.frames().collect();
    println!("{}", frames.len())
}
