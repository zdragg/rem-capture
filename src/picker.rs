use std::sync::mpsc::{Receiver, Sender};

use objc2::{
    AnyThread, DefinedClass, Message, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCContentSharingPicker, SCContentSharingPickerConfiguration,
    SCContentSharingPickerMode, SCContentSharingPickerObserver, SCStream,
};

/// Either picks a single window or a single display if selected. Returns None if prompt was cancelled by user.
pub fn pick() -> Option<Retained<SCContentFilter>> {
    // https://github.com/JXA-Cookbook/JXA-Cookbook/issues/27#issuecomment-277517668
    // Initialize Core Graphics. Otherwise, selecting a specific window will crash.
    let _ = objc2_core_graphics::CGMainDisplayID();

    let observer = Observer::new();

    let config = unsafe { SCContentSharingPickerConfiguration::new() };
    unsafe {
        config.setAllowedPickerModes(
            SCContentSharingPickerMode::SingleDisplay | SCContentSharingPickerMode::SingleWindow,
        );
    }

    let picker = unsafe { SCContentSharingPicker::sharedPicker() };
    unsafe {
        picker.setDefaultConfiguration(&config);
        picker.setActive(true);
        picker.addObserver(ProtocolObject::from_ref(&*observer));
        picker.present();
    }
    let filter = observer.ivars().rx.recv().expect("sender dropped");

    unsafe { picker.removeObserver(ProtocolObject::from_ref(&*observer)) };

    filter.map(|f| f.0)
}

struct SendSCContentFilter(Retained<SCContentFilter>);

// SAFETY: ScreenCaptureKit probably creates a new SCContentFilter on each did_update(),
// so it probably doesn't hold a reference to a filter that it already provided.
unsafe impl Send for SendSCContentFilter {}

struct ObserverIVars {
    tx: Sender<Option<SendSCContentFilter>>,
    rx: Receiver<Option<SendSCContentFilter>>,
}

impl ObserverIVars {
    fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { tx, rx }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = ObserverIVars]

    struct Observer;

    unsafe impl NSObjectProtocol for Observer {}

    unsafe impl SCContentSharingPickerObserver for Observer {
        /// Tells the observer that a sharing picker canceled selection for a stream.
        #[unsafe(method(contentSharingPicker:didCancelForStream:))]
        fn did_cancel(&self, picker: &SCContentSharingPicker, stream: Option<&SCStream>) {
            self.ivars().tx.send(None).expect("receiver dropped");
        }

        /// Tells the observer that a sharing picker updated the content filter for a stream.
        #[unsafe(method(contentSharingPicker:didUpdateWithFilter:forStream:))]
        fn did_update(
            &self,
            picker: &SCContentSharingPicker,
            filter: &SCContentFilter,
            stream: Option<&SCStream>,
        ) {
            // Print filter address. If it returns different addresses on each did_update(),
            // then ScreenCaptureKit makes a new SCContentFilter every time.
            // println!("{:?}", filter as *const _);
            let filter = Some(SendSCContentFilter(filter.retain()));
            self.ivars().tx.send(filter).expect("receiver dropped");
        }

        /// Tells the observer that a sharing picker was unable to start.
        #[unsafe(method(contentSharingPickerStartDidFailWithError:))]
        fn did_fail(&self, error: &NSError) {
            panic!("failed to start picker: {}", error)
        }
    }
);

impl Observer {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(ObserverIVars::new());
        unsafe { msg_send![super(this), init] }
    }
}
