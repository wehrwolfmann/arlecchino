//! Extension point for real drawing.
//!
//! [`crate::api`] keeps the Graphics state (colour, clip, translation, font) and
//! writes the call trace; every drawing call is then forwarded, unchanged, to the
//! [`Backend`] installed with [`install`].  Without one (the default) nothing is
//! drawn, which is all the trace comparison needs.  A renderer that produces
//! pixels — on top of an own MIDP framebuffer, a window, or a headless image
//! dump for a pixel diff — implements this trait and is installed before
//! `run_midlet`.
//!
//! Coordinates are passed exactly as the game passed them; [`State::translate`]
//! is the accumulated `Graphics.translate` the backend must add, and
//! [`State::clip`] is the clip in device coordinates (the translation in force
//! when it was set is already applied, as with an AWT clip).

use std::cell::RefCell;

/// Where a drawing call goes: the screen (inside `paint`) or a mutable image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Screen,
    /// A mutable image, by the id `I <id> …` gave it in the trace.
    Image(i32),
}

/// Graphics state at the moment of a drawing call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// `0xRRGGBB`.
    pub color: i32,
    /// `(x, y, width, height)` in device coordinates.
    pub clip: (i32, i32, i32, i32),
    /// `(tx, ty)`.
    pub translate: (i32, i32),
    /// `(face, style, size)` of the current `Font`.
    pub font: (i32, i32, i32),
}

/// A renderer.  Every method has an empty default, so a backend implements only
/// what it supports; `anchor` and `transform` are the raw MIDP constants.
#[allow(unused_variables, clippy::too_many_arguments)]
pub trait Backend {
    /// Before `paint` of frame `frame` on a `width`×`height` screen.
    fn begin_frame(&mut self, frame: i32, width: i32, height: i32) {}
    /// After `paint` returned.
    fn end_frame(&mut self, frame: i32) {}
    /// An image came into existence.  `png` holds the encoded bytes for
    /// `createImage(String)` / `createImage(byte[],int,int)`, `None` for a mutable image.
    fn create_image(&mut self, id: i32, width: i32, height: i32, png: Option<&[u8]>) {}
    fn fill_rect(&mut self, to: Target, st: &State, x: i32, y: i32, w: i32, h: i32) {}
    fn draw_rect(&mut self, to: Target, st: &State, x: i32, y: i32, w: i32, h: i32) {}
    fn draw_line(&mut self, to: Target, st: &State, x1: i32, y1: i32, x2: i32, y2: i32) {}
    fn fill_round_rect(&mut self, to: Target, st: &State, x: i32, y: i32, w: i32, h: i32, arc_w: i32, arc_h: i32) {}
    fn draw_round_rect(&mut self, to: Target, st: &State, x: i32, y: i32, w: i32, h: i32, arc_w: i32, arc_h: i32) {}
    fn fill_arc(&mut self, to: Target, st: &State, x: i32, y: i32, w: i32, h: i32, start: i32, arc: i32) {}
    fn draw_arc(&mut self, to: Target, st: &State, x: i32, y: i32, w: i32, h: i32, start: i32, arc: i32) {}
    fn fill_triangle(&mut self, to: Target, st: &State, x1: i32, y1: i32, x2: i32, y2: i32, x3: i32, y3: i32) {}
    fn draw_image(&mut self, to: Target, st: &State, image: i32, x: i32, y: i32, anchor: i32) {}
    fn draw_region(
        &mut self,
        to: Target,
        st: &State,
        image: i32,
        src_x: i32,
        src_y: i32,
        w: i32,
        h: i32,
        transform: i32,
        x: i32,
        y: i32,
        anchor: i32,
    ) {
    }
    fn draw_string(&mut self, to: Target, st: &State, text: &str, x: i32, y: i32, anchor: i32) {}
}

thread_local! {
    static BACKEND: RefCell<Option<Box<dyn Backend>>> = const { RefCell::new(None) };
}

/// Installs `backend` for this thread and returns the previous one.
pub fn install(backend: Box<dyn Backend>) -> Option<Box<dyn Backend>> {
    BACKEND.with(|b| b.borrow_mut().replace(backend))
}

/// Removes the installed backend (drawing becomes trace-only again).
pub fn uninstall() -> Option<Box<dyn Backend>> {
    BACKEND.with(|b| b.borrow_mut().take())
}

/// Runs `f` on the installed backend, if any.  The game never runs inside `f`,
/// so a backend cannot be re-entered.
pub(crate) fn with(f: impl FnOnce(&mut dyn Backend)) {
    BACKEND.with(|b| {
        if let Some(be) = b.borrow_mut().as_mut() {
            f(be.as_mut())
        }
    })
}
