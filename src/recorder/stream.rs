use std::{ops::Deref, sync::mpsc::Sender};

use block2::RcBlock;
use objc2::{
    AnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_core_foundation::{CFRetained, Type};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCStreamOutput, SCStreamOutputType,
};

pub(super) struct Stream {
    stream: Retained<SCStream>,
    _stream_output_handler: Retained<StreamOutputHandler>,
    _stream_delegate: Retained<StreamDelegate>,
}

impl Stream {
    pub(super) fn start(
        content_filter: Retained<SCContentFilter>,
        width: usize,
        height: usize,
        stream_out: Sender<StreamCMSampleBuffer>,
    ) -> Self {
        let stream_config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            stream_config.setCaptureResolution(SCCaptureResolutionType::Best);
            stream_config.setWidth(width);
            stream_config.setHeight(height);
        };
        let stream_delegate = StreamDelegate::new();
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &content_filter,
                &stream_config,
                Some(ProtocolObject::from_ref(&*stream_delegate)),
            )
        };

        // Add output handler
        let stream_output_handler = StreamOutputHandler::new(stream_out);
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*stream_output_handler),
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
            _stream_output_handler: stream_output_handler,
            _stream_delegate: stream_delegate,
        }
    }
}

impl Drop for Stream {
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

pub struct StreamCMSampleBuffer(CFRetained<CMSampleBuffer>);

impl Deref for StreamCMSampleBuffer {
    type Target = CMSampleBuffer;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// SAFETY: The stream probably wouldn't modify the buffer after outputting it
unsafe impl Send for StreamCMSampleBuffer {}

struct StreamOutputHandlerIVars {
    stream_out: Sender<StreamCMSampleBuffer>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = StreamOutputHandlerIVars]
    struct StreamOutputHandler;

    unsafe impl NSObjectProtocol for StreamOutputHandler {}

    unsafe impl SCStreamOutput for StreamOutputHandler {
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

            self.ivars()
                .stream_out
                .send(StreamCMSampleBuffer(sample_buffer.retain()))
                .expect("receiver dropped");
        }
    }
);

impl StreamOutputHandler {
    fn new(stream_out: Sender<StreamCMSampleBuffer>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(StreamOutputHandlerIVars { stream_out });
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    struct StreamDelegate;

    unsafe impl NSObjectProtocol for StreamDelegate {}

    unsafe impl SCStreamDelegate for StreamDelegate {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, stream: &SCStream, error: &NSError) {
            panic!("stream stopped with error: {:?}", error);
        }
    }
);

impl StreamDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}
