use super::{
    Output, Wake,
    protocol::{self, Event, State, Target},
};
use cef::*;
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

pub(super) struct Data {
    target: Target,
    output: Arc<Output>,
    state: Mutex<State>,
    geometry: Mutex<(u32, u32, f32)>,
    popup: Mutex<Rect>,
    painted_size: Mutex<[Option<[u32; 2]>; 2]>,
    pacing: Mutex<super::pacing::Pacing>,
}
impl Data {
    pub(super) fn new(target: Target, output: Arc<Output>) -> Self {
        Self {
            target,
            output,
            state: Mutex::new(State::default()),
            geometry: Mutex::new((800, 600, 1.0)),
            popup: Mutex::new(Rect::default()),
            painted_size: Mutex::new([None, None]),
            pacing: Mutex::new(super::pacing::Pacing::default()),
        }
    }
    pub(super) fn update(&self, edit: impl FnOnce(&mut State)) {
        if let Ok(mut state) = self.state.lock() {
            edit(&mut state);
            state.title = state.title.chars().take(512).collect();
            if state.url.len() > protocol::MAX_URL {
                state.url.clear();
            }
            self.output.put(
                self.target,
                0,
                Event::State {
                    target: self.target,
                    state: state.clone(),
                },
                Vec::new(),
            );
        }
    }
    pub(super) fn closed(&self) -> bool {
        self.state.lock().is_ok_and(|s| s.closed)
    }
    pub(super) fn resize(&self, width: u32, height: u32, scale: f32) {
        if let Ok(mut geometry) = self.geometry.lock() {
            *geometry = (width, height, scale);
        }
    }
    pub(super) fn invalidate(&self) {
        if let Ok(mut sizes) = self.painted_size.lock() {
            *sizes = [None, None];
        }
    }
    pub(super) fn frame_rate(&self, rate: u32) {
        if let Ok(mut pacing) = self.pacing.lock() {
            pacing.frame_rate(rate);
        }
    }
    pub(super) fn visible(&self, visible: bool) {
        if let Ok(mut pacing) = self.pacing.lock() {
            pacing.visible(visible);
        }
    }
    pub(super) fn pump_interval(&self, now: Instant) -> Duration {
        self.pacing
            .lock()
            .map_or(Duration::from_millis(33), |pacing| pacing.interval(now))
    }
}

wrap_browser_process_handler! {
    struct Pump {
        deadline: Arc<(Mutex<Wake>, Condvar)>,
    }
    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            let (lock, wake) = &*self.deadline;
            if let Ok(mut deadline) = lock.lock() {
                let at = Instant::now() + Duration::from_millis(delay_ms.clamp(0, 60_000) as u64);
                deadline.next = Some(deadline.next.map_or(at, |next| next.min(at)));
            }
            wake.notify_one();
        }
    }
}
wrap_app! {
    pub(super) struct HostApp {
        deadline: Arc<(Mutex<Wake>, Condvar)>,
    }
    impl App {
        fn on_before_command_line_processing(&self, _process: Option<&CefString>, command: Option<&mut CommandLine>) {
            #[cfg(target_os = "linux")]
            if std::env::var_os("WAYLAND_DISPLAY").is_none()
                && std::env::var_os("DISPLAY").is_some()
                && let Some(command) = command {
                // An X11 Neptune launch can still inherit a Wayland session
                // type. Keep CEF's native tools on the selected X11 display.
                command.append_switch_with_value(Some(&"ozone-platform".into()), Some(&"x11".into()));
            }
            #[cfg(not(target_os = "linux"))]
            let _ = command;
        }
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(Pump::new(self.deadline.clone()))
        }
    }
}

wrap_render_handler! {
    struct Render {
        data: Arc<Data>,
    }
    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let (Some(rect), Ok(size)) = (rect, self.data.geometry.lock()) {
                rect.width = size.0 as i32;
                rect.height = size.1 as i32;
            }
        }
        fn screen_info(&self, _browser: Option<&mut Browser>, info: Option<&mut ScreenInfo>) -> i32 {
            if let (Some(info), Ok(size)) = (info, self.data.geometry.lock()) {
                info.device_scale_factor = size.2;
                info.rect = Rect { x: 0, y: 0, width: size.0 as i32, height: size.1 as i32 };
                info.available_rect = info.rect.clone();
                return 1;
            }
            0
        }
        fn on_paint(&self, _browser: Option<&mut Browser>, type_: PaintElementType,
            dirty_rects: Option<&[Rect]>, buffer: *const u8, width: i32, height: i32) {
            let Some(len) = protocol::frame_len(width.max(0) as u32, height.max(0) as u32) else { return };
            if buffer.is_null() { return }
            if let Ok(mut pacing) = self.data.pacing.lock() { pacing.paint(Instant::now()); }
            let popup = type_ == PaintElementType::POPUP;
            let size = [width as u32, height as u32];
            let mut damage = dirty_rects.into_iter().flatten().filter_map(|rect| {
                // CEF damage is in physical pixels. Clip signed rectangles
                // before converting; no unchecked size or offset reaches IPC.
                let x = rect.x.clamp(0, width) as u32;
                let y = rect.y.clamp(0, height) as u32;
                let end_x = rect.x.saturating_add(rect.width).clamp(0, width) as u32;
                let end_y = rect.y.saturating_add(rect.height).clamp(0, height) as u32;
                let region = protocol::Damage { x, y, width: end_x.saturating_sub(x), height: end_y.saturating_sub(y) };
                region.valid(size[0], size[1]).then_some(region)
            }).reduce(protocol::Damage::union).unwrap_or(protocol::Damage::full(size[0], size[1]));
            if let Ok(mut sizes) = self.data.painted_size.lock() {
                let previous = sizes[usize::from(popup)].replace(size);
                if previous != Some(size) { damage = protocol::Damage::full(size[0], size[1]); }
            } else { return }
            // SAFETY: CEF owns the complete BGRA buffer for this callback.
            // Output copies only the required region before we return.
            let pixels = unsafe { std::slice::from_raw_parts(buffer, len) };
            let rect = self.data.popup.lock().map(|r| r.clone()).unwrap_or_default();
            self.data.output.paint(self.data.target, popup, size, [rect.x, rect.y], damage, pixels);
        }
        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            if let (Some(rect), Ok(mut popup)) = (rect, self.data.popup.lock()) { *popup = rect.clone(); }
        }
        fn on_ime_composition_range_changed(&self, _browser: Option<&mut Browser>, _range: Option<&Range>, bounds: Option<&[Rect]>) {
            self.data.update(|s| s.caret = bounds.and_then(|b| b.last()).map(|r| [r.x.saturating_add(r.width), r.y, 1, r.height.max(1)]));
        }
        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: i32) {
            if show == 0 {
                if let Ok(mut sizes) = self.data.painted_size.lock() { sizes[1] = None; }
                self.data.output.put(self.data.target, 2, Event::Frame { target: self.data.target,
                    width: 0, height: 0, popup: true, x: 0, y: 0, damage: None }, Vec::new());
            }
        }
    }
}

wrap_display_handler! {
    struct Display {
        data: Arc<Data>,
    }
    impl DisplayHandler {
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            if let Some(title) = title { self.data.update(|s| s.title = title.to_string()); }
        }
        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.is_some_and(|f| f.is_main() == 1) && let Some(url) = url {
                self.data.update(|s| s.url = url.to_string());
            }
        }
    }
}

wrap_load_handler! {
    struct Load {
        data: Arc<Data>,
    }
    impl LoadHandler {
        fn on_loading_state_change(&self, _browser: Option<&mut Browser>, loading: i32, back: i32, forward: i32) {
            self.data.update(|s| { s.loading = loading != 0; s.back = back != 0; s.forward = forward != 0; });
        }
        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, code: Errorcode,
            _text: Option<&CefString>, _url: Option<&CefString>) {
            if frame.is_some_and(|f| f.is_main() == 1) && code != Errorcode::ABORTED {
                // No page-provided error text or URL enters process diagnostics.
                self.data.update(|s| s.error = Some("Could not load this page. Check the address and that your server is running, then reload.".into()));
            }
        }
    }
}

wrap_life_span_handler! {
    struct Life {
        data: Arc<Data>,
    }
    impl LifeSpanHandler {
        fn on_before_dev_tools_popup(&self, _browser: Option<&mut Browser>,
            _info: Option<&mut WindowInfo>, _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>, _extra: Option<&mut Option<DictionaryValue>>,
            use_default_window: Option<&mut i32>) {
            // This offscreen client does not implement CEF Views. Let Chromium
            // own the native DevTools window instead of awaiting a Views host.
            if let Some(default) = use_default_window { *default = 1; }
        }
        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            self.data.update(|s| { s.ready = true; s.url = "about:blank".into(); });
        }
        fn on_before_close(&self, _browser: Option<&mut Browser>) { self.data.update(|s| s.closed = true); }
        fn on_before_popup(&self, browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _id: i32,
            url: Option<&CefString>, _name: Option<&CefString>, _disposition: WindowOpenDisposition,
            gesture: i32, _features: Option<&PopupFeatures>, _info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>,
            _extra: Option<&mut Option<DictionaryValue>>, _no_access: Option<&mut i32>) -> i32 {
            // User-initiated links stay in this preview. Unsolicited windows
            // cannot allocate browsers or launch another application.
            if gesture != 0 && let Some(url) = url && super::valid_url(&url.to_string())
                && let Some(frame) = browser.and_then(|b| b.main_frame()) { frame.load_url(Some(url)); }
            1
        }
    }
}

wrap_request_handler! {
    struct Request {
        data: Arc<Data>,
    }
    impl RequestHandler {
        fn on_before_browse(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            request: Option<&mut cef::Request>, _gesture: i32, _redirect: i32) -> i32 {
            if frame.is_some_and(|f| f.is_main() == 1) && let Some(request) = request {
                let allowed = super::valid_url(&CefString::from(&request.url()).to_string());
                if allowed { self.data.update(|s| { s.error = None; s.caret = None; }); }
                return i32::from(!allowed);
            }
            0
        }
        fn on_render_process_terminated(&self, _browser: Option<&mut Browser>, _status: TerminationStatus,
            _code: i32, _error: Option<&CefString>) {
            self.data.update(|s| { s.loading = false; s.error = Some("The page stopped unexpectedly. Reload to restart it.".into()); });
        }
    }
}

wrap_client! {
    pub(super) struct PreviewClient {
        data: Arc<Data>,
    }
    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> { Some(Render::new(self.data.clone())) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(Display::new(self.data.clone())) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(Load::new(self.data.clone())) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(Life::new(self.data.clone())) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(Request::new(self.data.clone())) }
    }
}

// DevTools is a native CEF window with its own client. Its internal URLs,
// frames and title must never replace the preview's state or pixel snapshot.
wrap_client! {
    pub(super) struct ToolsClient;
    impl Client {}
}
