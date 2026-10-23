// lispm-remote: the display's view, mounted in a page.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The view around the model, as a page mounts it ([`DisplayView`]): the
//! canvas, the keyboard (through a hidden text area, so phones show
//! theirs and input methods work), the pointer and touch, the window list
//! and tabs, and the image pool (in memory, and in the browser's cache
//! across visits).  The view builds its elements inside the one the page
//! gives it, and leaves the connection to the page: it sends through a
//! function and is handed what arrives.
//!
//! ```js
//! const view = new DisplayView(element, bytes => channel.send(bytes));
//! channel.onopen = () => view.connected();       // sends Hello
//! channel.onmessage = bytes => view.receive(bytes);
//! channel.onclose = () => view.disconnected();
//! view.close();                                  // sends Bye
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;

use js_sys::Uint8Array;
use lispm_remote_display::Button;
use lispm_remote_display::CellPixels;
use lispm_remote_display::ClientMessage;
use lispm_remote_display::Input;
use lispm_remote_display::Key;
use lispm_remote_display::KeyCode;
use lispm_remote_display::Mods;
use lispm_remote_display::Named;
use lispm_remote_display::Pointer;
use lispm_remote_display::PointerKind;
use lispm_remote_display::ResourceId;
use lispm_remote_display::Rgb;
use lispm_remote_display::ServerMessage;
use lispm_remote_display::Size;
use lispm_remote_display::SurfaceId;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::CanvasRenderingContext2d;
use web_sys::Document;
use web_sys::HtmlCanvasElement;
use web_sys::HtmlElement;
use web_sys::HtmlTextAreaElement;
use web_sys::ImageBitmap;

use crate::keymap;
use crate::keymap::KeyFields;
use crate::model::Effect;
use crate::model::Model;
use crate::render;
use crate::render::Metrics;
use crate::render::Theme;

/// The colours the display tells Emacs the client has.
const THEME: Theme = Theme {
    foreground: Rgb::new(0, 0, 0),
    background: Rgb::new(255, 255, 255),
};
/// The browser cache the image pool keeps across visits.
const POOL: &str = "lispm-remote-pool";

type Shared = Rc<RefCell<App>>;

/// A finger on the screen: where it went down, and how far it scrolled.
struct Touch {
    x: f64,
    y: f64,
    scrolled: f64,
    dragged: bool,
}

struct App {
    window: web_sys::Window,
    document: Document,
    canvas: HtmlCanvasElement,
    context: CanvasRenderingContext2d,
    keys: HtmlTextAreaElement,
    tabs: HtmlElement,
    list: HtmlElement,
    panel: HtmlElement,
    status: HtmlElement,
    add: HtmlElement,
    sender: js_sys::Function,
    connected: bool,
    model: Model,
    metrics: Metrics,
    images: HashMap<ResourceId, ImageBitmap>,
    sought: HashSet<ResourceId>,
    drawing: bool,
    composing: bool,
    touch: Option<Touch>,
    wheel: f64,
}

/// The view's look, added to the page once.
const STYLE: &str = "\
.lispm-display{position:relative;display:flex;flex-direction:column;width:100%;height:100%;\
background:#fff;color:#111;font:13px -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif}\
.lispm-display-bar{display:flex;align-items:center;gap:4px;padding:4px 6px;background:#e8e8e8;\
border-bottom:1px solid #b8b8b8;min-height:34px}\
.lispm-display-tabs{display:flex;gap:4px;overflow-x:auto;flex:1;scrollbar-width:none}\
.lispm-display-tabs button,.lispm-display-add{font:inherit;color:inherit;border:1px solid #b8b8b8;\
border-radius:5px;background:transparent;padding:3px 8px;white-space:nowrap;cursor:pointer}\
.lispm-display-tabs button.active{background:#fff}\
.lispm-display-tabs .close{margin-left:6px;color:#666}\
.lispm-display-add{font-size:16px;line-height:1;padding:3px 10px}\
.lispm-display-status{color:#666;white-space:nowrap}\
.lispm-display-windows{position:absolute;top:40px;right:6px;max-height:60%;overflow-y:auto;\
min-width:220px;background:#fff;border:1px solid #b8b8b8;border-radius:6px;\
box-shadow:0 4px 16px rgba(0,0,0,.15);z-index:2}\
.lispm-display-windows[hidden]{display:none}\
.lispm-display-windows ul{list-style:none;margin:0;padding:4px}\
.lispm-display-windows button{display:block;width:100%;text-align:left;font:inherit;color:inherit;\
border:0;border-radius:4px;background:transparent;padding:6px 8px;cursor:pointer}\
.lispm-display-windows button:hover{background:#e8e8e8}\
.lispm-display-screen{position:relative;flex:1;min-height:0}\
.lispm-display-screen canvas{position:absolute;inset:0;width:100%;height:100%;touch-action:none}\
.lispm-display-screen canvas.bell{filter:invert(1)}\
.lispm-display-keys{position:absolute;left:0;bottom:0;width:1px;height:1px;opacity:0;border:0;\
padding:0;resize:none}";

/// LispM's windows, redrawn in an element of a page.
#[wasm_bindgen]
pub struct DisplayView {
    app: Shared,
}

#[wasm_bindgen]
impl DisplayView {
    /// A view inside `root`, sending each message to LispM by calling
    /// `send` with its bytes (a `Uint8Array`).
    ///
    /// # Errors
    ///
    /// When the browser has no canvas.
    #[wasm_bindgen(constructor)]
    pub fn new(root: &HtmlElement, send: js_sys::Function) -> Result<Self, JsValue> {
        let app = Rc::new(RefCell::new(App::new(root, send)?));
        listen(&app)?;
        Ok(Self { app })
    }

    /// The channel to LispM opened: start over, and say hello.
    pub fn connected(&self) {
        {
            let mut state = self.app.borrow_mut();
            state.connected = true;
            state.model.reset();
            state.sought.clear();
            state.set_status("");
            state.send(&ClientMessage::Hello);
        }
        self.app.borrow().render_tabs();
        schedule_draw(&self.app);
    }

    /// The channel to LispM closed.
    pub fn disconnected(&self) {
        let mut state = self.app.borrow_mut();
        state.connected = false;
        state.set_status("Not connected");
    }

    /// Takes in `bytes`, a message LispM sent.
    pub fn receive(&self, bytes: &[u8]) {
        if let Ok(message) = ServerMessage::decode(bytes) {
            let done = self.app.borrow_mut().model.receive(message);
            effects(&self.app, done);
        }
    }

    /// The view is shown again, or its size changed: fit the window
    /// shown to it.
    pub fn fit(&self) {
        self.app.borrow().fit();
        schedule_draw(&self.app);
    }

    /// The view is done: tell LispM to send nothing more.
    pub fn close(&self) {
        let mut state = self.app.borrow_mut();
        state.send(&ClientMessage::Bye);
        state.connected = false;
    }
}

/// Makes the element `tag` with class `class` inside `parent`.
fn child<T: JsCast>(
    document: &Document,
    parent: &web_sys::Element,
    tag: &str,
    class: &str,
) -> Result<T, JsValue> {
    let element = document.create_element(tag)?;
    if !class.is_empty() {
        element.set_class_name(class);
    }
    parent.append_child(&element)?;
    element
        .dyn_into::<T>()
        .map_err(|_| JsValue::from_str("an element of the wrong kind"))
}

/// Adds the view's look to the page, once.
fn add_style(document: &Document) -> Result<(), JsValue> {
    if document.get_element_by_id("lispm-display-style").is_none() {
        let style = document.create_element("style")?;
        style.set_id("lispm-display-style");
        style.set_text_content(Some(STYLE));
        if let Some(head) = document.head() {
            head.append_child(&style)?;
        }
    }
    Ok(())
}

impl App {
    fn new(root: &HtmlElement, sender: js_sys::Function) -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or("no window")?;
        let document = window.document().ok_or("no document")?;
        add_style(&document)?;
        let view: web_sys::Element = child(&document, root, "div", "lispm-display")?;
        let bar: web_sys::Element = child(&document, &view, "div", "lispm-display-bar")?;
        let tabs: HtmlElement = child(&document, &bar, "nav", "lispm-display-tabs")?;
        let add: HtmlElement = child(&document, &bar, "button", "lispm-display-add")?;
        add.set_text_content(Some("+"));
        add.set_title("Open a window");
        let status: HtmlElement = child(&document, &bar, "span", "lispm-display-status")?;
        let panel: HtmlElement = child(&document, &view, "div", "lispm-display-windows")?;
        panel.set_hidden(true);
        let list: HtmlElement = child(&document, &panel, "ul", "")?;
        let screen: web_sys::Element = child(&document, &view, "div", "lispm-display-screen")?;
        let canvas: HtmlCanvasElement = child(&document, &screen, "canvas", "")?;
        let keys: HtmlTextAreaElement =
            child(&document, &screen, "textarea", "lispm-display-keys")?;
        for (name, value) in [
            ("aria-label", "Keyboard"),
            ("autocapitalize", "off"),
            ("autocomplete", "off"),
            ("autocorrect", "off"),
            ("spellcheck", "false"),
        ] {
            keys.set_attribute(name, value)?;
        }
        let context: CanvasRenderingContext2d = canvas
            .get_context("2d")?
            .ok_or("no 2d context")?
            .dyn_into()?;
        let size = if window.inner_width()?.as_f64().unwrap_or(800.0) < 600.0 {
            13.0
        } else {
            15.0
        };
        context.set_font(&render::font(size, false, false));
        let width = context.measure_text("M")?.width();
        let metrics = Metrics {
            size,
            cell_width: width,
            cell_height: (size * 1.3).ceil(),
            ratio: window.device_pixel_ratio(),
        };
        Ok(Self {
            keys,
            tabs,
            list,
            panel,
            status,
            add,
            sender,
            connected: false,
            window,
            document,
            canvas,
            context,
            model: Model::default(),
            metrics,
            images: HashMap::new(),
            sought: HashSet::new(),
            drawing: false,
            composing: false,
            touch: None,
            wheel: 0.0,
        })
    }

    fn send(&self, message: &ClientMessage) {
        if self.connected {
            let bytes = Uint8Array::from(message.encode().as_slice());
            let _ = self.sender.call1(&JsValue::NULL, &bytes);
        }
    }

    fn input(&self, input: Input) {
        if let Some(surface) = self.model.active {
            self.send(&ClientMessage::Input { surface, input });
        }
    }

    /// The grid that fits the canvas, and a cell in device pixels.
    fn grid(&self) -> Option<(Size, CellPixels)> {
        let rect = self.canvas.get_bounding_client_rect();
        let fit = |pixels: f64, cell: f64| {
            let cells = (pixels / cell).floor().clamp(1.0, 1000.0);
            // Saturating: the value is in 1..=1000 already.
            #[allow(clippy::as_conversions)]
            let cells = cells as u16;
            cells
        };
        let device = |pixels: f64| {
            #[allow(clippy::as_conversions)]
            let pixels = (pixels * self.metrics.ratio).round().clamp(1.0, 4096.0) as u16;
            pixels
        };
        let size = Size::new(
            fit(rect.width(), self.metrics.cell_width),
            fit(rect.height(), self.metrics.cell_height),
        )
        .ok()?;
        let cell = CellPixels {
            width: device(self.metrics.cell_width),
            height: device(self.metrics.cell_height),
        };
        Some((size, cell))
    }

    fn set_status(&self, text: &str) {
        self.status.set_text_content(Some(text));
    }

    fn draw(&mut self) {
        self.drawing = false;
        let rect = self.canvas.get_bounding_client_rect();
        let ratio = self.metrics.ratio;
        #[allow(clippy::as_conversions)]
        let (width, height) = (
            (rect.width() * ratio).round() as u32,
            (rect.height() * ratio).round() as u32,
        );
        if self.canvas.width() != width || self.canvas.height() != height {
            self.canvas.set_width(width);
            self.canvas.set_height(height);
        }
        let _ = self.context.set_transform(ratio, 0.0, 0.0, ratio, 0.0, 0.0);
        let focused = self
            .document
            .active_element()
            .is_some_and(|active| active == *self.keys.unchecked_ref::<web_sys::Element>());
        if let Some((_, copy)) = self.model.shown() {
            render::draw(
                &self.context,
                &self.metrics,
                copy.screen(),
                &self.images,
                THEME,
                focused,
            );
        } else {
            self.context.set_fill_style_str("rgb(255,255,255)");
            self.context
                .fill_rect(0.0, 0.0, rect.width(), rect.height());
        }
    }

    /// The images the shown screen places that the pool lacks.
    fn missing(&mut self) -> Vec<ResourceId> {
        let Some((_, copy)) = self.model.shown() else {
            return Vec::new();
        };
        let wanted: Vec<ResourceId> = copy
            .screen()
            .placements
            .iter()
            .map(|placement| placement.resource)
            .filter(|id| !self.images.contains_key(id))
            .collect();
        wanted
            .into_iter()
            .filter(|id| self.sought.insert(*id))
            .collect()
    }

    fn render_tabs(&self) {
        self.tabs.set_text_content(None);
        for (id, copy) in &self.model.surfaces {
            let Ok(tab) = self.document.create_element("button") else {
                continue;
            };
            tab.set_text_content(Some(&copy.window));
            let _ = tab.set_attribute("data-surface", &id.get().to_string());
            let _ = tab.set_attribute(
                "class",
                if self.model.active == Some(*id) {
                    "active"
                } else {
                    ""
                },
            );
            if let Ok(close) = self.document.create_element("span") {
                close.set_text_content(Some("\u{d7}"));
                let _ = close.set_attribute("data-close", &id.get().to_string());
                let _ = close.set_attribute("class", "close");
                let _ = close.set_attribute("title", "Close");
                let _ = tab.append_child(&close);
            }
            let _ = self.tabs.append_child(&tab);
        }
    }

    fn render_windows(&self) {
        self.list.set_text_content(None);
        for window in &self.model.windows {
            let (Ok(item), Ok(button)) = (
                self.document.create_element("li"),
                self.document.create_element("button"),
            ) else {
                continue;
            };
            button.set_text_content(Some(&window.title));
            let _ = button.set_attribute("data-window", &window.name);
            let _ = item.append_child(&button);
            let _ = self.list.append_child(&item);
        }
    }

    /// Fits the shown surface to this client.
    fn fit(&self) {
        if let (Some(surface), Some((size, cell))) = (self.model.active, self.grid()) {
            self.send(&ClientMessage::Resize {
                surface,
                size,
                cell,
            });
        }
    }

    /// The cell under the client point `(x, y)`.
    fn cell_at(&self, x: f64, y: f64) -> (u16, u16) {
        let rect = self.canvas.get_bounding_client_rect();
        let cell = |offset: f64, size: f64| {
            #[allow(clippy::as_conversions)]
            let cell = (offset / size).floor().clamp(0.0, 999.0) as u16;
            cell
        };
        (
            cell(x - rect.left(), self.metrics.cell_width),
            cell(y - rect.top(), self.metrics.cell_height),
        )
    }

    fn pointer(&self, kind: PointerKind, button: Button, x: f64, y: f64, mods: Mods) {
        let (col, row) = self.cell_at(x, y);
        self.input(Input::Pointer(Pointer {
            kind,
            button,
            col,
            row,
            mods,
        }));
    }
}

fn schedule_draw(app: &Shared) {
    let window = {
        let mut state = app.borrow_mut();
        if state.drawing {
            return;
        }
        state.drawing = true;
        state.window.clone()
    };
    let app = Rc::clone(app);
    let callback = Closure::once_into_js(move || {
        let missing = {
            let mut state = app.borrow_mut();
            state.draw();
            state.missing()
        };
        for id in missing {
            seek(&app, id);
        }
    });
    let _ = window.request_animation_frame(callback.unchecked_ref());
}

/// Finds the image `id`: in the browser's cache, else from LispM.
fn seek(app: &Shared, id: ResourceId) {
    let app = Rc::clone(app);
    wasm_bindgen_futures::spawn_local(async move {
        if let Some(bytes) = cached(id).await {
            if id.names(&bytes) {
                keep(&app, id, bytes, false).await;
                return;
            }
        }
        app.borrow().send(&ClientMessage::Want { id });
    });
}

async fn cached(id: ResourceId) -> Option<Vec<u8>> {
    let window = web_sys::window()?;
    let caches = window.caches().ok()?;
    let cache: web_sys::Cache = JsFuture::from(caches.open(POOL))
        .await
        .ok()?
        .dyn_into()
        .ok()?;
    let found = JsFuture::from(cache.match_with_str(&format!("/pool/{}", id.to_hex())))
        .await
        .ok()?;
    let response: web_sys::Response = found.dyn_into().ok()?;
    let buffer = JsFuture::from(response.array_buffer().ok()?).await.ok()?;
    Some(Uint8Array::new(&buffer).to_vec())
}

/// Makes the image `id` from `bytes`, draws again, and keeps the bytes
/// in the browser's cache when `store`.
async fn keep(app: &Shared, id: ResourceId, bytes: Vec<u8>, store: bool) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let array = Uint8Array::from(bytes.as_slice());
    let parts = js_sys::Array::of1(&array);
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("image/png");
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options) else {
        return;
    };
    let Ok(promise) = window.create_image_bitmap_with_blob(&blob) else {
        return;
    };
    let Ok(bitmap) = JsFuture::from(promise).await else {
        return;
    };
    let Ok(bitmap) = bitmap.dyn_into::<ImageBitmap>() else {
        return;
    };
    app.borrow_mut().images.insert(id, bitmap);
    schedule_draw(app);
    if store {
        if let Ok(caches) = window.caches() {
            if let Ok(cache) = JsFuture::from(caches.open(POOL)).await {
                let cache: web_sys::Cache = cache.unchecked_into();
                let mut body = bytes;
                if let Ok(response) = web_sys::Response::new_with_opt_u8_array(Some(&mut body)) {
                    let _ = JsFuture::from(
                        cache.put_with_str(&format!("/pool/{}", id.to_hex()), &response),
                    )
                    .await;
                }
            }
        }
    }
}

fn effects(app: &Shared, effects: Vec<Effect>) {
    for effect in effects {
        match effect {
            Effect::Send(message) => app.borrow().send(&message),
            Effect::Redraw(surface) => {
                if app.borrow().model.active == Some(surface) {
                    schedule_draw(app);
                }
            }
            Effect::Windows => app.borrow().render_windows(),
            Effect::Surfaces => {
                app.borrow().render_tabs();
                schedule_draw(app);
            }
            Effect::Show(_) => {
                {
                    let state = app.borrow();
                    state.render_tabs();
                    state.fit();
                }
                focus_keys(app);
            }
            Effect::Fit(_) => app.borrow().fit(),
            Effect::Clipboard(text) => {
                let clipboard = app.borrow().window.navigator().clipboard();
                let _ = clipboard.write_text(&text);
            }
            Effect::Bell(_) => {
                let state = app.borrow();
                let _ = state.canvas.class_list().toggle("bell");
                let canvas = state.canvas.clone();
                let undo = Closure::once_into_js(move || {
                    let _ = canvas.class_list().toggle("bell");
                });
                let _ = state
                    .window
                    .set_timeout_with_callback_and_timeout_and_arguments_0(
                        undo.unchecked_ref(),
                        120,
                    );
            }
            Effect::Resource(id, bytes) => {
                let app = Rc::clone(app);
                wasm_bindgen_futures::spawn_local(async move { keep(&app, id, bytes, true).await });
            }
            Effect::Missing(_) => {}
        }
    }
}

/// Adds `handler` for `event` on `target`, for the page's life.
fn on<E: JsCast + 'static>(
    target: &web_sys::EventTarget,
    event: &str,
    handler: impl FnMut(E) + 'static,
) -> Result<(), JsValue> {
    let mut handler = handler;
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        if let Ok(event) = event.dyn_into::<E>() {
            handler(event);
        }
    });
    target.add_event_listener_with_callback(event, closure.as_ref().unchecked_ref())?;
    closure.forget();
    Ok(())
}

fn listen(app: &Shared) -> Result<(), JsValue> {
    let (window, canvas, keys, tabs, list, add) = {
        let state = app.borrow();
        (
            state.window.clone(),
            state.canvas.clone(),
            state.keys.clone(),
            state.tabs.clone(),
            state.list.clone(),
            state.add.clone(),
        )
    };
    listen_keys(app, &keys)?;
    listen_pointer(app, &canvas)?;
    let resized = Rc::clone(app);
    on(&window, "resize", move |_: web_sys::Event| {
        resized.borrow().fit();
        schedule_draw(&resized);
    })?;
    let chosen = Rc::clone(app);
    on(&tabs, "click", move |event: web_sys::MouseEvent| {
        let Some(target) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let number = |attribute: &str| {
            target
                .closest(&format!("[{attribute}]"))
                .ok()
                .flatten()
                .and_then(|found| found.get_attribute(attribute))
                .and_then(|value| value.parse::<u32>().ok())
                .map(SurfaceId::new)
        };
        if let Some(surface) = number("data-close") {
            chosen.borrow().send(&ClientMessage::Close { surface });
        } else if let Some(surface) = number("data-surface") {
            chosen.borrow_mut().model.active = Some(surface);
            {
                let state = chosen.borrow();
                state.render_tabs();
                state.fit();
            }
            focus_keys(&chosen);
            schedule_draw(&chosen);
        }
    })?;
    let opening = Rc::clone(app);
    on(&list, "click", move |event: web_sys::MouseEvent| {
        let window = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .and_then(|target| target.closest("[data-window]").ok().flatten())
            .and_then(|button| button.get_attribute("data-window"));
        let mut state = opening.borrow_mut();
        if let (Some(window), Some((size, cell))) = (window, state.grid()) {
            let message = state.model.open(&window, size, cell);
            state.send(&message);
            let _ = state.panel.set_attribute("hidden", "");
        }
    })?;
    let toggling = Rc::clone(app);
    on(&add, "click", move |_: web_sys::MouseEvent| {
        let state = toggling.borrow();
        if state.panel.has_attribute("hidden") {
            let _ = state.panel.remove_attribute("hidden");
        } else {
            let _ = state.panel.set_attribute("hidden", "");
        }
    })?;
    Ok(())
}

/// Sends text a person typed (an input method's commit, a phone's
/// keyboard) as the keys that type it, so Emacs inserts it as typing;
/// only a paste travels as text, which Emacs takes as a paste.
fn typed_text(state: &App, text: &str) {
    for c in text.chars() {
        state.input(Input::Key(Key {
            code: KeyCode::Char(c),
            mods: Mods::NONE,
        }));
    }
}

/// Gives the hidden text area the keyboard, holding no borrow of the
/// state: focusing runs the focus handlers at once.
fn focus_keys(app: &Shared) {
    let keys = app.borrow().keys.clone();
    let _ = keys.focus();
}

fn fields(event: &web_sys::KeyboardEvent) -> (String, String, bool, bool, bool, bool) {
    (
        event.key(),
        event.code(),
        event.ctrl_key(),
        event.alt_key(),
        event.meta_key(),
        event.shift_key(),
    )
}

fn listen_keys(app: &Shared, keys: &HtmlTextAreaElement) -> Result<(), JsValue> {
    let pressed = Rc::clone(app);
    on(keys, "keydown", move |event: web_sys::KeyboardEvent| {
        if event.is_composing() || event.key_code() == 229 || pressed.borrow().composing {
            return;
        }
        let (key, code, ctrl, alt, meta, shift) = fields(&event);
        let fields = KeyFields {
            key: &key,
            code: &code,
            ctrl,
            alt,
            meta,
            shift,
        };
        if let Some(input) = keymap::key_input(fields) {
            event.prevent_default();
            pressed.borrow().input(input);
        }
    })?;
    let composing = Rc::clone(app);
    on(
        keys,
        "compositionstart",
        move |_: web_sys::CompositionEvent| {
            composing.borrow_mut().composing = true;
        },
    )?;
    let composed = Rc::clone(app);
    on(
        keys,
        "compositionend",
        move |event: web_sys::CompositionEvent| {
            let mut state = composed.borrow_mut();
            state.composing = false;
            if let Some(text) = event.data() {
                typed_text(&state, &text);
            }
            state.keys.set_value("");
        },
    )?;
    let typed = Rc::clone(app);
    on(keys, "input", move |event: web_sys::InputEvent| {
        let state = typed.borrow();
        if state.composing || event.is_composing() {
            return;
        }
        let named = |named| {
            Input::Key(Key {
                code: KeyCode::Named(named),
                mods: Mods::NONE,
            })
        };
        let input = match event.input_type().as_str() {
            "insertText" => {
                if let Some(text) = event.data() {
                    typed_text(&state, &text);
                }
                None
            }
            "deleteContentBackward" => Some(named(Named::Backspace)),
            "insertLineBreak" | "insertParagraph" => Some(named(Named::Enter)),
            _ => None,
        };
        if let Some(input) = input {
            state.input(input);
        }
        state.keys.set_value("");
    })?;
    let pasted = Rc::clone(app);
    on(keys, "paste", move |event: web_sys::ClipboardEvent| {
        if let Some(text) = event
            .clipboard_data()
            .and_then(|data| data.get_data("text/plain").ok())
        {
            event.prevent_default();
            pasted.borrow().input(Input::Text(text));
        }
    })?;
    for (event, focused) in [("focus", true), ("blur", false)] {
        let app = Rc::clone(app);
        on(keys, event, move |_: web_sys::Event| {
            // Focus moves while a handler may hold the state; act after it.
            let app = Rc::clone(&app);
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(surface) = app.borrow().model.active {
                    app.borrow().send(&ClientMessage::Input {
                        surface,
                        input: Input::Focus(focused),
                    });
                }
                schedule_draw(&app);
            });
        })?;
    }
    Ok(())
}

fn button(number: i16) -> Button {
    match number {
        1 => Button::Middle,
        2 => Button::Right,
        _ => Button::Left,
    }
}

fn mods(event: &web_sys::MouseEvent) -> Mods {
    [
        (event.shift_key(), Mods::SHIFT),
        (event.alt_key(), Mods::ALT),
        (event.ctrl_key(), Mods::CTRL),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(Mods::NONE, |mods, (_, modifier)| mods.with(modifier))
}

fn listen_pointer(app: &Shared, canvas: &HtmlCanvasElement) -> Result<(), JsValue> {
    let down = Rc::clone(app);
    on(
        canvas,
        "pointerdown",
        move |event: web_sys::PointerEvent| {
            event.prevent_default();
            focus_keys(&down);
            let mut state = down.borrow_mut();
            let (x, y) = (f64::from(event.client_x()), f64::from(event.client_y()));
            if event.pointer_type() == "touch" {
                state.touch = Some(Touch {
                    x,
                    y,
                    scrolled: 0.0,
                    dragged: false,
                });
            } else {
                state.pointer(
                    PointerKind::Press,
                    button(event.button()),
                    x,
                    y,
                    mods(&event),
                );
            }
        },
    )?;
    let moved = Rc::clone(app);
    on(
        canvas,
        "pointermove",
        move |event: web_sys::PointerEvent| {
            let mut state = moved.borrow_mut();
            let (x, y) = (f64::from(event.client_x()), f64::from(event.client_y()));
            let cell_height = state.metrics.cell_height;
            if let Some(touch) = state.touch.as_mut() {
                touch.scrolled += touch.y - y;
                touch.y = y;
                let mut wheel = Vec::new();
                while touch.scrolled.abs() >= cell_height {
                    touch.dragged = true;
                    let up = touch.scrolled < 0.0;
                    touch.scrolled += if up { cell_height } else { -cell_height };
                    wheel.push(if up {
                        Button::WheelUp
                    } else {
                        Button::WheelDown
                    });
                }
                let at = (touch.x, y);
                for button in wheel {
                    state.pointer(PointerKind::Press, button, at.0, at.1, Mods::NONE);
                }
            } else if event.buttons() != 0 {
                state.pointer(
                    PointerKind::Motion,
                    button(event.button().max(0)),
                    x,
                    y,
                    mods(&event),
                );
            }
        },
    )?;
    let up = Rc::clone(app);
    on(canvas, "pointerup", move |event: web_sys::PointerEvent| {
        let mut state = up.borrow_mut();
        let (x, y) = (f64::from(event.client_x()), f64::from(event.client_y()));
        match state.touch.take() {
            Some(touch) if !touch.dragged => {
                state.pointer(
                    PointerKind::Press,
                    Button::Left,
                    touch.x,
                    touch.y,
                    Mods::NONE,
                );
                state.pointer(
                    PointerKind::Release,
                    Button::Left,
                    touch.x,
                    touch.y,
                    Mods::NONE,
                );
            }
            Some(_) => {}
            None => state.pointer(
                PointerKind::Release,
                button(event.button()),
                x,
                y,
                mods(&event),
            ),
        }
    })?;
    let wheeled = Rc::clone(app);
    on(canvas, "wheel", move |event: web_sys::WheelEvent| {
        event.prevent_default();
        let mut state = wheeled.borrow_mut();
        let step = state.metrics.cell_height * 2.0;
        state.wheel += event.delta_y();
        let (x, y) = (f64::from(event.client_x()), f64::from(event.client_y()));
        while state.wheel.abs() >= step {
            let down = state.wheel > 0.0;
            state.wheel -= if down { step } else { -step };
            let button = if down {
                Button::WheelDown
            } else {
                Button::WheelUp
            };
            state.pointer(PointerKind::Press, button, x, y, mods(&event));
        }
    })?;
    Ok(())
}
