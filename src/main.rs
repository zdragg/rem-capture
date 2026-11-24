mod picker;
mod recorder;
mod writer;

use std::{thread, time::Duration};

use crate::recorder::Recorder;

fn main() {
    let content_filter = picker::pick().unwrap();
    let recorder = Recorder::new(content_filter);
    recorder.start();
    thread::sleep(Duration::from_secs(1));
    recorder.stop();
}
