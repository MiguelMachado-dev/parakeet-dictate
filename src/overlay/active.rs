use std::sync::{Arc, Mutex};

use log::error;

use super::Style;
use super::window::OverlaySurface;
use crate::cues::PlayCue;
use crate::reducer::{Cue, Transition};

pub type OpenWindow = Box<dyn Fn(Style) -> Result<Box<dyn OverlaySurface>, String> + Send + Sync>;

struct Inner {
    style: String,
    overlay: Option<Box<dyn OverlaySurface>>,
    last: Option<Transition>,
}

/// Which overlay style shows, if any, and the one window that draws it. The window opens the first
/// time a style is active, so an app started with "off" creates none until the tray picks a style.
pub struct ActiveOverlay {
    // The controller notifies while the tray switches; without one lock a resync of an older transition
    // could reach the overlay after a newer one.
    inner: Mutex<Inner>,
    open_window: OpenWindow,
    cues: Arc<dyn PlayCue>,
}

impl ActiveOverlay {
    pub fn new(style: &str, open_window: OpenWindow, cues: Arc<dyn PlayCue>) -> Self {
        let inner = Mutex::new(Inner { style: "off".to_owned(), overlay: None, last: None });
        let overlay = Self { inner, open_window, cues };
        overlay.switch(style);
        overlay
    }

    pub fn style(&self) -> String {
        self.inner.lock().unwrap().style.clone()
    }

    /// Draw `style` from now on, or "off". False when the window could not be created; it stays off.
    pub fn switch(&self, style: &str) -> bool {
        let renderer = Style::named(style);
        let mut inner = self.inner.lock().unwrap();
        match (renderer, inner.overlay.as_ref()) {
            (Some(renderer), None) => match (self.open_window)(renderer) {
                Ok(window) => inner.overlay = Some(window),
                Err(e) => {
                    error!("could not create the overlay window; dictating without it: {e}");
                    self.cues.play(Cue::Error);
                    return false;
                }
            },
            (_, Some(window)) => window.restyle(renderer),
            (None, None) => {}
        }
        inner.style = style.to_owned();
        if renderer.is_some()
            && let (Some(window), Some(last)) = (inner.overlay.as_ref(), inner.last.as_ref())
        {
            // Off dropped the dictation on screen; without this it would only reappear on the next transition.
            window.notify(last);
        }
        true
    }

    pub fn notify(&self, transition: &Transition) {
        let mut inner = self.inner.lock().unwrap();
        inner.last = Some(transition.clone());
        if inner.style != "off"
            && let Some(window) = inner.overlay.as_ref()
        {
            window.notify(transition);
        }
    }

    pub fn close(&self) {
        if let Some(mut window) = self.inner.lock().unwrap().overlay.take() {
            window.close();
        }
    }
}
