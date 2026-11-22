use std::{
    ptr::NonNull,
    sync::mpsc::{Receiver, Sender},
};

use block2::RcBlock;
use objc2::{
    AnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_core_foundation::{CFRetained, kCFAllocatorDefault};
use objc2_core_media::{CMSampleBuffer, kCMVideoCodecType_HEVC};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType,
};
use objc2_video_toolbox::{VTCompressionSession, VTEncodeInfoFlags};

pub struct Recorder {
    stream: Retained<SCStream>,
    output: Retained<OutputHandler>,
}

impl Recorder {
    /// Sets up a stream and output into encoder, with provided filter.
    pub fn new(content_filter: Retained<SCContentFilter>) -> Self {
        let stream_config = unsafe { SCStreamConfiguration::new() };
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &content_filter,
                &stream_config,
                None,
            )
        };

        // Setup compression session
        let mut session_ptr = std::ptr::null_mut();
        let status = unsafe {
            VTCompressionSession::create(
                kCFAllocatorDefault,
                stream_config.width() as i32,
                stream_config.height() as i32,
                kCMVideoCodecType_HEVC,
                None,
                None,
                None,
                None,
                std::ptr::null_mut(),
                NonNull::from_mut(&mut session_ptr),
            )
        };
        assert_eq!(status, 0, "VTCompressionSession creation failed ({status})");
        let Some(nonnull_session_ptr) = NonNull::new(session_ptr) else {
            panic!("VTCompressionSession creation failed")
        };
        let session = unsafe { CFRetained::from_raw(nonnull_session_ptr) };

        // Add output handler
        let output = OutputHandler::new(session);
        if let Err(e) = unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Screen,
                None,
            )
        } {
            panic!("failed to add output to stream: {e}");
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

    /// Gets encoded frames without blocking.
    pub fn frames(&self) -> impl Iterator<Item = CFRetained<CMSampleBuffer>> {
        self.output
            .ivars()
            .frame_rx
            .try_iter()
            .map(|send_buf| send_buf.0)
    }
}

struct SendCMSampleBuffer(CFRetained<CMSampleBuffer>);

// SAFETY: Video Toolbox probably doesn't modify the sample buffer after it finishes encoding.
unsafe impl Send for SendCMSampleBuffer {}

struct OutputHandlerIVars {
    compression_session: CFRetained<VTCompressionSession>,
    frame_tx: Sender<SendCMSampleBuffer>,
    frame_rx: Receiver<SendCMSampleBuffer>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = OutputHandlerIVars]
    struct OutputHandler;

    unsafe impl NSObjectProtocol for OutputHandler {}

    unsafe impl SCStreamOutput for OutputHandler {
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

            let compression_output_handler = RcBlock::new({
                let frame_tx = self.ivars().frame_tx.clone();
                move |status: i32, _flags: VTEncodeInfoFlags, frame: *mut CMSampleBuffer| {
                    assert_eq!(status, 0, "frame encoding failed ({status})");
                    let frame = match NonNull::new(frame) {
                        Some(frame) => SendCMSampleBuffer(unsafe { CFRetained::retain(frame) }),
                        None => {
                            // frame was dropped
                            return;
                        }
                    };
                    frame_tx.send(frame).expect("receiver dropped");
                }
            });

            let status = unsafe {
                self.ivars()
                    .compression_session
                    .encode_frame_with_output_handler(
                        &buffer,
                        sample_buffer.presentation_time_stamp(),
                        sample_buffer.duration(),
                        None,
                        std::ptr::null_mut(),
                        RcBlock::as_ptr(&compression_output_handler),
                    )
            };
            assert_eq!(status, 0, "frame encoding failed ({status})");
        }
    }
);

impl OutputHandler {
    fn new(compression_session: CFRetained<VTCompressionSession>) -> Retained<Self> {
        let (frame_tx, frame_rx) = std::sync::mpsc::channel();
        let this = Self::alloc().set_ivars(OutputHandlerIVars {
            compression_session,
            frame_tx,
            frame_rx,
        });
        unsafe { msg_send![super(this), init] }
    }
}
