//! Recover the labeled zoom key when Shift turns it into a different symbol.

use eframe::egui::Key;

/// Keep the native hook installed only for this application's lifetime.
#[derive(Default)]
pub(crate) struct FontShortcutMonitor {
    #[cfg(target_os = "macos")]
    _monitor: Option<macos::Monitor>,
}

impl FontShortcutMonitor {
    pub(crate) fn install() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                _monitor: macos::install_monitor(),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::default()
        }
    }
}

/// CEF's macOS key events use the hardware key position, not a Windows VK.
#[cfg(target_os = "macos")]
pub(crate) fn browser_keycode(physical: Key) -> Option<i32> {
    macos::keycode(physical).map(i32::from)
}

/// Resolve a physical key using the current macOS layout, without modifiers.
/// egui otherwise loses symbols such as `*` and `_` and substitutes a US key
/// position. Query on demand so changing the input source takes effect at once.
pub(crate) fn unshifted_zoom_key(physical: Key) -> Option<Key> {
    #[cfg(target_os = "macos")]
    {
        macos::unshifted_zoom_key(physical)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = physical;
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Key;
    use block2::RcBlock;
    use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags};
    use std::ffi::c_void;
    use std::ptr::NonNull;

    pub(super) struct Monitor(Retained<AnyObject>);

    impl Drop for Monitor {
        fn drop(&mut self) {
            // SAFETY: This is the token returned by addLocalMonitor, and App
            // owns and drops it on the native event-loop thread.
            unsafe { NSEvent::removeMonitor(&self.0) };
        }
    }

    pub(super) fn install_monitor() -> Option<Monitor> {
        let mtm = MainThreadMarker::new()?;
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
            // SAFETY: AppKit supplies a live NSEvent for this callback.
            let native = unsafe { event.as_ref() };
            let modifiers = native.modifierFlags();
            let font_chord = modifiers
                .contains(NSEventModifierFlags::Command | NSEventModifierFlags::Shift)
                && !modifiers
                    .intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Option);
            if font_chord
                && unshifted_zoom_keycode(native.keyCode()).is_some()
                && let Some(window) = native.window(mtm)
            {
                // AppKit can swallow Cmd+Shift+Minus as a key equivalent.
                // Deliver it straight to its window, then suppress the normal
                // application dispatch so the key cannot arrive twice.
                window.sendEvent(native);
                std::ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });
        // SAFETY: The callback returns either its original, still-live event or
        // null. AppKit copies the block and invokes this local monitor on main.
        unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
        }
        .map(Monitor)
    }

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        #[link_name = "kTISPropertyUnicodeKeyLayoutData"]
        static LAYOUT_DATA: *const c_void;
        fn TISCopyCurrentKeyboardLayoutInputSource() -> *const c_void;
        fn TISGetInputSourceProperty(
            source: *const c_void,
            property: *const c_void,
        ) -> *const c_void;
        fn LMGetKbdType() -> u8;
        fn UCKeyTranslate(
            layout: *const c_void,
            keycode: u16,
            action: u16,
            modifiers: u32,
            keyboard_type: u32,
            options: u32,
            dead_state: *mut u32,
            capacity: usize,
            length: *mut usize,
            characters: *mut u16,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
        fn CFRelease(value: *const c_void);
    }

    struct InputSource(*const c_void);

    impl Drop for InputSource {
        fn drop(&mut self) {
            // SAFETY: This non-null reference is owned by the matching Copy call.
            unsafe { CFRelease(self.0) };
        }
    }

    pub(super) fn unshifted_zoom_key(physical: Key) -> Option<Key> {
        unshifted_zoom_keycode(keycode(physical)?)
    }

    fn unshifted_zoom_keycode(keycode: u16) -> Option<Key> {
        // AppKit/Carbon keyboard services belong on the native event-loop thread.
        // Headless callers use a supplied keymap rather than touching the desktop.
        let _mtm = MainThreadMarker::new()?;
        // SAFETY: The copied source keeps its borrowed CFData and layout bytes
        // alive throughout translation. Null results are checked before use;
        // output pointers refer to initialized, bounded stack storage. Display
        // translation with no dead keys does not change the user's input state.
        unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if source.is_null() {
                return None;
            }
            let source = InputSource(source);
            let data = TISGetInputSourceProperty(source.0, LAYOUT_DATA);
            if data.is_null() {
                return None;
            }
            let layout = CFDataGetBytePtr(data);
            if layout.is_null() {
                return None;
            }
            let mut characters = [0_u16; 16];
            let mut length = 0;
            let mut dead_state = 0;
            let status = UCKeyTranslate(
                layout.cast(),
                keycode,
                3, // kUCKeyActionDisplay
                0, // No modifiers, including Shift.
                u32::from(LMGetKbdType()),
                1, // kUCKeyTranslateNoDeadKeysMask
                &mut dead_state,
                characters.len(),
                &mut length,
                characters.as_mut_ptr(),
            );
            if status != 0 || length != 1 {
                return None;
            }
            match characters[0] {
                0x2b => Some(Key::Plus),
                0x2d => Some(Key::Minus),
                0x3d => Some(Key::Equals),
                0x30 => Some(Key::Num0),
                _ => None,
            }
        }
    }

    /// macOS virtual keycodes describe US key positions, as does physical_key.
    pub(super) fn keycode(key: Key) -> Option<u16> {
        Some(match key {
            Key::A => 0x00,
            Key::S => 0x01,
            Key::D => 0x02,
            Key::F => 0x03,
            Key::H => 0x04,
            Key::G => 0x05,
            Key::Z => 0x06,
            Key::X => 0x07,
            Key::C => 0x08,
            Key::V => 0x09,
            Key::IntlBackslash => 0x0a,
            Key::B => 0x0b,
            Key::Q => 0x0c,
            Key::W => 0x0d,
            Key::E => 0x0e,
            Key::R => 0x0f,
            Key::Y => 0x10,
            Key::T => 0x11,
            Key::Num1 => 0x12,
            Key::Num2 => 0x13,
            Key::Num3 => 0x14,
            Key::Num4 => 0x15,
            Key::Num6 => 0x16,
            Key::Num5 => 0x17,
            Key::Equals => 0x18,
            Key::Num9 => 0x19,
            Key::Num7 => 0x1a,
            Key::Minus => 0x1b,
            Key::Num8 => 0x1c,
            Key::Num0 => 0x1d,
            Key::CloseBracket => 0x1e,
            Key::O => 0x1f,
            Key::U => 0x20,
            Key::OpenBracket => 0x21,
            Key::I => 0x22,
            Key::P => 0x23,
            Key::L => 0x25,
            Key::J => 0x26,
            Key::Quote => 0x27,
            Key::K => 0x28,
            Key::Semicolon => 0x29,
            Key::Backslash => 0x2a,
            Key::Comma => 0x2b,
            Key::Slash => 0x2c,
            Key::N => 0x2d,
            Key::M => 0x2e,
            Key::Period => 0x2f,
            Key::Backtick => 0x32,
            Key::Enter => 0x24,
            Key::Tab => 0x30,
            Key::Space => 0x31,
            Key::Backspace => 0x33,
            Key::Escape => 0x35,
            Key::F1 => 0x7a,
            Key::F2 => 0x78,
            Key::F3 => 0x63,
            Key::F4 => 0x76,
            Key::F5 => 0x60,
            Key::F6 => 0x61,
            Key::F7 => 0x62,
            Key::F8 => 0x64,
            Key::F9 => 0x65,
            Key::F10 => 0x6d,
            Key::F11 => 0x67,
            Key::F12 => 0x6f,
            Key::Insert => 0x72,
            Key::Home => 0x73,
            Key::PageUp => 0x74,
            Key::Delete => 0x75,
            Key::End => 0x77,
            Key::PageDown => 0x79,
            Key::ArrowLeft => 0x7b,
            Key::ArrowRight => 0x7c,
            Key::ArrowDown => 0x7d,
            Key::ArrowUp => 0x7e,
            Key::Plus => 0x45, // The keypad's Add key.
            _ => return None,
        })
    }
}
