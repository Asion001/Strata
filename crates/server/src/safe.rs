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
pub const SNIFF_LEN: u64 = 4096; // enough for long XML/SVG prologs

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
    } else if starts(b"BM")
        && head.len() >= 18
        && head[6..10] == [0, 0, 0, 0]
        && matches!(head[14], 12 | 40 | 52 | 56 | 64 | 108 | 124)
        && head[15..18] == [0, 0, 0]
    {
        "image/bmp"
    } else if starts(b"II*\0") || starts(b"MM\0*") {
        "image/tiff"
    } else if at(4, b"ftyp") {
        return ftyp(head);
    } else if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        "video/webm"
    } else if starts(b"OggS") {
        "audio/ogg"
    } else if starts(b"fLaC") {
        "audio/flac"
    } else if (starts(b"ID3") && head.len() >= 5 && matches!(head[3], 2..=4) && head[4] != 0xFF)
        || mpeg_frame(head)
    {
        "audio/mpeg"
    } else if adts_frame(head) {
        "audio/aac"
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

/// ISO base media files: images (AVIF/HEIF), audio (M4A), video (MP4,
/// 3GP, QuickTime). The major brand decides, falling back to the compatible
/// brands; anything else (e.g. camera raw files) is not a known media type.
fn ftyp(head: &[u8]) -> Option<&'static str> {
    let size = u32::from_be_bytes(head.get(0..4)?.try_into().ok()?) as usize;
    let mut brands: Vec<&[u8]> = vec![head.get(8..12)?];
    let end = size.min(head.len());
    let mut off = 16;
    while off + 4 <= end {
        brands.push(&head[off..off + 4]);
        off += 4;
    }
    let image = |b: &[u8]| match b {
        b"avif" | b"avis" => Some("image/avif"),
        b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"hevx" => Some("image/heic"),
        b"mif1" | b"msf1" => Some("image/heif"),
        _ => None,
    };
    let other = |b: &[u8]| match b {
        b"M4A " | b"M4B " | b"M4P " => Some("audio/mp4"),
        b"qt  " => Some("video/quicktime"),
        b"crx " | b"CRX " => Some("raw"),
        _ if b.starts_with(b"iso")
            || b.starts_with(b"mp4")
            || b.starts_with(b"3gp")
            || b.starts_with(b"3g2") =>
        {
            Some("video/mp4")
        }
        b"avc1" | b"dash" | b"M4V " | b"M4VH" | b"M4VP" | b"mmp4" | b"mp71" | b"XAVC" | b"MSNV"
        | b"f4v " | b"NDAS" => Some("video/mp4"),
        _ => None,
    };
    // Major brand first, then compatible brands.
    let pick = image(brands[0]).or_else(|| other(brands[0])).or_else(|| {
        brands[1..]
            .iter()
            .find_map(|b| image(b))
            .or_else(|| brands[1..].iter().find_map(|b| other(b)))
    })?;
    (pick != "raw").then_some(pick)
}

/// Length of the MPEG audio frame starting at `h`, if `h` starts with a
/// valid frame header (Layer I/II/III, MPEG 1/2/2.5).
fn mpeg_frame_len(h: &[u8]) -> Option<usize> {
    if h.len() < 4 || h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (h[1] >> 3) & 3; // 0 = 2.5, 2 = 2, 3 = 1
    let layer = (h[1] >> 1) & 3; // 1 = III, 2 = II, 3 = I
    let bitrate_idx = (h[2] >> 4) as usize;
    let rate_idx = ((h[2] >> 2) & 3) as usize;
    let padding = ((h[2] >> 1) & 1) as usize;
    if version == 1 || layer == 0 || bitrate_idx == 0 || bitrate_idx == 15 || rate_idx == 3 {
        return None;
    }
    const RATES: [[usize; 3]; 4] = [
        [11025, 12000, 8000],
        [0, 0, 0],
        [22050, 24000, 16000],
        [44100, 48000, 32000],
    ];
    const BR_V1: [[usize; 15]; 3] = [
        [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ], // III
        [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
        ], // II
        [
            0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
        ], // I
    ];
    const BR_V2: [[usize; 15]; 3] = [
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160], // II and III
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
        [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
        ], // I
    ];
    let rate = RATES[version as usize][rate_idx];
    let li = (layer - 1) as usize;
    let kbps = if version == 3 {
        BR_V1[li][bitrate_idx]
    } else {
        BR_V2[li][bitrate_idx]
    };
    let len = match layer {
        3 => (12 * kbps * 1000 / rate + padding) * 4,
        1 if version != 3 => 72 * kbps * 1000 / rate + padding,
        _ => 144 * kbps * 1000 / rate + padding,
    };
    (len >= 4).then_some(len)
}

/// MPEG audio (MP3 and friends): a valid frame header followed by another
/// one exactly a frame later. One header alone is too weak: three ordinary
/// bytes of Windows-1251 text (e.g. "я...") can look like one.
fn mpeg_frame(h: &[u8]) -> bool {
    match mpeg_frame_len(h) {
        Some(len) => h.len() >= len + 4 && mpeg_frame_len(&h[len..]).is_some(),
        None => false,
    }
}

/// ADTS (raw .aac): a header whose frame length points at the next header.
fn adts_frame(h: &[u8]) -> bool {
    let header = |h: &[u8]| -> Option<usize> {
        if h.len() < 7 || h[0] != 0xFF || h[1] & 0xF6 != 0xF0 || (h[2] >> 2) & 0x0F >= 13 {
            return None;
        }
        let len =
            (((h[3] & 0x03) as usize) << 11) | ((h[4] as usize) << 3) | ((h[5] as usize) >> 5);
        (len >= 7).then_some(len)
    };
    match header(h) {
        Some(len) => h.len() >= len + 7 && header(&h[len..]).is_some(),
        None => false,
    }
}

/// Text flavour of the start of a file, if it looks like text.
fn text_kind(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0xFF, 0xFE]) {
        return Some("text/plain; charset=utf-16le");
    }
    if head.starts_with(&[0xFE, 0xFF]) {
        return Some("text/plain; charset=utf-16be");
    }
    if head.contains(&0) {
        return None;
    }
    match std::str::from_utf8(head) {
        Ok(_) => return Some("text/plain; charset=utf-8"),
        Err(e) if e.error_len().is_none() => return Some("text/plain; charset=utf-8"), // cut-off last char
        Err(_) => {}
    }
    // Legacy 8-bit encodings (Windows-1251, Latin-1, ...): text if almost no
    // control bytes. The browser picks the charset; nosniff keeps it text.
    let controls = head
        .iter()
        .filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B))
        .count();
    // A stray byte or two (e.g. a DOS EOF marker) in a short file is fine.
    (controls <= 2 + head.len() / 100).then_some("text/plain")
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

/// What the first bytes of a file say about it (independent of purpose, so
/// it can be cached per file version).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Class {
    /// Media type for normal serving.
    pub base: &'static str,
    /// An SVG document (named .svg and containing `<svg`).
    pub svg: bool,
}

pub fn classify(name: &str, head: &[u8]) -> Class {
    let sniffed = sniff(head);
    let text = text_kind(head);
    let svg = ext(name) == "svg"
        && sniffed.is_none()
        && text.is_some()
        && head.windows(4).any(|w| w.eq_ignore_ascii_case(b"<svg"));
    let base = sniffed.or(text).unwrap_or("application/octet-stream");
    Class { base, svg }
}

/// Decide the Content-Type and disposition: magic bytes decide; the
/// extension only matters for SVG on the image-preview route.
pub fn decide_class(class: Class, purpose: Purpose) -> Decision {
    if purpose == Purpose::ImagePreview && class.svg {
        return Decision {
            content_type: "image/svg+xml",
            inline: true,
        };
    }
    let content_type = class.base;
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

pub fn decide(name: &str, head: &[u8], purpose: Purpose) -> Decision {
    decide_class(classify(name, head), purpose)
}

/// An ETag value safe to put in a header (backends may return anything).
pub fn header_safe_tag(tag: &str) -> String {
    if !tag.is_empty() && tag.len() <= 128 && tag.bytes().all(|b| b.is_ascii_graphic() && b != b'"')
    {
        tag.to_string()
    } else {
        blake3::hash(tag.as_bytes()).to_hex()[..32].to_string()
    }
}

/// Unicode format characters that can disguise a file name (e.g. U+202E
/// RIGHT-TO-LEFT OVERRIDE making "fdp.exe" read as "exe.pdf").
fn is_bidi_or_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}' | '\u{061C}')
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
        .filter(|c| !c.is_control() && !is_bidi_or_invisible(*c) && *c != '"' && *c != '\\')
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
    fn loose_markers_are_not_media() {
        assert_eq!(
            decide("a.txt", b"\xff\xfeh\0i\0", C).content_type,
            "text/plain; charset=utf-16le"
        );
        assert_eq!(
            decide("a.txt", b"BMW service log, 2026: oil changed", C).content_type,
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            decide("a.txt", b"ID3 is a tag format", C).content_type,
            "text/plain; charset=utf-8"
        );
        assert_eq!(sniff(b"\0\0\0\x18ftypheic"), Some("image/heic"));
        assert_eq!(sniff(b"\0\0\0\x18ftypcrx "), None);
        assert_eq!(sniff(b"ID3\x04\x00"), Some("audio/mpeg"));
        // two MP3 frames (MPEG-1 Layer III, 128 kbps, 44.1 kHz: 417 bytes)
        let mut mp3 = vec![0xFF, 0xFB, 0x90, 0x00];
        mp3.resize(417, 0);
        mp3.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        assert_eq!(sniff(&mp3), Some("audio/mpeg"));
        assert_eq!(
            sniff(&[0xFF, 0xFB, 0x90, 0x00]),
            None,
            "one header is not enough"
        );
        // Windows-1251 Cyrillic text stays viewable
        let cp1251 = [
            0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2, b' ', 0xEC, 0xE8, 0xF0, b'\n',
        ];
        assert_eq!(decide("a.txt", &cp1251, C).content_type, "text/plain");
    }

    #[test]
    fn long_svg_prolog_is_still_an_image_on_the_preview_route() {
        let mut svg = b"<?xml version='1.0'?>\n<!-- generator -->\n".to_vec();
        svg.extend(std::iter::repeat_n(b' ', 1500));
        svg.extend_from_slice(b"<svg xmlns='http://www.w3.org/2000/svg'/>");
        assert_eq!(
            decide("a.svg", &svg, Purpose::ImagePreview).content_type,
            "image/svg+xml"
        );
    }

    #[test]
    fn bidi_overrides_are_stripped() {
        let v = content_disposition(false, "invoice\u{202E}fdp.exe");
        assert!(v.to_str().unwrap().contains("filename=\"invoicefdp.exe\""));
    }

    #[test]
    fn more_media_and_text() {
        // 3GP phone video with major brand 3gp6, and XAVC camera video
        assert_eq!(
            sniff(b"\0\0\0\x18ftyp3gp6\0\0\0\0isom3gp6"),
            Some("video/mp4")
        );
        assert_eq!(
            sniff(b"\0\0\0\x18ftypXAVC\0\0\0\0XAVCmp42"),
            Some("video/mp4")
        );
        // unknown major brand, but compatible with isom
        assert_eq!(
            sniff(b"\0\0\0\x18ftypabcd\0\0\0\0isommp41"),
            Some("video/mp4")
        );
        // Canon CR3 is not media
        assert_eq!(sniff(b"\0\0\0\x18ftypcrx \0\0\0\0crx isom"), None);
        // ADTS AAC
        let mut aac = vec![0xFF, 0xF1, 0x50, 0x80, 0x02, 0x1F, 0xFC];
        aac.resize(16, 0);
        aac.extend_from_slice(&[0xFF, 0xF1, 0x50, 0x80, 0x02, 0x1F, 0xFC]);
        assert_eq!(sniff(&aac), Some("audio/aac"));
        // CP1251 text starting with "ясно" / "явно" is text, not audio
        let ya: Vec<u8> = vec![0xFF, 0xF1, 0xED, 0xEE, b' ', 0xFF, 0xE2, 0xED, 0xEE, b'\n'];
        assert_eq!(decide("n.txt", &ya, C).content_type, "text/plain");
        let ya2: Vec<u8> = vec![
            0xFF, 0xE2, 0xED, 0xEE, b' ', 0xF2, 0xE5, 0xEA, 0xF1, 0xF2, b'\n',
        ];
        assert_eq!(decide("n.txt", &ya2, C).content_type, "text/plain");
        // short CP1251 text ending with a DOS EOF marker
        let mut t = vec![0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2, b'\n'];
        t.push(0x1A);
        assert_eq!(decide("r.txt", &t, C).content_type, "text/plain");
        assert_eq!(header_safe_tag("abc-1"), "abc-1");
        assert_eq!(header_safe_tag("a\"b\u{e9}").len(), 32);
    }

    #[test]
    fn media_types() {
        assert_eq!(sniff(b"\0\0\0\x20ftypisom"), Some("video/mp4"));
        assert_eq!(sniff(b"\0\0\0\x20ftypM4A "), Some("audio/mp4"));
        assert_eq!(sniff(b"%PDF-1.7"), Some("application/pdf"));
        assert_eq!(sniff(b"ID3\x04\x00"), Some("audio/mpeg"));
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
