use std::{
    cell::Cell,
    ffi::c_void,
    ops::Deref,
    ptr::NonNull,
    sync::mpsc::{Receiver, Sender},
};

use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, kCFAllocatorDefault};
use objc2_core_media::{CMSampleBuffer, kCMTimeInvalid, kCMVideoCodecType_HEVC};
use objc2_video_toolbox::{
    VTCompressionSession, VTEncodeInfoFlags, VTSessionSetProperty,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, kVTEncodeFrameOptionKey_ForceKeyFrame,
};

use crate::recorder::EncoderCommand;

pub(super) fn spawn(
    width: usize,
    height: usize,
    from_stream: Receiver<EncoderCommand>,
    to_writer: Sender<EncodedCMSampleBuffer>,
) {
    std::thread::spawn(move || {
        let writer = Encoder::new(width, height, to_writer);
        while let Ok(command) = from_stream.recv() {
            match command {
                EncoderCommand::Encode(frame) => {
                    writer.encode(&frame);
                }
                EncoderCommand::ForceKeyframe => {
                    writer.force_keyframe.set(true);
                }
            }
        }
        // exit after sender drops
    });
}

pub(super) struct Encoder {
    session: CFRetained<VTCompressionSession>,
    encoder_out_ptr: *mut Sender<EncodedCMSampleBuffer>,
    force_keyframe: Cell<bool>,
}

impl Encoder {
    pub(super) fn new(
        width: usize,
        height: usize,
        encoder_out: Sender<EncodedCMSampleBuffer>,
    ) -> Self {
        let encoder_out_ptr = Box::into_raw(Box::new(encoder_out));

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
                encoder_out_ptr.cast(),
                NonNull::from_mut(&mut session_ptr),
            )
        };
        assert_eq!(status, 0, "VTCompressionSession creation failed ({status})");
        let Some(nonnull_session_ptr) = NonNull::new(session_ptr) else {
            panic!("VTCompressionSession creation failed")
        };
        let session = unsafe { CFRetained::from_raw(nonnull_session_ptr) };

        // Maximum 1 second between keyframes
        unsafe {
            VTSessionSetProperty(
                &session,
                kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
                Some(&CFNumber::new_i32(1)),
            )
        };

        Encoder {
            session,
            encoder_out_ptr,
            force_keyframe: Cell::new(false),
        }
    }

    pub(super) fn encode(&self, cm_buffer: &CMSampleBuffer) {
        let Some(cv_buffer) = (unsafe { cm_buffer.image_buffer() }) else {
            return;
        };

        // Force next frame to be a keyframe if flag is set
        let frame_properties = if self.force_keyframe.get() {
            self.force_keyframe.set(false);
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

pub(super) struct EncodedCMSampleBuffer(CFRetained<CMSampleBuffer>);

impl Deref for EncodedCMSampleBuffer {
    type Target = CMSampleBuffer;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// SAFETY: CMSampleBuffer probably wouldn't be modified after the encoding completes
unsafe impl Send for EncodedCMSampleBuffer {}

unsafe extern "C-unwind" fn on_encoded(
    encoder_out_ptr: *mut c_void,
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

    let encoded_frame_sender: &Sender<EncodedCMSampleBuffer> =
        unsafe { &*(encoder_out_ptr.cast()) };
    encoded_frame_sender
        .send(EncodedCMSampleBuffer(frame))
        .expect("receiver dropped")
}

// Runs after `encoder_in` is dropped and the thread finishes.
// Finishes the compression session so it stops using the encoded frame sender,
// then drops the encoded frame sender.
impl Drop for Encoder {
    fn drop(&mut self) {
        // Invalid time will emit all pending frames from the session.
        unsafe { self.session.complete_frames(kCMTimeInvalid) };
        unsafe { self.session.invalidate() };
        // SAFETY: pointer is only freed here
        drop(unsafe { Box::from_raw(self.encoder_out_ptr) });
    }
}
