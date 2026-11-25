mod picker;
mod recorder;
mod writer;

use std::{
    fs::File,
    io::{BufWriter, Write},
    thread,
    time::Duration,
};

use crate::recorder::Recorder;

fn main() {
    let content_filter = picker::pick().unwrap();
    let recorder = Recorder::new(content_filter);

    println!("recording 1 STARTED!");
    let guard = recorder.record();
    thread::sleep(Duration::from_secs(3));
    drop(guard);
    println!("recording 1 STOPPED!");

    thread::sleep(Duration::from_secs(1));

    println!("recording 2 STARTED!");
    let guard = recorder.record();
    thread::sleep(Duration::from_secs(5));

    if let Some(segments) = guard.cut() {
        drop(guard);
        let file = File::create("output.mp4").unwrap();
        let mut buf_writer = BufWriter::new(file);

        for segment in segments.bytes() {
            buf_writer.write_all(segment).unwrap()
        }
    }
}
