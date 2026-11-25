use core::panic;
use std::{
    collections::VecDeque,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use objc2::{
    AnyThread, DefinedClass, Message, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_av_foundation::{
    AVAssetSegmentType, AVAssetWriter, AVAssetWriterDelegate, AVAssetWriterInput, AVFileTypeMPEG4,
    AVFileTypeProfileMPEG4AppleHLS, AVMediaTypeVideo, AVOutputSettingsAssistant,
    AVOutputSettingsPresetHEVC1920x1080, AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_foundation::{
    NSData, NSDictionary, NSMutableCopying, NSNumber, NSObject, NSObjectProtocol, NSString,
};
use objc2_uniform_type_identifiers::UTType;

pub struct Writer {
    writer: Retained<AVAssetWriter>,
    video_input: Retained<AVAssetWriterInput>,
    output: Retained<WriterOutput>,
    writing_start_claimed: AtomicBool,
    writing_started: AtomicBool,
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
            asset_writer.setDelegate(Some(ProtocolObject::from_ref(&*writer_output)));
        };

        Writer {
            writer: asset_writer,
            video_input,
            output: writer_output,
            writing_start_claimed: AtomicBool::new(false),
            writing_started: AtomicBool::new(false),
        }
    }

    /// Starts the writing process. Run this before feed().
    pub fn start(&self, start_time_stamp: CMTime) {
        if let Err(_) = self.writing_start_claimed.compare_exchange(
            false,
            true,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            panic!("attempted to start writer when writer has already begun writing")
        }
        unsafe { self.writer.setInitialSegmentStartTime(start_time_stamp) };
        if unsafe { !self.writer.startWriting() } {
            // Start failed.
            panic!(
                "writer failed to start writing, writer status: {:?}, error: {:?}",
                unsafe { self.writer.status() },
                unsafe { self.writer.error() }
            )
        }
        unsafe {
            self.writer.startSessionAtSourceTime(start_time_stamp);
        };
        self.writing_started.store(true, Ordering::Release);
    }

    pub fn started(&self) -> bool {
        self.writing_started.load(Ordering::Relaxed)
    }

    /// Feed a frame. Run this after start().
    pub fn feed(&self, sample_buffer: &CMSampleBuffer) {
        if !self.writing_started.load(Ordering::Acquire) {
            panic!("attempted to feed frames before writing began")
        }
        if unsafe { !self.video_input.isReadyForMoreMediaData() } {
            return;
        }
        let append_succeeded = unsafe { self.video_input.appendSampleBuffer(&sample_buffer) };
        if !append_succeeded {
            panic!(
                "appending sample buffer to writer failed, writer status: {:?}, error: {:?}",
                unsafe { self.writer.status() },
                unsafe { self.writer.error() }
            );
        }
    }

    /// Clear out all separable segments.
    pub fn clear(&self) {
        self.output.ivars().clear_separable_segments();
    }

    /// Copies out segments. `vec[0]` is the init segment.
    /// Returns None if not even the init segment exists yet.
    pub fn cut(&self) -> Option<Segments> {
        let separable_segment_guard = self.output.ivars().separable_segments.lock().unwrap();
        let init_segment = self.output.ivars().init_segment.get()?.0.clone();

        let separable_segments = separable_segment_guard
            .iter()
            .map(|segment| segment.0.clone())
            .collect();

        Some(Segments {
            init_segment,
            separable_segments,
        })
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

#[derive(Debug)]
pub struct Segments {
    init_segment: Retained<NSData>,
    separable_segments: Vec<Retained<NSData>>,
}

impl Segments {
    pub fn bytes(&self) -> impl Iterator<Item = &[u8]> {
        // SAFETY: data is immutable
        let init_segment = unsafe { self.init_segment.as_bytes_unchecked() };
        let separable_segments = self
            .separable_segments
            .iter()
            .map(|segment| unsafe { segment.as_bytes_unchecked() });
        std::iter::once(init_segment).chain(separable_segments)
    }
}

pub struct SendSyncNSData(Retained<NSData>);

// SAFETY: Segment data isn't mutable.
unsafe impl Send for SendSyncNSData {}
unsafe impl Sync for SendSyncNSData {}

struct WriterOutputIVars {
    init_segment: OnceLock<SendSyncNSData>,
    separable_segments: Mutex<VecDeque<SendSyncNSData>>,
}

impl WriterOutputIVars {
    fn insert_init_segment(&self, segment_data: SendSyncNSData) {
        if let Err(_) = self.init_segment.set(segment_data) {
            panic!("trying to insert init segment when it already exists");
        }
    }

    fn push_separable_segment(&self, segment_data: SendSyncNSData) {
        let mut guard = self.separable_segments.lock().unwrap();
        (*guard).push_back(segment_data);
        // TODO: take out front, keeping it ~= 30 seconds
    }

    fn clear_separable_segments(&self) {
        (*self.separable_segments.lock().unwrap()).clear();
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = WriterOutputIVars]
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
            let segment_data = SendSyncNSData(segment_data.retain());
            match segment_type {
                AVAssetSegmentType::Initialization => {
                    self.ivars().insert_init_segment(segment_data);
                }
                AVAssetSegmentType::Separable => {
                    self.ivars().push_separable_segment(segment_data);
                }
                _ => unreachable!(),
            }
            println!("segment type: {:?}", segment_type);
        }
    }
);

impl WriterOutput {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(WriterOutputIVars {
            init_segment: OnceLock::new(),
            separable_segments: Mutex::new(VecDeque::new()),
        });
        unsafe { msg_send![super(this), init] }
    }
}
