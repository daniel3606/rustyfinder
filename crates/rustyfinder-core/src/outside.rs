//! Requests from outside the app: the `rustyfinder://` URL scheme, the
//! "Reveal in RustyFinder" service and folders dropped on the Dock icon.

/// What another app asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutsideRequest {
    /// Show this folder.
    Open(String),
    /// Show the folder holding this item, with the item selected.
    Reveal(String),
    /// Open a favorite (any kind) by id.
    Favorite(u64),
    /// Restore a workspace by id.
    Workspace(u64),
}

fn decode(text: &str) -> String {
    // Percent-decoding by hand: only %XX escapes are special; a stray '%'
    // is kept as it is.
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        match escaped {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read a `rustyfinder://` URL:
///
/// - `rustyfinder://open?path=/Users/me/Projects`
/// - `rustyfinder://reveal?path=/Users/me/notes.txt`
/// - `rustyfinder://favorite/12`
/// - `rustyfinder://workspace/3`
///
/// Paths must be absolute; anything else is refused (`None`).
pub fn parse_app_url(url: &str) -> Option<OutsideRequest> {
    let rest = url.strip_prefix("rustyfinder://").or_else(|| url.strip_prefix("rustyfinder:"))?;
    let (head, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut parts = head.trim_matches('/').splitn(2, '/');
    let action = parts.next().unwrap_or_default().to_ascii_lowercase();
    let arg = parts.next().unwrap_or_default();
    let param = |name: &str| {
        query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == name).then(|| decode(v))
        })
    };
    let path = || param("path").filter(|p| p.starts_with('/')).map(|p| crate::paths::normalize(&p, "/"));
    match action.as_str() {
        "open" => path().map(OutsideRequest::Open),
        "reveal" => path().map(OutsideRequest::Reveal),
        "favorite" => arg.parse().ok().or_else(|| param("id")?.parse().ok()).map(OutsideRequest::Favorite),
        "workspace" => arg.parse().ok().or_else(|| param("id")?.parse().ok()).map(OutsideRequest::Workspace),
        _ => None,
    }
}

/// The URL that opens a favorite, for "Copy Link".
pub fn favorite_url(id: u64) -> String {
    format!("rustyfinder://favorite/{id}")
}

/// What the service was handed, one entry per line or item: file URLs,
/// plain paths (quoted or `~`-relative) and `rustyfinder://` links. A
/// folder is opened, anything else revealed; `is_dir` answers which.
pub fn service_requests(items: &[String], home: &str, is_dir: impl Fn(&str) -> bool) -> Vec<OutsideRequest> {
    let mut out = Vec::new();
    for item in items.iter().flat_map(|i| i.lines()) {
        let text = item.trim();
        if text.is_empty() {
            continue;
        }
        if let Some(request) = parse_app_url(text) {
            out.push(request);
            continue;
        }
        let path = if let Some(rest) = text.strip_prefix("file://") {
            decode(rest.strip_prefix("localhost").unwrap_or(rest))
        } else {
            text.to_string()
        };
        let unquoted = path.trim_matches('\'');
        if !(unquoted.starts_with('/') || unquoted.starts_with('~')) {
            continue;
        }
        let path = crate::goto::parse_typed(&path, "/", home);
        out.push(if is_dir(&path) { OutsideRequest::Open(path) } else { OutsideRequest::Reveal(path) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_urls_open_reveal_and_name_favorites() {
        assert_eq!(
            parse_app_url("rustyfinder://open?path=/Users/me/My%20Docs"),
            Some(OutsideRequest::Open("/Users/me/My Docs".into()))
        );
        assert_eq!(
            parse_app_url("rustyfinder://reveal?path=/tmp/a.txt&x=1"),
            Some(OutsideRequest::Reveal("/tmp/a.txt".into()))
        );
        assert_eq!(parse_app_url("rustyfinder://favorite/12"), Some(OutsideRequest::Favorite(12)));
        assert_eq!(parse_app_url("rustyfinder://favorite?id=5"), Some(OutsideRequest::Favorite(5)));
        assert_eq!(parse_app_url("rustyfinder://workspace/3"), Some(OutsideRequest::Workspace(3)));
        assert_eq!(parse_app_url(&favorite_url(9)), Some(OutsideRequest::Favorite(9)));
        assert_eq!(parse_app_url("rustyfinder://open?path=relative"), None);
        assert_eq!(parse_app_url("rustyfinder://favorite/abc"), None);
        assert_eq!(parse_app_url("rustyfinder://delete?path=/"), None);
        assert_eq!(parse_app_url("https://example.org"), None);
        assert_eq!(parse_app_url("rustyfinder://open?path=/a/../b"), Some(OutsideRequest::Open("/b".into())));
        assert_eq!(parse_app_url("rustyfinder://open?path=/a%zz"), Some(OutsideRequest::Open("/a%zz".into())));
    }

    #[test]
    fn the_service_takes_file_urls_paths_and_links() {
        let is_dir = |p: &str| p.ends_with("dir");
        let items = vec![
            "file:///tmp/some%20dir/".to_string(),
            "/tmp/file.txt\n~/dir".to_string(),
            "'/tmp/quoted dir'".to_string(),
            "not a path".to_string(),
            "rustyfinder://favorite/4".to_string(),
        ];
        assert_eq!(
            service_requests(&items, "/Users/me", is_dir),
            vec![
                OutsideRequest::Open("/tmp/some dir".into()),
                OutsideRequest::Reveal("/tmp/file.txt".into()),
                OutsideRequest::Open("/Users/me/dir".into()),
                OutsideRequest::Open("/tmp/quoted dir".into()),
                OutsideRequest::Favorite(4),
            ]
        );
    }
}
