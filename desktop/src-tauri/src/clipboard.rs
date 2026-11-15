//! System clipboard access through `arboard`, on a dedicated thread.
//!
//! The thread owns one long-lived `arboard::Clipboard`: on X11/Wayland the
//! process must stay alive as the selection owner for pasted data to remain
//! available, and on every platform it keeps clipboard calls off the UI and
//! engine threads.

use std::borrow::Cow;
use std::io::Cursor;
use std::sync::mpsc;
use std::thread;
use warp::ClipboardContent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipError {
    Empty,
    Unavailable,
}

type Job = Box<dyn FnOnce(&mut Option<arboard::Clipboard>) + Send>;

pub struct ClipboardService {
    tx: mpsc::Sender<Job>,
}

/// Largest image we send (raw RGBA). Bigger screenshots are still fine; this
/// only guards against pathological clipboard contents.
const MAX_IMAGE_PIXELS: usize = 64 * 1024 * 1024;

impl ClipboardService {
    pub fn start() -> ClipboardService {
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("warpdrop-clipboard".into())
            .spawn(move || {
                let mut cb: Option<arboard::Clipboard> = None;
                for job in rx {
                    if cb.is_none() {
                        cb = arboard::Clipboard::new().ok();
                    }
                    job(&mut cb);
                }
            })
            .expect("spawn clipboard thread");
        ClipboardService { tx }
    }

    fn run<T: Send + 'static>(&self, f: impl FnOnce(&mut arboard::Clipboard) -> T + Send + 'static) -> Option<T> {
        let (rtx, rrx) = mpsc::channel();
        let job: Job = Box::new(move |cb| {
            let r = cb.as_mut().map(f);
            let _ = rtx.send(r);
        });
        self.tx.send(job).ok()?;
        rrx.recv().ok().flatten()
    }

    /// Reads text (or a URL) first, then an image (re-encoded as PNG).
    pub fn read(&self) -> Result<ClipboardContent, ClipError> {
        let text = self.run(|cb| cb.get_text().ok()).ok_or(ClipError::Unavailable)?;
        if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
            return Ok(classify_text(text));
        }
        let image = self
            .run(|cb| {
                cb.get_image().ok().map(|img| (img.width, img.height, img.bytes.into_owned()))
            })
            .ok_or(ClipError::Unavailable)?;
        let Some((w, h, rgba)) = image else { return Err(ClipError::Empty) };
        if w == 0 || h == 0 || w * h > MAX_IMAGE_PIXELS {
            return Err(ClipError::Empty);
        }
        let png = encode_png(w as u32, h as u32, rgba).ok_or(ClipError::Unavailable)?;
        Ok(ClipboardContent::Image { mime: "image/png".into(), data: png })
    }

    pub fn write(&self, content: &ClipboardContent) -> Result<(), ClipError> {
        match content {
            ClipboardContent::Text { text } => self.write_text(text.clone()),
            ClipboardContent::Url { url } => self.write_text(url.clone()),
            ClipboardContent::Image { data, .. } => {
                let img = image::load_from_memory(data).map_err(|_| ClipError::Unavailable)?.to_rgba8();
                let (w, h) = img.dimensions();
                let bytes = img.into_raw();
                self.run(move |cb| {
                    cb.set_image(arboard::ImageData { width: w as usize, height: h as usize, bytes: Cow::Owned(bytes) }).is_ok()
                })
                .filter(|ok| *ok)
                .map(|_| ())
                .ok_or(ClipError::Unavailable)
            }
        }
    }

    pub fn write_text(&self, text: String) -> Result<(), ClipError> {
        self.run(move |cb| cb.set_text(text).is_ok()).filter(|ok| *ok).map(|_| ()).ok_or(ClipError::Unavailable)
    }
}

/// A single `http(s)://` token is sent as a URL so the receiver can open it.
pub fn classify_text(text: String) -> ClipboardContent {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    if (lower.starts_with("http://") || lower.starts_with("https://")) && !t.contains(char::is_whitespace) && t.len() > 8 {
        ClipboardContent::Url { url: t.to_string() }
    } else {
        ClipboardContent::Text { text }
    }
}

fn encode_png(w: u32, h: u32, rgba: Vec<u8>) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(w, h, rgba)?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_text() {
        assert!(matches!(classify_text("https://example.com/a?b=c".into()), ClipboardContent::Url { .. }));
        assert!(matches!(classify_text("see https://example.com".into()), ClipboardContent::Text { .. }));
        assert!(matches!(classify_text("hello".into()), ClipboardContent::Text { .. }));
    }

    #[test]
    fn png_roundtrip() {
        let png = encode_png(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 128]).unwrap();
        let back = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (2, 1));
        assert_eq!(back.into_raw(), vec![255, 0, 0, 255, 0, 0, 255, 128]);
    }
}
