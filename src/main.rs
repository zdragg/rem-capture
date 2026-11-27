use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    thread,
    time::Duration,
};

use rem_capture::Recorder;

fn main() {
    let recorder = Recorder::start(5).expect("user shouldn't cancel prompt");
    thread::sleep(Duration::from_secs(3));
    let bytes = recorder.get();
    write_to("output1.mp4", bytes);

    thread::sleep(Duration::from_secs(3));
    let bytes = recorder.get();
    write_to("output2.mp4", bytes);

    thread::sleep(Duration::from_secs(3));
    let bytes = recorder.get();
    write_to("output3.mp4", bytes);
}

fn write_to<'a>(path: impl AsRef<Path>, segments: impl IntoIterator<Item = impl AsRef<[u8]>>) {
    let file = File::create(path.as_ref()).unwrap();
    let mut buf_writer = BufWriter::new(file);

    for segment in segments {
        buf_writer.write_all(segment.as_ref()).unwrap()
    }
}
