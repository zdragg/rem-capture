use std::sync::mpsc::{Receiver, Sender};

use block2::RcBlock;
use objc2::{AnyThread, define_class, msg_send, rc::Retained, runtime::ProtocolObject};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCStream, SCStreamConfiguration, SCStreamOutput,
    SCStreamOutputType,
};

use crate::writer::Writer;

pub struct Recorder {
    stream: Retained<SCStream>,
    output: Retained<StreamOutput>,
}

impl Recorder {
    /// Sets up a stream and output into encoder. (Recorder, width in pixels, height in pixels)
    pub fn new(content_filter: Retained<SCContentFilter>) -> Self {
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

        let writer = Writer::new(width, height);

        // Add output handler
        let output = StreamOutput::new();
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*output),
                    SCStreamOutputType::Screen,
                    None,
                )
                .expect("failed to add output to stream");
        }

        Self { stream, output }
    }

    pub fn start(&self) {
        let block = RcBlock::new(|error: *mut NSError| {
            if let Some(err) = unsafe { error.as_ref() } {
                panic!("failed to start stream: {}", err);
            }
        });
        unsafe { self.stream.startCaptureWithCompletionHandler(Some(&block)) };
    }

    pub fn stop(&self) {
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

pub struct FragmentedBlock {}

struct OutputHandlerIVars {
    frame_tx: Sender<FragmentedBlock>,
    frame_rx: Receiver<FragmentedBlock>,
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

            let Some(buffer) = (unsafe { sample_buffer.image_buffer() }) else {
                return;
            };
            // println!(
            //     "{:?}, {:?}, {:?}",
            //     CVImageBufferGetDisplaySize(&buffer),
            //     CVImageBufferGetCleanRect(&buffer),
            //     CVImageBufferGetEncodedSize(&buffer)
            // );
        }
    }
);

impl StreamOutput {
    fn new() -> Retained<Self> {
        let (frame_tx, frame_rx) = std::sync::mpsc::channel();
        let this = Self::alloc().set_ivars(OutputHandlerIVars { frame_tx, frame_rx });
        unsafe { msg_send![super(this), init] }
    }
}
