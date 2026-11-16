//! Safe responses for user file bytes.
//!
//! No uploaded file may run script in a user's session, whatever its name,
//! type or content. Every response carrying user bytes gets a sandboxing CSP
//! and friends, and the Content-Type comes from a server-side allowlist keyed
//! on the file's magic bytes. The user-controlled extension alone never makes
//! a file "active": anything that could be (HTML, SVG, XML, JS, CSS, ...) is
//! served as `text/plain` inline or as an attachment.

use axum::http::header::{self, HeaderMap, HeaderValue};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// CSP for every response that carries user file bytes.
pub const FILE_CSP: &str = "sandbox; default-src 'none'; img-src 'self' data:; media-src 'self'; style-src 'unsafe-inline'";

/// Bytes read from the start of a file to decide its type.
pub const SNIFF_LEN: u64 = 512;

/// Set the protective headers on a response carrying user file bytes.
pub fn protect(h: &mut HeaderMap) {
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(FILE_CSP),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}

/// Recognise a file by its magic bytes. Only types on this list are ever
/// served with their real media type.
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    let starts = |m: &[u8]| head.starts_with(m);
    let at = |off: usize, m: &[u8]| head.len() >= off + m.len() && &head[off..off + m.len()] == m;
    Some(if starts(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if starts(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        "image/gif"
    } else if starts(b"RIFF") && at(8, b"WEBP") {
        "image/webp"
    } else if starts(b"RIFF") && at(8, b"WAVE") {
        "audio/wav"
    } else if starts(b"BM") && head.len() >= 14 {
        "image/bmp"
    } else if starts(b"II*\0") || starts(b"MM\0*") {
        "image/tiff"
    } else if at(4, b"ftyp") {
        match head.get(8..12) {
            Some(b"avif") | Some(b"avis") => "image/avif",
            Some(b"M4A ") | Some(b"M4B ") => "audio/mp4",
            Some(b"qt  ") => "video/quicktime",
            _ => "video/mp4",
        }
    } else if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        "video/webm"
    } else if starts(b"OggS") {
        "audio/ogg"
    } else if starts(b"fLaC") {
        "audio/flac"
    } else if starts(b"ID3") || (head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0) {
        "audio/mpeg"
    } else if starts(b"%PDF-") {
        "application/pdf"
    } else if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
        "application/zip"
    } else if starts(&[0x1F, 0x8B]) {
        "application/gzip"
    } else {
        return None;
    })
}

/// Does the start of a file look like text (no NUL, valid UTF-8 apart from a
/// possibly cut-off last character)?
fn looks_like_text(head: &[u8]) -> bool {
    if head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(), // only an incomplete trailing char
    }
}

/// How to serve a file's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub content_type: &'static str,
    /// false: Content-Disposition attachment.
    pub inline: bool,
}

/// What the response is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Plain download or inline view (`download` forces attachment).
    Content { download: bool },
    /// The image-preview route, loaded by the UI only through `<img>`.
    /// The only place SVG is served as `image/svg+xml`.
    ImagePreview,
}

fn ext(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Decide the Content-Type and disposition from the magic bytes; the
/// extension only matters for SVG on the image-preview route.
pub fn decide(name: &str, head: &[u8], purpose: Purpose) -> Decision {
    if purpose == Purpose::ImagePreview && ext(name) == "svg" && looks_like_text(head) {
        let text = String::from_utf8_lossy(head).to_ascii_lowercase();
        if text.contains("<svg") {
            return Decision {
                content_type: "image/svg+xml",
                inline: true,
            };
        }
    }
    let content_type = match sniff(head) {
        Some(t) => t,
        None if looks_like_text(head) => "text/plain; charset=utf-8",
        None => "application/octet-stream",
    };
    let inline = match purpose {
        Purpose::Content { download: true } => false,
        Purpose::Content { download: false } | Purpose::ImagePreview => {
            content_type.starts_with("image/")
                || content_type.starts_with("video/")
                || content_type.starts_with("audio/")
                || content_type == "application/pdf"
                || content_type.starts_with("text/plain")
        }
    };
    Decision {
        content_type,
        inline,
    }
}

const RFC5987: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'!')
    .remove(b'#')
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b'-')
    .remove(b'.')
    .remove(b'^')
    .remove(b'_')
    .remove(b'`')
    .remove(b'|')
    .remove(b'~');

/// RFC 6266 Content-Disposition with an ASCII fallback and a UTF-8
/// `filename*`. Control characters, quotes and backslashes are stripped, so
/// a file name can never break out of the header.
pub fn content_disposition(inline: bool, name: &str) -> HeaderValue {
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    let clean = if clean.trim().is_empty() {
        "download".to_string()
    } else {
        clean
    };
    let ascii: String = clean
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() {
                c
            } else {
                '_'
            }
        })
        .collect();
    let kind = if inline { "inline" } else { "attachment" };
    let v = format!(
        "{kind}; filename=\"{ascii}\"; filename*=UTF-8''{}",
        utf8_percent_encode(&clean, RFC5987)
    );
    HeaderValue::from_str(&v).unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: Purpose = Purpose::Content { download: false };

    #[test]
    fn active_types_are_never_served_as_such() {
        for (name, body) in [
            ("evil.html", &b"<html><script>alert(1)</script>"[..]),
            (
                "evil.svg",
                b"<svg xmlns='http://www.w3.org/2000/svg' onload='alert(1)'/>",
            ),
            (
                "evil.xhtml",
                b"<html xmlns='http://www.w3.org/1999/xhtml'><script>x</script></html>",
            ),
            ("evil.js", b"alert(1)"),
            ("evil.xml", b"<?xml version='1.0'?><x/>"),
            ("evil.css", b"body{}"),
        ] {
            let d = decide(name, body, C);
            assert_eq!(d.content_type, "text/plain; charset=utf-8", "{name}");
        }
    }

    #[test]
    fn extension_alone_decides_nothing() {
        // A JPEG/HTML polyglot is a JPEG; HTML named .jpg is text.
        let mut poly = vec![0xFF, 0xD8, 0xFF, 0xE0];
        poly.extend_from_slice(b"<html><script>alert(1)</script>");
        assert_eq!(decide("x.html", &poly, C).content_type, "image/jpeg");
        assert_eq!(
            decide("x.jpg", b"<script>alert(1)</script>", C).content_type,
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            decide("x.png", &[0, 1, 2, 3], C).content_type,
            "application/octet-stream"
        );
        assert!(!decide("x.png", &[0, 1, 2, 3], C).inline);
    }

    #[test]
    fn svg_is_an_image_only_on_the_preview_route() {
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'/>";
        assert_eq!(
            decide("a.svg", svg, Purpose::ImagePreview).content_type,
            "image/svg+xml"
        );
        assert_eq!(
            decide("a.svg", svg, C).content_type,
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            decide("a.txt", svg, Purpose::ImagePreview).content_type,
            "text/plain; charset=utf-8"
        );
    }

    #[test]
    fn downloads_are_attachments() {
        assert!(
            !decide(
                "a.jpg",
                &[0xFF, 0xD8, 0xFF],
                Purpose::Content { download: true }
            )
            .inline
        );
        assert!(decide("a.jpg", &[0xFF, 0xD8, 0xFF], C).inline);
    }

    #[test]
    fn media_types() {
        assert_eq!(sniff(b"\0\0\0\x20ftypisom"), Some("video/mp4"));
        assert_eq!(sniff(b"\0\0\0\x20ftypM4A "), Some("audio/mp4"));
        assert_eq!(sniff(b"%PDF-1.7"), Some("application/pdf"));
        assert_eq!(sniff(b"ID3\x04"), Some("audio/mpeg"));
    }

    #[test]
    fn disposition_cannot_inject_headers() {
        let v = content_disposition(false, "a\"b\r\nSet-Cookie: x=1;\\.txt");
        let s = v.to_str().unwrap();
        assert!(!s.contains('\r') && !s.contains('\n'));
        assert_eq!(s.matches('"').count(), 2, "{s}");
        assert!(
            s.starts_with("attachment; filename=\"abSet-Cookie: x=1;.txt\"; filename*=UTF-8''")
        );
        let u = content_disposition(true, "фото.jpg");
        assert!(u.to_str().unwrap().contains("filename=\"____.jpg\""));
        assert!(
            u.to_str()
                .unwrap()
                .contains("UTF-8''%D1%84%D0%BE%D1%82%D0%BE.jpg")
        );
    }
}
