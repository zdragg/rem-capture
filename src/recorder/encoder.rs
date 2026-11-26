use std::{cell::Cell, ffi::c_void, ops::Deref, ptr::NonNull, sync::mpsc::Sender};

use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, kCFAllocatorDefault};
use objc2_core_media::{CMSampleBuffer, kCMVideoCodecType_HEVC};
use objc2_video_toolbox::{
    VTCompressionSession, VTEncodeInfoFlags, kVTEncodeFrameOptionKey_ForceKeyFrame,
};

pub struct Encoder {
    session: CFRetained<VTCompressionSession>,
    force_segment_output: Cell<bool>,
}

pub struct EncodedCMSampleBuffer(CFRetained<CMSampleBuffer>);

impl Deref for EncodedCMSampleBuffer {
    type Target = CMSampleBuffer;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// SAFETY: CMSampleBuffer probably wouldn't be modified after the encoding completes
unsafe impl Send for EncodedCMSampleBuffer {}

impl Encoder {
    pub fn new(width: usize, height: usize, encoded_tx: Sender<EncodedCMSampleBuffer>) -> Self {
        // Setup compression session
        let mut session_ptr = std::ptr::null_mut();
        let status = unsafe {
            VTCompressionSession::create(
                kCFAllocatorDefault,
                width as i32,
                height as i32,
                kCMVideoCodecType_HEVC,
                None,
                None,
                None,
                Some(on_encoded),
                Box::into_raw(Box::new(encoded_tx)).cast(),
                NonNull::from_mut(&mut session_ptr),
            )
        };
        assert_eq!(status, 0, "VTCompressionSession creation failed ({status})");
        let Some(nonnull_session_ptr) = NonNull::new(session_ptr) else {
            panic!("VTCompressionSession creation failed")
        };
        let session = unsafe { CFRetained::from_raw(nonnull_session_ptr) };

        Encoder {
            session,
            force_segment_output: Cell::new(false),
        }
    }

    // Set next frame to a keyframe, then `Writer::feed`: will flush the segment.
    pub fn force_segment_output(&self) {
        self.force_segment_output.set(true);
    }

    pub fn encode(&self, cm_buffer: &CMSampleBuffer) {
        let Some(cv_buffer) = (unsafe { cm_buffer.image_buffer() }) else {
            return;
        };

        // Force next frame to be a keyframe if flag is set
        let frame_properties = if self.force_segment_output.get() {
            self.force_segment_output.set(false);
            let dict = CFDictionary::from_slices(
                &[unsafe { kVTEncodeFrameOptionKey_ForceKeyFrame }],
                &[CFBoolean::new(true)],
            );
            Some(dict)
        } else {
            None
        };

        let status = unsafe {
            self.session.encode_frame(
                &cv_buffer,
                cm_buffer.presentation_time_stamp(),
                cm_buffer.duration(),
                frame_properties.as_ref().map(|dict| dict.as_opaque()),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, 0, "frame encoding failed ({status})");
    }
}

unsafe extern "C-unwind" fn on_encoded(
    encoded_frame_tx_ptr: *mut c_void,
    _frame_refcon: *mut c_void,
    status: i32, // OSStatus is an alias for i32
    _flags: VTEncodeInfoFlags,
    frame: *mut CMSampleBuffer,
) {
    assert_eq!(status, 0, "frame encoding failed ({status})");
    let frame = match NonNull::new(frame) {
        Some(frame) => unsafe { CFRetained::retain(frame) },
        None => {
            // frame was dropped
            return;
        }
    };

    let num_samples = unsafe { frame.num_samples() };
    assert_eq!(
        num_samples, 1,
        "expected 1 media sample in CMSampleBuffer, found {num_samples}"
    );

    let encoded_frame_sender: &Sender<CFRetained<CMSampleBuffer>> =
        unsafe { &*(encoded_frame_tx_ptr.cast()) };
    encoded_frame_sender.send(frame).expect("receiver dropped")
}
