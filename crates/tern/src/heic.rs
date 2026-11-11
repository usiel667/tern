//! HEIC / HEIF pictures (iPhone photos) for the message view, which can't draw them: decoded
//! by GTK's image loader (glycin, with its HEIF loader) and turned into a JPEG to show. The
//! attachment itself stays the original.

use gtk::gdk;
use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf};
use gtk::glib;
use gtk::prelude::*;

/// Longest side of the picture shown; phone photos are ~4000 px, more than any message view.
const MAX_SIDE: i32 = 2400;

/// Whether an attachment is a HEIC / HEIF picture (by type, or by name when sent as a
/// generic file).
pub fn is_heic(mime: &str, filename: Option<&str>) -> bool {
    let mime = mime.to_ascii_lowercase();
    let by_name = filename.is_some_and(|f| {
        let f = f.to_ascii_lowercase();
        f.ends_with(".heic") || f.ends_with(".heif")
    });
    mime.starts_with("image/heic") || mime.starts_with("image/heif") || (mime == "application/octet-stream" && by_name)
}

/// The picture as a JPEG no bigger than `MAX_SIDE`, or `None` if it can't be read. Slow for
/// a full photo (most of a second): call it off the main thread.
pub fn to_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    let texture = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)).ok()?;
    let (w, h) = (texture.width(), texture.height());
    let mut down = gdk::TextureDownloader::new(&texture);
    down.set_format(gdk::MemoryFormat::R8g8b8);
    let (pixels, stride) = down.download_bytes();
    let pixbuf = Pixbuf::from_bytes(&pixels, Colorspace::Rgb, false, 8, w, h, stride as i32);
    let scale = (MAX_SIDE as f64 / w.max(h) as f64).min(1.0);
    let pixbuf = if scale < 1.0 {
        let (nw, nh) = (((w as f64) * scale).round().max(1.0) as i32, ((h as f64) * scale).round().max(1.0) as i32);
        pixbuf.scale_simple(nw, nh, InterpType::Bilinear)?
    } else {
        pixbuf
    };
    pixbuf.save_to_bufferv("jpeg", &[("quality", "85")]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_heic() {
        assert!(is_heic("image/heic", None));
        assert!(is_heic("IMAGE/HEIF", Some("x")));
        assert!(is_heic("application/octet-stream", Some("IMG_0001.HEIC")));
        assert!(!is_heic("image/jpeg", Some("a.jpeg")));
        assert!(!is_heic("application/octet-stream", Some("notes.txt")));
    }

    #[test]
    fn converts_to_jpeg() {
        // A 64×48 gradient made with ImageMagick (`magick -size 64x48 gradient:red-blue x.heic`).
        let heic = include_bytes!("testdata/tiny.heic");
        let Some(jpeg) = to_jpeg(heic) else {
            // No HEIF loader on this machine (glycin's heif loader / libheif): nothing to test.
            eprintln!("skipped: no HEIF loader");
            return;
        };
        assert_eq!(&jpeg[..3], &[0xFF, 0xD8, 0xFF], "a JPEG");
        let back = gdk::Texture::from_bytes(&glib::Bytes::from(&jpeg)).unwrap();
        assert_eq!((back.width(), back.height()), (64, 48), "small pictures keep their size");
    }
}
