use objc2::{AnyThread, define_class, msg_send, rc::Retained, runtime::ProtocolObject};
use objc2_av_foundation::{
    AVAssetSegmentType, AVAssetWriter, AVAssetWriterDelegate, AVAssetWriterInput, AVFileTypeMPEG4,
    AVFileTypeProfileMPEG4AppleHLS, AVMediaTypeVideo, AVOutputSettingsAssistant,
    AVOutputSettingsPresetHEVC1920x1080, AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_core_media::CMTime;
use objc2_foundation::{
    NSData, NSDictionary, NSMutableCopying, NSNumber, NSObject, NSObjectProtocol, NSString,
};
use objc2_uniform_type_identifiers::UTType;

pub struct Writer {
    writer: Retained<AVAssetWriter>,
    output: Retained<WriterOutput>,
}

impl Writer {
    pub fn new(width: usize, height: usize) -> Self {
        let asset_writer = unsafe {
            AVAssetWriter::initWithContentType(
                AVAssetWriter::alloc(),
                &UTType::typeWithIdentifier(
                    &AVFileTypeMPEG4.expect("AVFileType MPEG4 should be avaiable"),
                )
                .expect("system should know the mpeg-4 identifier"),
            )
        };

        let (video_settings, audio_settings) = Self::get_output_settings(width, height);
        let video_input = unsafe {
            AVAssetWriterInput::initWithMediaType_outputSettings(
                AVAssetWriterInput::alloc(),
                AVMediaTypeVideo.expect("AVMediaType Video should be available"),
                Some(&video_settings),
            )
        };
        unsafe {
            video_input.setExpectsMediaDataInRealTime(true);
        }

        let writer_output = WriterOutput::new();

        unsafe {
            asset_writer.addInput(&video_input);
            asset_writer.setOutputFileTypeProfile(Some(
                AVFileTypeProfileMPEG4AppleHLS
                    .expect("AVFileTypeProfile MPEG4AppleHLS should be available"),
            ));
            asset_writer.setPreferredOutputSegmentInterval(CMTime::with_seconds(1f64, 1i32));
            // asset_writer.setInitialSegmentStartTime(kCMTimeZero)
            asset_writer.setDelegate(Some(ProtocolObject::from_ref(&*writer_output)));
        };

        Writer {
            writer: asset_writer,
            output: writer_output,
        }
    }

    pub fn start(&self) {
        if unsafe { !self.writer.startWriting() } {
            // Start failed.
            panic!(
                "writer failed to start writing, writer status: {:?}, error: {:?}",
                unsafe { self.writer.status() },
                unsafe { self.writer.error() }
            )
        }
    }

    fn get_output_settings(
        width: usize,
        height: usize,
    ) -> (
        Retained<NSDictionary<NSString>>,
        Retained<NSDictionary<NSString>>,
    ) {
        let output_settings = unsafe {
            AVOutputSettingsAssistant::outputSettingsAssistantWithPreset(
                AVOutputSettingsPresetHEVC1920x1080,
            )
            .expect("AVOutputSettingsPreset HEVC1920x1080 should be available")
        };
        let audio_settings = unsafe { output_settings.audioSettings() }
            .expect("AVOutputSettingsPreset HEVC1920x1080 should contain audio settings");
        let video_settings = unsafe { output_settings.videoSettings() }
            .expect("AVOutputSettingsPreset HEVC1920x1080 should contain video settings");

        let modified_video_settings = {
            let mutable_dict = video_settings.mutableCopy();
            let width = NSNumber::new_usize(width);
            let height = NSNumber::new_usize(height);
            let width_key = ProtocolObject::from_ref(
                unsafe { AVVideoWidthKey }.expect("AVVideoWidthKey should be available"),
            );
            let height_key = ProtocolObject::from_ref(
                unsafe { AVVideoHeightKey }.expect("AVVideoHeightKey should be available"),
            );
            // SAFETY: AVVideoWidthKey and AVVideoHeightKey corresponds to NSNumber.
            // <https://developer.apple.com/documentation/avfoundation/avvideoheightkey>
            // <https://developer.apple.com/documentation/avfoundation/avvideowidthkey>
            unsafe {
                mutable_dict.setObject_forKey(&width, width_key);
                mutable_dict.setObject_forKey(&height, height_key);
            }
            Retained::into_super(mutable_dict)
        };
        (modified_video_settings, audio_settings)
    }
}

define_class!(
    #[unsafe(super(NSObject))]

    struct WriterOutput;

    unsafe impl NSObjectProtocol for WriterOutput {}

    unsafe impl AVAssetWriterDelegate for WriterOutput {
        #[unsafe(method(assetWriter:didOutputSegmentData:segmentType:))]
        fn did_output(
            &self,
            writer: &AVAssetWriter,
            segment_data: &NSData,
            segment_type: AVAssetSegmentType,
        ) {
        }
    }
);

impl WriterOutput {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}
