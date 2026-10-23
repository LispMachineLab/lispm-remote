// lispm-remote: the page.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The page around the model: the WebSocket to LispM, the canvas, the
//! keyboard (through a hidden text area, so phones show theirs and input
//! methods work), the pointer and touch, the window list and tabs, and
//! the image pool (in memory, and in the browser's cache across visits).

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
use web_sys::WebSocket;

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
    socket: Option<WebSocket>,
    model: Model,
    metrics: Metrics,
    images: HashMap<ResourceId, ImageBitmap>,
    sought: HashSet<ResourceId>,
    drawing: bool,
    composing: bool,
    touch: Option<Touch>,
    wheel: f64,
}

/// Starts the page.
///
/// # Errors
///
/// When the page lacks its elements or the browser its canvas.
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let app = Rc::new(RefCell::new(App::new()?));
    listen(&app)?;
    connect(&app)?;
    Ok(())
}

fn element<T: JsCast>(document: &Document, id: &str) -> Result<T, JsValue> {
    document
        .get_element_by_id(id)
        .and_then(|element| element.dyn_into::<T>().ok())
        .ok_or_else(|| JsValue::from_str(&format!("the page has no #{id}")))
}

impl App {
    fn new() -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or("no window")?;
        let document = window.document().ok_or("no document")?;
        let canvas: HtmlCanvasElement = element(&document, "canvas")?;
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
            keys: element(&document, "keys")?,
            tabs: element(&document, "tabs")?,
            list: element(&document, "window-list")?,
            panel: element(&document, "windows")?,
            status: element(&document, "status")?,
            window,
            document,
            canvas,
            context,
            socket: None,
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
        if let Some(socket) = &self.socket {
            if socket.ready_state() == WebSocket::OPEN {
                let _ = socket.send_with_u8_array(&message.encode());
            }
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
                &copy.screen,
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
            .screen
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
                    "tab active"
                } else {
                    "tab"
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

fn connect(app: &Shared) -> Result<(), JsValue> {
    let (url, status) = {
        let state = app.borrow();
        let location = state.window.location();
        let hash = location.hash()?;
        let token = hash.strip_prefix("#t=").unwrap_or("");
        let scheme = if location.protocol()? == "https:" {
            "wss"
        } else {
            "ws"
        };
        (
            format!("{scheme}://{}/ws?t={token}", location.host()?),
            state.status.clone(),
        )
    };
    status.set_text_content(Some("Connecting\u{2026}"));
    let socket = WebSocket::new(&url)?;
    socket.set_binary_type(web_sys::BinaryType::Arraybuffer);
    let opened = Rc::clone(app);
    let on_open = Closure::<dyn FnMut()>::new(move || {
        let mut state = opened.borrow_mut();
        state.model.reset();
        state.sought.clear();
        state.set_status("");
    });
    socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    on_open.forget();
    let receiving = Rc::clone(app);
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            let bytes = Uint8Array::new(&event.data()).to_vec();
            if let Ok(message) = ServerMessage::decode(&bytes) {
                let done = receiving.borrow_mut().model.receive(message);
                effects(&receiving, done);
            }
        });
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();
    let closed = Rc::clone(app);
    let on_close = Closure::<dyn FnMut()>::new(move || {
        let window = {
            let mut state = closed.borrow_mut();
            state.socket = None;
            state.set_status("Disconnected; trying again\u{2026}");
            state.window.clone()
        };
        let again = Rc::clone(&closed);
        let retry = Closure::once_into_js(move || {
            let _ = connect(&again);
        });
        let _ = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(retry.unchecked_ref(), 2000);
    });
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();
    app.borrow_mut().socket = Some(socket);
    Ok(())
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
    let (window, canvas, keys, tabs, list, document) = {
        let state = app.borrow();
        (
            state.window.clone(),
            state.canvas.clone(),
            state.keys.clone(),
            state.tabs.clone(),
            state.list.clone(),
            state.document.clone(),
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
    let add: HtmlElement = element(&document, "add")?;
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
