//! The hoshi-web client at `/ui/`: the release embedded by `--features ui`
//! (pinned in `hoshi-web.pin`), or a directory given with `--ui-dir` while
//! developing the client.

use std::borrow::Cow;
use std::path::PathBuf;

// `FILES`: bundle-relative path and bytes, empty without the `ui` feature.
include!(concat!(env!("OUT_DIR"), "/ui_files.rs"));

/// Same-origin only and no inline script. Inline styles are allowed because the
/// client's terminal view sizes and themes itself with generated `<style>`.
pub const CSP: &str =
    "default-src 'self'; style-src 'self' 'unsafe-inline'; base-uri 'self'; frame-ancestors 'none'";

/// What `/ui/<rel>` answers.
pub enum Reply {
    File(Asset),
    /// A relative `Location` back to a route the client can load.
    Redirect(String),
}

/// One served file.
pub struct Asset {
    pub content_type: &'static str,
    pub cache_control: &'static str,
    pub body: Cow<'static, [u8]>,
}

/// Where the client files come from.
pub struct Ui {
    dir: Option<PathBuf>,
}

impl Ui {
    /// `dir` when given, else the embedded bundle; `None` when there is neither.
    pub fn new(dir: Option<PathBuf>) -> Option<Self> {
        (dir.is_some() || !FILES.is_empty()).then_some(Self { dir })
    }

    /// The reply for `rel` (the path after `ui/`); `None` is 404. A route
    /// directly under `ui/` without an extension gets `index.html` for client
    /// routing. The page links `./assets/...` and its service worker caches
    /// one shell for every route, so deeper paths are redirected (relative,
    /// prefix-safe) instead of served a page whose links would not resolve.
    pub fn get(&self, rel: &str) -> Option<Reply> {
        let rel = if rel.is_empty() { "index.html" } else { rel };
        if let Some(body) = self.file(rel) {
            return Some(Reply::File(asset(rel, body)));
        }
        let last = rel.rsplit('/').next().unwrap_or(rel);
        if last.contains('.') {
            return None;
        }
        let depth = rel.matches('/').count();
        if depth == 0 {
            let index = self.file("index.html")?;
            return Some(Reply::File(asset("index.html", index)));
        }
        let up = "../".repeat(depth);
        Some(Reply::Redirect(match rel.strip_suffix('/') {
            Some(route) if !route.contains('/') => format!("{up}{route}"),
            _ => up,
        }))
    }

    fn file(&self, rel: &str) -> Option<Cow<'static, [u8]>> {
        let safe = rel.split('/').all(|part| {
            !part.is_empty() && !part.starts_with('.') && !part.contains(['\\', ':', '\0'])
        });
        if !safe {
            return None;
        }
        match &self.dir {
            Some(dir) => {
                let path = dir.join(rel);
                path.is_file()
                    .then(|| std::fs::read(path).ok().map(Cow::Owned))
                    .flatten()
            }
            None => FILES
                .iter()
                .find(|(name, _)| *name == rel)
                .map(|(_, bytes)| Cow::Borrowed(*bytes)),
        }
    }
}

fn asset(rel: &str, body: Cow<'static, [u8]>) -> Asset {
    let ext = rel.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    let content_type = match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        "json" | "map" => "application/json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    // Hashed build output never changes under a name; everything else may.
    let cache_control = if rel.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    Asset {
        content_type,
        cache_control,
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "ui")]
    fn file(ui: &Ui, rel: &str) -> Asset {
        match ui.get(rel) {
            Some(Reply::File(asset)) => asset,
            _ => panic!("{rel} is not a file"),
        }
    }

    fn redirect(ui: &Ui, rel: &str) -> String {
        match ui.get(rel) {
            Some(Reply::Redirect(location)) => location,
            _ => panic!("{rel} is not a redirect"),
        }
    }

    #[test]
    #[cfg(feature = "ui")]
    fn embedded_bundle_serves_the_client_without_files_on_disk() {
        let ui = Ui::new(None).expect("embedded bundle");
        let index = file(&ui, "");
        assert_eq!(index.content_type, "text/html; charset=utf-8");
        assert_eq!(
            file(&ui, "sw.js").content_type,
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            file(&ui, "manifest.webmanifest").content_type,
            "application/manifest+json"
        );
        assert_eq!(file(&ui, "cockpit").body, index.body);
        assert!(ui.get("assets/missing.js").is_none());
    }

    #[test]
    fn deeper_routes_redirect_up_to_the_client() {
        let ui = Ui::new(Some(PathBuf::from("/nonexistent"))).unwrap();
        assert_eq!(redirect(&ui, "terminal/"), "../terminal");
        assert_eq!(redirect(&ui, "colony/42"), "../");
        assert_eq!(redirect(&ui, "a/b/c/"), "../../../");
        assert!(ui.get("a/b/c.js").is_none());
    }

    #[test]
    fn no_client_without_the_feature_or_a_directory() {
        assert_eq!(Ui::new(None).is_some(), !FILES.is_empty());
        assert!(Ui::new(Some(PathBuf::from("."))).is_some());
    }
}
