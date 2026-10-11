use super::{
    Output, Wake,
    protocol::{self, Event, State, Target},
};
use cef::*;
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// What Chromium answered over DevTools, for the main loop to act on once
/// the message pump returns.
pub(super) enum Reply {
    Result {
        target: Target,
        /// From a recording's encoder page rather than the page in view.
        encoder: bool,
        id: i32,
        success: bool,
        body: Vec<u8>,
    },
    /// A frame of the page in view, for its recording.
    Frame { target: Target, params: Vec<u8> },
    /// A recording's encoder page is ready for its script.
    Loaded { target: Target },
}
pub(super) type Replies = Arc<Mutex<Vec<Reply>>>;

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
    pub(super) fn target(&self) -> Target {
        self.target
    }
    pub(super) fn picking(&self) -> bool {
        self.state.lock().is_ok_and(|s| s.picking)
    }
    pub(super) fn closed(&self) -> bool {
        self.state.lock().is_ok_and(|s| s.closed)
    }
    /// Physical pixels of one logical point in this pane.
    pub(super) fn scale(&self) -> f32 {
        self.geometry.lock().map_or(1.0, |g| g.2)
    }
    /// Chromium here paints offscreen pages at one pixel per view unit
    /// whatever scale the screen reports. The view is therefore sized in
    /// physical pixels and the page zoomed by the display's scale, which gives
    /// it the same layout and pixel ratio as a window on that display.
    fn zoom(&self, browser: Option<&mut Browser>) {
        let level = f64::from(self.scale()).ln() / 1.2f64.ln();
        if let Some(host) = browser.and_then(|b| b.host())
            && (host.zoom_level() - level).abs() > 1e-3
        {
            host.set_zoom_level(level);
        }
    }
    fn icon(&self, width: u32, height: u32, pixels: Vec<u8>) {
        self.output.put(
            self.target,
            6,
            Event::Icon {
                target: self.target,
                width,
                height,
            },
            pixels,
        );
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
            // The view is in physical pixels; the pane places the caret in points.
            let scale = self.data.scale();
            let points = |v: i32| (v as f32 / scale) as i32;
            self.data.update(|s| s.caret = bounds.and_then(|b| b.last()).map(|r| [points(r.x.saturating_add(r.width)), points(r.y), 1, points(r.height).max(1)]));
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
        fn on_address_change(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.is_some_and(|f| f.is_main() == 1) && let Some(url) = url {
                let url = url.to_string();
                // Chromium keeps zoom by site: a site new to this profile
                // would otherwise open unzoomed.
                self.data.zoom(browser);
                let mut moved = false;
                self.data.update(|s| { moved = site(&s.url) != site(&url); s.url = url; });
                // Another site's icon does not stand for this one.
                if moved { self.data.icon(0, 0, Vec::new()); }
            }
        }
        fn on_cursor_change(&self, _browser: Option<&mut Browser>, _cursor: std::os::raw::c_ulong, type_: CursorType,
            _custom: Option<&CursorInfo>) -> i32 {
            let cursor = *type_.as_ref() as u32;
            self.data.update(|s| s.cursor = cursor);
            1
        }
        fn on_tooltip(&self, _browser: Option<&mut Browser>, text: Option<&mut CefString>) -> i32 {
            let text = text.map(|text| said(&text.to_string())).filter(|text| !text.is_empty());
            self.data.update(|s| s.tooltip = text);
            1
        }
        fn on_status_message(&self, _browser: Option<&mut Browser>, value: Option<&CefString>) {
            let link = value.map(|value| said(&value.to_string())).filter(|link| !link.is_empty());
            self.data.update(|s| s.link = link);
        }
        fn on_favicon_urlchange(&self, browser: Option<&mut Browser>, icon_urls: Option<&mut CefStringList>) {
            let url = icon_urls.and_then(|urls| std::mem::take(urls).into_iter().find(|url| super::valid_url(url)));
            match (url, browser.and_then(|b| b.host())) {
                (Some(url), Some(host)) => host.download_image(Some(&url.as_str().into()), 1, protocol::MAX_ICON, 0,
                    Some(&mut Icon::new(self.data.clone()))),
                _ => self.data.icon(0, 0, Vec::new()),
            }
        }
    }
}

/// A page's words for the pane to show, bounded and on one line.
fn said(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(300)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The scheme, host and port of an address.
fn site(url: &str) -> &str {
    let host = url.find("://").map_or(0, |at| at + 3);
    &url[..url[host..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |end| host + end)]
}

wrap_download_image_callback! {
    struct Icon {
        data: Arc<Data>,
    }
    impl DownloadImageCallback {
        fn on_download_image_finished(&self, _url: Option<&CefString>, _status: i32, image: Option<&mut Image>) {
            let (mut width, mut height) = (0, 0);
            let pixels = image.and_then(|image| image.as_bitmap(1.0, ColorType::RGBA_8888,
                AlphaType::PREMULTIPLIED, Some(&mut width), Some(&mut height)));
            let (width, height) = (width.max(0) as u32, height.max(0) as u32);
            let len = width as usize * height as usize * 4;
            let mut bytes = vec![0; len];
            // A page's icon is a page's data: only a complete bitmap of a
            // bounded size is read, and only a small one is passed on.
            if let Some(pixels) = pixels
                && (1..=MAX_SOURCE).contains(&width) && (1..=MAX_SOURCE).contains(&height)
                && pixels.size() == len && pixels.data(Some(&mut bytes), 0) == len
            {
                let (width, height, bytes) = shrink(width, height, bytes);
                self.data.icon(width, height, bytes);
            } else {
                self.data.icon(0, 0, Vec::new());
            }
        }
    }
}

/// The largest icon read from Chromium, which may answer with one larger
/// than it was asked for.
const MAX_SOURCE: u32 = 512;

/// An icon no larger than a tab can use, each pixel the mean of those it
/// stands for. Alpha is premultiplied, so the mean needs no weighting.
fn shrink(width: u32, height: u32, pixels: Vec<u8>) -> (u32, u32, Vec<u8>) {
    let by = width.max(height).div_ceil(protocol::MAX_ICON);
    if by <= 1 {
        return (width, height, pixels);
    }
    let (small_width, small_height) = (width.div_ceil(by), height.div_ceil(by));
    let mut small = Vec::with_capacity((small_width * small_height * 4) as usize);
    for y in 0..small_height {
        for x in 0..small_width {
            let (mut sum, mut count) = ([0u32; 4], 0u32);
            for row in y * by..((y + 1) * by).min(height) {
                for column in x * by..((x + 1) * by).min(width) {
                    let at = ((row * width + column) * 4) as usize;
                    for (total, value) in sum.iter_mut().zip(&pixels[at..at + 4]) {
                        *total += u32::from(*value);
                    }
                    count += 1;
                }
            }
            small.extend(sum.map(|total| (total / count.max(1)) as u8));
        }
    }
    (small_width, small_height, small)
}

wrap_load_handler! {
    struct Load {
        data: Arc<Data>,
    }
    impl LoadHandler {
        fn on_load_start(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition: TransitionType) {
            if frame.is_some_and(|f| f.is_main() == 1) { self.data.zoom(browser); }
        }
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

wrap_dev_tools_message_observer! {
    pub(super) struct Observer {
        target: Target,
        encoder: bool,
        replies: Replies,
    }
    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self, _browser: Option<&mut Browser>, id: i32, success: i32, result: Option<&[u8]>) {
            if let Ok(mut replies) = self.replies.lock() {
                replies.push(Reply::Result { target: self.target, encoder: self.encoder, id, success: success != 0,
                    body: result.unwrap_or_default().to_vec() });
            }
        }
        fn on_dev_tools_event(&self, _browser: Option<&mut Browser>, method: Option<&CefString>, params: Option<&[u8]>) {
            if !self.encoder && method.is_some_and(|m| m.to_string() == "Page.screencastFrame")
                && let (Some(params), Ok(mut replies)) = (params, self.replies.lock()) {
                replies.push(Reply::Frame { target: self.target, params: params.to_vec() });
            }
        }
    }
}

// Chromium opens a saved profile's files after the call that asks for it.
wrap_request_context_handler! {
    pub(super) struct Opened {
        ready: Arc<AtomicBool>,
    }
    impl RequestContextHandler {
        fn on_request_context_initialized(&self, _context: Option<&mut RequestContext>) {
            self.ready.store(true, Ordering::Release);
        }
    }
}

wrap_set_cookie_callback! {
    pub(super) struct Counted {
        tally: Arc<Mutex<super::tools::Tally>>,
        output: Arc<Output>,
    }
    impl SetCookieCallback {
        fn on_complete(&self, success: i32) {
            if let Ok(mut tally) = self.tally.lock() { tally.settle(&self.output, success != 0); }
        }
    }
}

// A recording's encoder is a blank page of Neptune's own that is never shown.
wrap_render_handler! {
    struct Unseen;
    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect { rect.width = 16; rect.height = 16; }
        }
    }
}
wrap_load_handler! {
    struct EncoderLoad {
        target: Target,
        replies: Replies,
    }
    impl LoadHandler {
        fn on_loading_state_change(&self, _browser: Option<&mut Browser>, loading: i32, _back: i32, _forward: i32) {
            if loading == 0 && let Ok(mut replies) = self.replies.lock() {
                replies.push(Reply::Loaded { target: self.target });
            }
        }
    }
}
wrap_life_span_handler! {
    struct EncoderLife {
        closed: Arc<AtomicBool>,
    }
    impl LifeSpanHandler {
        fn on_before_close(&self, _browser: Option<&mut Browser>) { self.closed.store(true, Ordering::Release); }
        fn on_before_popup(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _id: i32,
            _url: Option<&CefString>, _name: Option<&CefString>, _disposition: WindowOpenDisposition,
            _gesture: i32, _features: Option<&PopupFeatures>, _info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>,
            _extra: Option<&mut Option<DictionaryValue>>, _no_access: Option<&mut i32>) -> i32 { 1 }
    }
}
wrap_client! {
    pub(super) struct EncoderClient {
        target: Target,
        replies: Replies,
        closed: Arc<AtomicBool>,
    }
    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> { Some(Unseen::new()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(EncoderLoad::new(self.target, self.replies.clone())) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(EncoderLife::new(self.closed.clone())) }
    }
}

// DevTools is a native CEF window with its own client. Its internal URLs,
// frames and title must never replace the preview's state or pixel snapshot.
wrap_client! {
    pub(super) struct ToolsClient;
    impl Client {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_oversized_icon_is_shrunk_to_what_a_tab_can_use() {
        // A small one is passed on as it is.
        assert_eq!(shrink(2, 1, vec![1; 8]), (2, 1, vec![1; 8]));
        let mut pixels = vec![0; 128 * 128 * 4];
        // Three of the top-left block of four are opaque grey.
        for at in [0, 1, 128] {
            pixels[at * 4..at * 4 + 4].copy_from_slice(&[200, 200, 200, 200]);
        }
        let (width, height, small) = shrink(128, 128, pixels);
        assert_eq!((width, height, small.len()), (64, 64, 64 * 64 * 4));
        assert_eq!(small[..8], [150, 150, 150, 150, 0, 0, 0, 0]);
        // A side that does not divide keeps its last, narrower block.
        let (width, height, small) = shrink(130, 65, vec![8; 130 * 65 * 4]);
        assert_eq!((width, height), (44, 22));
        assert!(small.iter().all(|value| *value == 8));
    }
}
