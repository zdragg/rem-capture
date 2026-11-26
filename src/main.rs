mod picker;
mod recorder;

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    thread,
    time::Duration,
};

use crate::recorder::{Recorder, writer::SegmentGet};

fn main() {
    let content_filter = picker::pick().unwrap();
    let recorder = Recorder::start(content_filter);

    thread::sleep(Duration::from_secs(3));
    write_to("output1.mp4", recorder.get());

    thread::sleep(Duration::from_secs(3));
    write_to("output2.mp4", recorder.get());

    thread::sleep(Duration::from_secs(3));
    write_to("output3.mp4", recorder.get());
}

fn write_to(path: impl AsRef<Path>, segments: SegmentGet) {
    let file = File::create(path.as_ref()).unwrap();
    let mut buf_writer = BufWriter::new(file);

    for segment in segments.bytes() {
        buf_writer.write_all(segment).unwrap()
    }
}
