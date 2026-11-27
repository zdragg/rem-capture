use core::panic;
use std::{
    cell::OnceCell,
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
        mpsc::Receiver,
    },
};

use objc2::{
    AnyThread, DefinedClass, Message, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_av_foundation::{
    AVAssetSegmentReport, AVAssetSegmentType, AVAssetWriter, AVAssetWriterDelegate,
    AVAssetWriterInput, AVFileTypeMPEG4, AVFileTypeProfileMPEG4AppleHLS, AVMediaTypeVideo,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString, CFType};
use objc2_core_media::{
    CMSampleBuffer, CMTime, CMTimeFlags, kCMSampleAttachmentKey_NotSync, kCMTimeIndefinite,
};
use objc2_foundation::{NSData, NSObject, NSObjectProtocol};
use objc2_uniform_type_identifiers::UTType;

use crate::recorder::encoder::EncodedCMSampleBuffer;

/// Starts the writer thread. The thread will block on the input and output frames
/// to the returned `Arc<Segments>`.
pub(super) fn spawn(
    to_writer: Receiver<EncodedCMSampleBuffer>,
    length_limit: usize,
) -> Arc<Segments> {
    let segments_output = Arc::new(Segments::new(length_limit));
    spawn_thread(to_writer, segments_output.clone());
    segments_output
}

fn spawn_thread(to_writer: Receiver<EncodedCMSampleBuffer>, segments: Arc<Segments>) {
    std::thread::spawn(move || {
        let writer = Writer::new(segments);
        while let Ok(frame) = to_writer.recv() {
            writer.feed(&frame);
        }
        // exit after sender drops
    });
}

struct Writer {
    writer_and_input: OnceCell<FirstFrameInit>,
    writer_out: Retained<WriterOutput>,
}

/// Content initialized on the first frame
struct FirstFrameInit {
    writer: Retained<AVAssetWriter>,
    writer_in: Retained<AVAssetWriterInput>,
}

impl Writer {
    fn new(segments: Arc<Segments>) -> Self {
        let writer_out = WriterOutput::new(segments);
        Self {
            writer_and_input: OnceCell::new(),
            writer_out,
        }
    }

    /// Feed a frame.
    fn feed(&self, frame: &CMSampleBuffer) {
        // runs initialization code on the first frame only
        let FirstFrameInit { writer, writer_in } = self
            .writer_and_input
            .get_or_init(|| self.first_frame_init(frame));

        if is_keyframe(frame) {
            unsafe { writer.flushSegment() };
        }

        if unsafe { !writer_in.isReadyForMoreMediaData() } {
            return;
        }
        let append_succeeded = unsafe { writer_in.appendSampleBuffer(&frame) };
        if !append_succeeded {
            panic!(
                "appending sample buffer to writer failed, writer status: {:?}, error: {:?}",
                unsafe { writer.status() },
                unsafe { writer.error() }
            );
        }
    }

    /// Initializes, should only run ONCE per writer
    fn first_frame_init(&self, frame: &CMSampleBuffer) -> FirstFrameInit {
        let format_description = unsafe { frame.format_description() }
            .expect("sample buffer should have a format description");
        // Create video input. Frames are piped into this variable!
        // SAFETY: output_settings is None.
        let writer_in = unsafe {
            AVAssetWriterInput::initWithMediaType_outputSettings_sourceFormatHint(
                AVAssetWriterInput::alloc(),
                AVMediaTypeVideo.expect("AVMediaType Video should be available"),
                None,
                Some(&format_description),
            )
        };
        unsafe {
            writer_in.setExpectsMediaDataInRealTime(true);
        }

        // Create the writer with the video input attached. The writer writes frames
        // received from the video input into the output.
        let writer = unsafe {
            AVAssetWriter::initWithContentType(
                AVAssetWriter::alloc(),
                &UTType::typeWithIdentifier(
                    &AVFileTypeMPEG4.expect("AVFileType MPEG4 should be avaiable"),
                )
                .expect("system should know the mpeg-4 identifier"),
            )
        };
        unsafe {
            writer.addInput(&writer_in);
            writer.setOutputFileTypeProfile(Some(
                AVFileTypeProfileMPEG4AppleHLS
                    .expect("AVFileTypeProfile MPEG4AppleHLS should be available"),
            ));
            writer.setPreferredOutputSegmentInterval(kCMTimeIndefinite);
            writer.setDelegate(Some(ProtocolObject::from_ref(&*self.writer_out)));
        };

        // Start writing!
        let start_timestamp = unsafe { frame.presentation_time_stamp() };
        if unsafe { !writer.startWriting() } {
            // Start failed.
            panic!(
                "writer failed to start writing, writer status: {:?}, error: {:?}",
                unsafe { writer.status() },
                unsafe { writer.error() }
            )
        }
        unsafe {
            writer.startSessionAtSourceTime(start_timestamp);
        };

        FirstFrameInit { writer, writer_in }
    }
}

fn is_keyframe(buffer: &CMSampleBuffer) -> bool {
    let Some(attachments) = (unsafe { buffer.sample_attachments_array(false) }) else {
        panic!("The CMSampleBuffer should have attachments")
    };

    // SAFETY: All sample keys are CFStrings. <https://developer.apple.com/documentation/coremedia/sample-attachment-keys>
    let attachments = unsafe { attachments.cast_unchecked::<CFDictionary<CFString, CFType>>() };

    // The buffer is guaranteed to have one and only one sample
    // (from the check on the sender side of the channel)
    // which means that there should be one and only one attachment.
    let dict = attachments
        .get(0)
        .expect("The CMSampleBuffer should have an attachment");

    match dict.get(unsafe { kCMSampleAttachmentKey_NotSync }) {
        Some(not_sync) => {
            // <https://developer.apple.com/documentation/coremedia/kcmsampleattachmentkey_notsync>
            let not_keyframe: CFRetained<CFBoolean> = CFRetained::downcast(not_sync)
                .expect("The key kCMSampleAttachmentKey_NotSync should be mapped to a CFBoolean");
            !not_keyframe.value()
        }
        // No NotSync -> yes keyframe
        None => true,
    }
}

#[derive(Clone)]
struct SegmentData(Retained<NSData>);

// SAFETY: Segment data is probably immutable.
unsafe impl Send for SegmentData {}
unsafe impl Sync for SegmentData {}

impl AsRef<[u8]> for SegmentData {
    fn as_ref(&self) -> &[u8] {
        // SAFETY: Segment data is probably immutable.
        unsafe { self.0.as_bytes_unchecked() }
    }
}

pub(super) struct Segments {
    init_segment: OnceLock<SegmentData>,
    separable_segments: Mutex<VecDeque<(SegmentData, CMTime)>>,
    generation: AtomicUsize,
    new_segment: Condvar,
    // How much video is kept in memory, in seconds.
    length_limit: usize,
}

impl Segments {
    /// Returns None if not even the init segment exists yet.
    pub fn get(&self) -> Vec<impl AsRef<[u8]>> {
        // Wait for the next segment to be outputted
        let mut separable_segments_guard = self.separable_segments.lock().unwrap();
        let current_gen = self.generation.load(Ordering::Relaxed);
        while current_gen == self.generation.load(Ordering::Relaxed) {
            separable_segments_guard = self.new_segment.wait(separable_segments_guard).unwrap();
        }

        let init_segment = self
            .init_segment
            .get()
            .expect("The init segment should exist at this point")
            .to_owned();
        let separable_segments = separable_segments_guard
            .iter()
            .map(|data| data.0.to_owned());
        let mut vec = vec![init_segment];
        vec.extend(separable_segments);

        vec
    }

    fn new(length_limit: usize) -> Self {
        Self {
            init_segment: OnceLock::new(),
            separable_segments: Mutex::new(VecDeque::new()),
            generation: AtomicUsize::new(0),
            new_segment: Condvar::new(),
            length_limit,
        }
    }

    fn insert_init_segment(&self, segment_data: SegmentData) {
        if let Err(_) = self.init_segment.set(segment_data) {
            panic!("trying to insert init segment when it already exists");
        }
    }

    fn push_separable_segment(&self, segment_data: SegmentData, latest_pts: CMTime) {
        let mut guard = self.separable_segments.lock().unwrap();
        guard.push_back((segment_data, latest_pts));
        let pop_if_pts_smaller_than_this =
            unsafe { latest_pts.subtract(CMTime::with_seconds(self.length_limit as f64, 1i32)) };
        while guard
            .pop_front_if(|frame| unsafe { frame.1.compare(pop_if_pts_smaller_than_this) } < 0)
            .is_some()
        {
            // pop pop pop
        }

        self.generation.fetch_add(1, Ordering::Relaxed);

        // Dropping the guard must happen after the generation increment, or else:
        // this thread: push segment and drop guard
        // consumer thread: load generation, finds N
        // consumer thread: tries to get frame, so acquire guard
        // consumer thread: checks generation: generation is still N! prepare to sleep
        // this thread: N += 1 and notify_one()
        // consumer thread: NOT in sleep yet, so notify_one() does nothing.
        // consumer thread: goes to sleep until the next segment push wakes it up
        //
        // If the guard is dropped after the increment, then the
        // consumer thread cannot acquire lock before the generation variable
        // accurately reflects the true generation number.
        drop(guard);

        self.new_segment.notify_one();
    }
}

struct WriterOutputIVars {
    segments: Arc<Segments>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = WriterOutputIVars]
    struct WriterOutput;

    unsafe impl NSObjectProtocol for WriterOutput {}

    unsafe impl AVAssetWriterDelegate for WriterOutput {
        #[unsafe(method(assetWriter:didOutputSegmentData:segmentType:segmentReport:))]
        fn did_output(
            &self,
            writer: &AVAssetWriter,
            segment_data: &NSData,
            segment_type: AVAssetSegmentType,
            segment_report: Option<&AVAssetSegmentReport>,
        ) {
            let segments = &self.ivars().segments;
            let segment_data = SegmentData(segment_data.retain());
            match segment_type {
                AVAssetSegmentType::Initialization => {
                    segments.insert_init_segment(segment_data);
                }
                AVAssetSegmentType::Separable => {
                    let latest_pts = find_latest_pts(segment_report);
                    segments.push_separable_segment(segment_data, latest_pts);
                }
                _ => unreachable!(),
            }
        }
    }
);

impl WriterOutput {
    fn new(segments: Arc<Segments>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(WriterOutputIVars { segments });
        unsafe { msg_send![super(this), init] }
    }
}

fn find_latest_pts(report: Option<&AVAssetSegmentReport>) -> CMTime {
    let reports = unsafe {
        report
            .expect("separable segments should come with a report")
            .trackReports()
    };

    let media_type_video =
        unsafe { AVMediaTypeVideo.expect("AVMediaTypeVideo should be available") };

    for report in reports {
        let media_type = unsafe { &*report.mediaType() };
        if media_type != media_type_video {
            continue;
        }
        let earliest_pts = unsafe { report.earliestPresentationTimeStamp() };

        // using bitflags here because "Do not test a time against [kCMTimeInvalid] using time == invalid"
        // <https://developer.apple.com/documentation/coremedia/cmtime/invalid>
        if !earliest_pts.flags.contains(CMTimeFlags::Valid) {
            panic!("segment report should contain valid presentation timestamp");
        }
        let duration = unsafe { report.duration() };
        if !duration.flags.contains(CMTimeFlags::Valid) {
            panic!("segment report should contain valid duration");
        }
        return unsafe { earliest_pts.add(duration) };
    }
    panic!("segment report should contain video track info")
}
