//! What a non-`/api` request that matched no static asset gets.
//!
//! Static Assets answer every request for a file in `public/` before the worker runs. Whatever
//! reaches the worker outside `/api` matched nothing: a client-side route (`/account/orders/…`)
//! gets the SPA shell (`index.html`, `200`), but a missing *file* (`/assets/foo.js`, a stale
//! hashed bundle after a deploy) is a real `404`. Serving the shell for those would hand the
//! browser HTML as JavaScript, CSS or an image, with a `200` that hides the miss.

use axum::http::Method;

/// The answer to a request no static asset matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// Serve `index.html`: the SPA's router handles the path (including its own "not found").
    Shell,
    /// `404`: a file that does not exist, or a method no page answers.
    NotFound,
}

/// The fallback for a non-`/api` `path`: the shell for page navigations (`GET`/`HEAD` of an
/// extension-less path), `404` for everything else.
pub fn fallback(method: &Method, path: &str) -> Fallback {
    let last = path.rsplit('/').next().unwrap_or_default();
    if (method == Method::GET || method == Method::HEAD) && !last.contains('.') {
        Fallback::Shell
    } else {
        Fallback::NotFound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_get_the_shell_and_files_a_404() {
        for path in [
            "/",
            "/builder",
            "/account/orders/TD-2026-000123",
            "/admin/tickets/t_1/",
        ] {
            assert_eq!(fallback(&Method::GET, path), Fallback::Shell, "{path}");
        }
        assert_eq!(fallback(&Method::HEAD, "/cart"), Fallback::Shell);
        for path in [
            "/assets/foo.js",
            "/assets/techdepo-web-1a2b3c.wasm",
            "/favicon.ico",
            "/robots.txt",
            "/index.html",
        ] {
            assert_eq!(fallback(&Method::GET, path), Fallback::NotFound, "{path}");
        }
        // A dot in an earlier segment does not make the path a file.
        assert_eq!(fallback(&Method::GET, "/v1.2/page"), Fallback::Shell);
        assert_eq!(fallback(&Method::POST, "/checkout"), Fallback::NotFound);
    }
}
