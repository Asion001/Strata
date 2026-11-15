//! Path handling. Strata paths are relative, `/`-separated, never start or end
//! with `/`, and the empty string is the root.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathError {
    #[error("invalid path component: {0:?}")]
    InvalidComponent(String),
    #[error("path too long")]
    TooLong,
}

/// Normalise a user-supplied path: strips leading/trailing/duplicate slashes,
/// rejects `.`/`..`, control characters and over-long names.
pub fn normalize(input: &str) -> Result<String, PathError> {
    if input.len() > 4096 {
        return Err(PathError::TooLong);
    }
    let mut out: Vec<&str> = Vec::new();
    for part in input.split('/') {
        if part.is_empty() {
            continue;
        }
        validate_name(part)?;
        out.push(part);
    }
    Ok(out.join("/"))
}

/// Validate a single file or folder name.
pub fn validate_name(name: &str) -> Result<(), PathError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.contains('/')
        || name.chars().any(|c| c.is_control())
    {
        return Err(PathError::InvalidComponent(name.to_string()));
    }
    Ok(())
}

/// Join two normalised paths.
pub fn join(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_string(),
        (_, true) => a.to_string(),
        _ => format!("{a}/{b}"),
    }
}

/// Split a normalised path into (parent, name). The root has no parent.
pub fn split(path: &str) -> Option<(&str, &str)> {
    if path.is_empty() {
        return None;
    }
    Some(match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    })
}

/// If `path` equals `prefix` or lies below it, return the remainder.
pub fn strip_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        return Some(path);
    }
    if path == prefix {
        return Some("");
    }
    path.strip_prefix(prefix).and_then(|r| r.strip_prefix('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize("/a//b/").unwrap(), "a/b");
        assert_eq!(normalize("").unwrap(), "");
        assert!(normalize("a/../b").is_err());
        assert!(normalize("a/./b").is_err());
    }

    #[test]
    fn prefixes() {
        assert_eq!(strip_prefix("a/b/c", "a/b"), Some("c"));
        assert_eq!(strip_prefix("a/b", "a/b"), Some(""));
        assert_eq!(strip_prefix("a/bc", "a/b"), None);
        assert_eq!(strip_prefix("x", ""), Some("x"));
        assert_eq!(split("a/b/c"), Some(("a/b", "c")));
        assert_eq!(split("c"), Some(("", "c")));
    }
}
