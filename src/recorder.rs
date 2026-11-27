mod encoder;
mod picker;
mod stream;
mod writer;

use std::sync::Arc;

use crate::recorder::{stream::Stream, writer::Segments};

pub use crate::recorder::writer::SegmentGet;

pub struct Recorder {
    _stream: Stream,
    writer_out: Arc<Segments>,
}

impl Recorder {
    /// Start recording. Use .get() to get the last x seconds.
    pub fn start() -> Self {
        let content_filter = picker::pick().unwrap();

        let point_pixel_scale = unsafe { content_filter.pointPixelScale() };
        let rect = unsafe { content_filter.contentRect() }.standardize().size;
        // Width and height of the captured video, in pixels
        let (width, height) = (
            (rect.width * f64::from(point_pixel_scale)).round() as usize,
            (rect.height * f64::from(point_pixel_scale)).round() as usize,
        );

        let (stream_out, encoder_in) = std::sync::mpsc::channel();
        let (encoder_out, writer_in) = std::sync::mpsc::channel();
        let stream = Stream::start(content_filter, width, height, stream_out);
        encoder::spawn(width, height, encoder_in, encoder_out);
        let writer_out = writer::spawn(writer_in);

        Self {
            _stream: stream,
            writer_out,
        }
    }

    pub fn get(&self) -> SegmentGet {
        self.writer_out.get()
    }
}
