mod recorder;

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    thread,
    time::Duration,
};

use crate::recorder::{Recorder, SegmentGet};

fn main() {
    let recorder = Recorder::start();

    thread::sleep(Duration::from_secs(5));
    write_to("output.mp4", recorder.get());
}

fn write_to(path: impl AsRef<Path>, segments: SegmentGet) {
    let file = File::create(path.as_ref()).unwrap();
    let mut buf_writer = BufWriter::new(file);

    for segment in segments.bytes() {
        buf_writer.write_all(segment).unwrap()
    }
}
