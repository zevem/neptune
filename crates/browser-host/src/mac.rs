//! The helper owns a separate Cocoa application conforming to CEF's protocol.
use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, define_class, msg_send,
    runtime::{Bool, NSObjectProtocol},
};
use objc2_app_kit::{NSApp, NSApplication, NSEvent};
use std::cell::Cell;

#[derive(Default)]
pub struct Ivars {
    sending: Cell<Bool>,
}
define_class! {
    #[unsafe(super(NSApplication))]
    #[ivars = Ivars]
    pub struct BrowserApplication;
    impl BrowserApplication {
        #[unsafe(method(sendEvent:))]
        unsafe fn send_event(&self, event: &NSEvent) {
            let previous = self.ivars().sending.replace(Bool::YES);
            let _: () = unsafe { msg_send![super(self), sendEvent: event] };
            self.ivars().sending.set(previous);
        }
    }
    unsafe impl CrAppProtocol for BrowserApplication {
        #[unsafe(method(isHandlingSendEvent))]
        unsafe fn is_handling_send_event(&self) -> Bool { self.ivars().sending.get() }
    }
    unsafe impl CrAppControlProtocol for BrowserApplication {
        #[unsafe(method(setHandlingSendEvent:))]
        unsafe fn set_handling_send_event(&self, value: Bool) { self.ivars().sending.set(value); }
    }
    unsafe impl CefAppProtocol for BrowserApplication {}
}
pub fn initialize() {
    let _: objc2::rc::Retained<BrowserApplication> =
        unsafe { msg_send![BrowserApplication::class(), sharedApplication] };
    let marker = MainThreadMarker::new().expect("Browser application requires the main thread");
    assert!(NSApp(marker).isKindOfClass(BrowserApplication::class()));
}
