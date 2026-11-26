pub mod encoder;
pub mod writer;

use std::sync::Arc;

use block2::RcBlock;
use objc2::{
    AnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCStream, SCStreamConfiguration, SCStreamOutput,
    SCStreamOutputType,
};

use crate::recorder::{
    encoder::Encoder,
    writer::{SegmentGet, Segments},
};

pub struct Recorder {
    stream: Retained<SCStream>,
    stream_output: Retained<StreamOutput>,
}

impl Recorder {
    /// Sets up a stream and output into encoder. (Recorder, width in pixels, height in pixels)
    pub fn start(content_filter: Retained<SCContentFilter>) -> Self {
        let point_pixel_scale = unsafe { content_filter.pointPixelScale() };
        let rect = unsafe { content_filter.contentRect() }.standardize().size;
        // Width and height of the captured video, in pixels
        let (width, height) = (
            (rect.width * f64::from(point_pixel_scale)).round() as usize,
            (rect.height * f64::from(point_pixel_scale)).round() as usize,
        );

        let stream_config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            stream_config.setCaptureResolution(SCCaptureResolutionType::Best);
            stream_config.setWidth(width);
            stream_config.setHeight(height);
        };
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &content_filter,
                &stream_config,
                None,
            )
        };

        // Add output handler
        let stream_output = StreamOutput::new(width, height);
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*stream_output),
                    SCStreamOutputType::Screen,
                    None,
                )
                .expect("failed to add output to stream");
        }

        // Start stream
        let block = RcBlock::new(|error: *mut NSError| {
            if let Some(err) = unsafe { error.as_ref() } {
                panic!("failed to start stream: {}", err);
            }
        });
        unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };

        Self {
            stream,
            stream_output,
        }
    }

    pub fn get(&self) -> SegmentGet {
        self.stream_output.ivars().encoder.force_segment_output();
        self.stream_output.ivars().segments_output.get()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let block = RcBlock::new(|error: *mut NSError| {
            if let Some(err) = unsafe { error.as_ref() } {
                panic!("failed to stop stream: {}", err);
            }
        });
        unsafe {
            self.stream.stopCaptureWithCompletionHandler(Some(&block));
        }
    }
}

struct OutputHandlerIVars {
    encoder: Encoder,
    segments_output: Arc<Segments>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = OutputHandlerIVars]
    struct StreamOutput;

    unsafe impl NSObjectProtocol for StreamOutput {}

    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output_sample_buffer(
            &self,
            stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            r#type: SCStreamOutputType,
        ) {
            assert_eq!(
                r#type,
                SCStreamOutputType::Screen,
                "SCStream output type is not Screen"
            );

            self.ivars().encoder.encode(sample_buffer);
        }
    }
);

impl StreamOutput {
    fn new(width: usize, height: usize) -> Retained<Self> {
        // Channel to send encoded frames FROM the encoder TO the writer
        let (encoded_frame_tx, encoded_frame_rx) = std::sync::mpsc::channel();
        let encoder = Encoder::new(width, height, encoded_frame_tx);

        // Spawn writer thread in the background
        let segments_output = writer::start(encoded_frame_rx);

        let this = Self::alloc().set_ivars(OutputHandlerIVars {
            encoder,
            segments_output,
        });
        unsafe { msg_send![super(this), init] }
    }
}
