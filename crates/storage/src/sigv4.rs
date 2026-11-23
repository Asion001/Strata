//! AWS Signature Version 4 for a single unsigned-payload GET (used for the
//! one S3 call OpenDAL doesn't make: GetBucketVersioning).

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("any key length");
    m.update(data.as_bytes());
    m.finalize().into_bytes().to_vec()
}

/// RFC 3986 encoding as SigV4 wants it (unreserved characters kept).
fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Headers (`host`, `x-amz-date`, `x-amz-content-sha256`, `authorization`)
/// for an S3 GET of `url` with an empty body.
pub fn sign_get(
    url: &reqwest::Url,
    region: &str,
    access_key: &str,
    secret_key: &str,
    now: DateTime<Utc>,
) -> Vec<(&'static str, String)> {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let host = match url.port() {
        Some(p) => format!("{}:{p}", url.host_str().unwrap_or("")),
        None => url.host_str().unwrap_or("").to_string(),
    };
    // Path segments are already percent-encoded by Url; decode-free re-encode
    // of the raw path would double-encode, so encode each decoded segment.
    let path = url
        .path_segments()
        .map(|segs| {
            segs.map(|s| {
                let decoded = percent_decode(s);
                uri_encode(&decoded, false)
            })
            .collect::<Vec<_>>()
            .join("/")
        })
        .unwrap_or_default();
    let canonical_uri = format!("/{path}");
    let mut query: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (uri_encode(&k, false), uri_encode(&v, false)))
        .collect();
    query.sort();
    let canonical_query = query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_request = format!(
        "GET\n{canonical_uri}\n{canonical_query}\nhost:{host}\nx-amz-content-sha256:{EMPTY_SHA256}\nx-amz-date:{amz_date}\n\n{signed_headers}\n{EMPTY_SHA256}"
    );
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );
    let k_date = hmac(format!("AWS4{secret_key}").as_bytes(), &date);
    let k_region = hmac(&k_date, region);
    let k_service = hmac(&k_region, "s3");
    let k_signing = hmac(&k_service, "aws4_request");
    let signature = hex::encode(hmac(&k_signing, &string_to_sign));
    vec![
        ("host", host),
        ("x-amz-date", amz_date),
        ("x-amz-content-sha256", EMPTY_SHA256.to_string()),
        (
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
            ),
        ),
    ]
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// The GET Object example from the AWS SigV4 documentation for S3
    /// (examplebucket, `Range` header dropped: same keys, date and scope,
    /// signature recomputed for these signed headers).
    #[test]
    fn known_signing_key_and_shape() {
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let url =
            reqwest::Url::parse("https://examplebucket.s3.amazonaws.com/?versioning").unwrap();
        let h = sign_get(
            &url,
            "us-east-1",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            now,
        );
        let auth = &h.iter().find(|(k, _)| *k == "authorization").unwrap().1;
        assert!(auth.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature="
        ));
        // AWS's documented example for GET ?lifecycle with the same headers
        // signs to this value; ?versioning differs only in the query name.
        let url2 =
            reqwest::Url::parse("https://examplebucket.s3.amazonaws.com/?lifecycle").unwrap();
        let h2 = sign_get(
            &url2,
            "us-east-1",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            now,
        );
        let sig = h2[3].1.rsplit("Signature=").next().unwrap();
        assert_eq!(
            sig,
            "fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
        );
    }

    #[test]
    fn encoding() {
        assert_eq!(uri_encode("a b/c~", false), "a%20b%2Fc~");
        assert_eq!(uri_encode("a b/c", true), "a%20b/c");
        assert_eq!(percent_decode("a%20b"), "a b");
    }
}
