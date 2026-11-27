//! Pick using Picker, then Stream -> Encoder -> Writer.

mod encoder;
mod picker;
mod stream;
mod writer;

use std::sync::{Arc, mpsc::Sender};

use objc2_core_foundation::CFRetained;
use objc2_core_media::CMSampleBuffer;

use crate::recorder::{stream::Stream, writer::Segments};

/// A recorder.
///
/// [`Recorder::start()`] starts the recorder and returns a handle. \
/// [`Recorder::get()`] returns an iterator of bytes that can be combined into a HEVC MPEG-4 video.
pub struct Recorder {
    _stream: Stream, // kept alive here
    writer_output: Arc<Segments>,
    to_encoder: Sender<EncoderCommand>, // Send commands to encoder
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

        let (to_encoder, from_stream) = std::sync::mpsc::channel();
        let (to_writer, from_encoder) = std::sync::mpsc::channel();
        let stream = Stream::start(content_filter, width, height, to_encoder.clone());
        encoder::spawn(width, height, from_stream, to_writer);
        let writer_output = writer::spawn(from_encoder);

        Self {
            _stream: stream,
            writer_output,
            to_encoder,
        }
    }

    pub fn get(&self) -> Vec<impl AsRef<[u8]>> {
        self.force_keyframe();
        self.writer_output.get()
    }

    fn force_keyframe(&self) {
        self.to_encoder.send(EncoderCommand::ForceKeyframe).unwrap();
    }
}

enum EncoderCommand {
    Encode(CFRetained<CMSampleBuffer>),
    ForceKeyframe,
}

// SAFETY: The stream probably wouldn't modify the buffer after outputting it
unsafe impl Send for EncoderCommand {}
