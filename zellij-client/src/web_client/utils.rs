use axum::http::Request;
use axum_extra::extract::cookie::Cookie;
use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::path::{Component, PathBuf};

pub fn get_mime_type(ext: Option<&str>) -> &str {
    match ext {
        None => "text/plain",
        Some(ext) => match ext {
            "html" => "text/html",
            "css" => "text/css",
            "js" => "application/javascript",
            "wasm" => "application/wasm",
            "png" => "image/png",
            "ico" => "image/x-icon",
            "svg" => "image/svg+xml",
            _ => "text/plain",
        },
    }
}

pub fn should_use_https(
    ip: IpAddr,
    has_certificate: bool,
    enforce_https_for_localhost: bool,
) -> Result<bool, String> {
    let is_loopback = match ip {
        IpAddr::V4(ipv4) => ipv4.is_loopback(),
        IpAddr::V6(ipv6) => ipv6.is_loopback(),
    };

    if is_loopback && !enforce_https_for_localhost {
        Ok(has_certificate)
    } else if is_loopback {
        Err(format!("Cannot bind without an SSL certificate."))
    } else if has_certificate {
        Ok(true)
    } else {
        Err(format!(
            "Cannot bind to non-loopback IP: {} without an SSL certificate.",
            ip
        ))
    }
}

pub fn parse_cookies<T>(request: &Request<T>) -> HashMap<String, String> {
    let mut cookies = HashMap::new();

    for cookie_header in request.headers().get_all("cookie") {
        if let Ok(cookie_str) = cookie_header.to_str() {
            for cookie_part in cookie_str.split(';') {
                if let Ok(cookie) = Cookie::parse(cookie_part.trim()) {
                    cookies.insert(cookie.name().to_string(), cookie.value().to_string());
                }
            }
        }
    }

    cookies
}

/// Maximum length of a deep-link path in bytes (rejects degenerate
/// inputs and bounds the on-wire `cd '…'\n` payload).
pub const DEEP_LINK_PATH_MAX_LEN: usize = 4096;

/// Shell metacharacters rejected by `validate_deep_link_path`. Their
/// absence (combined with the single-quote and control-byte rejections)
/// lets the server build a fixed-shape `cd '<path>'\n` payload without
/// any per-shell escape pass.
const REJECTED_SHELL_METACHARS: &[char] = &[
    ';', '&', '|', '`', '$', '!', '<', '>', '*', '?', '(', ')', '{', '}', '\\', '"',
];

/// Why a deep-link `?path=` value was rejected. Variants are split so
/// the WebSocket handler can decide between a silent drop (shape-invalid
/// — looks like an attack probe) and a `LogError` surface to the
/// browser (semantically invalid — the user gets visible feedback in
/// the xterm area before the shell prompt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLinkPathError {
    Empty,
    NotAbsolute,
    TooLong,
    /// Any byte `< 0x20` — null, CR, LF, BEL, …. Silently dropped on
    /// the websocket_handlers side; logged only as a fingerprint.
    ControlCharacter,
    SingleQuote,
    ShellMetacharacter(char),
    ParentComponent,
}

impl DeepLinkPathError {
    /// True when the rejection looks like an attack probe and should be
    /// silently dropped (no `LogError` to the browser, no full path in
    /// logs). Semantically-invalid rejections (`NotAbsolute`,
    /// `TooLong`, `ShellMetacharacter`, `SingleQuote`,
    /// `ParentComponent`) return `false` so the user sees the reason.
    pub fn is_silent_drop(&self) -> bool {
        matches!(self, DeepLinkPathError::ControlCharacter)
    }
}

impl fmt::Display for DeepLinkPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeepLinkPathError::Empty => write!(f, "path is empty"),
            DeepLinkPathError::NotAbsolute => write!(f, "must be absolute"),
            DeepLinkPathError::TooLong => write!(
                f,
                "path exceeds {} bytes",
                DEEP_LINK_PATH_MAX_LEN
            ),
            DeepLinkPathError::ControlCharacter => write!(f, "contains a control character"),
            DeepLinkPathError::SingleQuote => write!(f, "must not contain a single quote"),
            DeepLinkPathError::ShellMetacharacter(c) => {
                write!(f, "must not contain shell metacharacter {:?}", c)
            },
            DeepLinkPathError::ParentComponent => {
                write!(f, "must not contain a '..' path component")
            },
        }
    }
}

/// Validate a deep-link `?path=` value supplied via the page URL.
///
/// Returns `Ok(Some(_))` for a validated absolute path,
/// `Ok(None)` only when called with an explicit empty string (the
/// websocket handler handles the unsupplied case before calling), and
/// `Err(_)` for any rejection. See `DeepLinkPathError::is_silent_drop`
/// for which errors should be surfaced to the user vs silently dropped.
///
/// The validator deliberately does NOT check existence — that is a
/// TOCTOU race against the eventual server-side spawn, and the server
/// already falls back to home directory when the cwd is unreachable.
pub fn validate_deep_link_path(raw: &str) -> Result<Option<PathBuf>, DeepLinkPathError> {
    if raw.is_empty() {
        return Err(DeepLinkPathError::Empty);
    }
    if raw.len() > DEEP_LINK_PATH_MAX_LEN {
        return Err(DeepLinkPathError::TooLong);
    }
    if raw.as_bytes().first() != Some(&b'/') {
        return Err(DeepLinkPathError::NotAbsolute);
    }
    // Scan bytes for control characters first — these look like attack
    // probes (null injection, CRLF, etc.) and should be silently
    // dropped without surfacing a reason to the user.
    if raw.bytes().any(|b| b < 0x20) {
        return Err(DeepLinkPathError::ControlCharacter);
    }
    if raw.contains('\'') {
        return Err(DeepLinkPathError::SingleQuote);
    }
    if let Some(c) = raw.chars().find(|c| REJECTED_SHELL_METACHARS.contains(c)) {
        return Err(DeepLinkPathError::ShellMetacharacter(c));
    }
    let path_buf = PathBuf::from(raw);
    if path_buf
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(DeepLinkPathError::ParentComponent);
    }
    Ok(Some(path_buf))
}

pub fn terminal_init_messages() -> Vec<&'static str> {
    let clear_client_terminal_attributes = "\u{1b}[?1l\u{1b}=\u{1b}[r\u{1b}[?1000l\u{1b}[?1002l\u{1b}[?1003l\u{1b}[?1005l\u{1b}[?1006l\u{1b}[?12l";
    let enter_alternate_screen = "\u{1b}[?1049h";
    let bracketed_paste = "\u{1b}[?2004h";
    let enter_kitty_keyboard_mode = "\u{1b}[>1u";
    let enable_mouse_mode = "\u{1b}[?1000h\u{1b}[?1002h\u{1b}[?1015h\u{1b}[?1006h";
    vec![
        clear_client_terminal_attributes,
        enter_alternate_screen,
        bracketed_paste,
        enter_kitty_keyboard_mode,
        enable_mouse_mode,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_deep_link_path_accepts_absolute_path() {
        assert_eq!(
            validate_deep_link_path("/tmp"),
            Ok(Some(PathBuf::from("/tmp")))
        );
        assert_eq!(
            validate_deep_link_path("/srv/projects/foo"),
            Ok(Some(PathBuf::from("/srv/projects/foo")))
        );
    }

    #[test]
    fn validate_deep_link_path_accepts_utf8_and_spaces() {
        // Spaces and non-ASCII UTF-8 are allowed — the validator only
        // rejects control bytes and the explicit shell-meta set.
        let p = "/home/user/with spaces and-éscapéd";
        assert_eq!(validate_deep_link_path(p), Ok(Some(PathBuf::from(p))));
    }

    #[test]
    fn validate_deep_link_path_rejects_empty_string() {
        assert_eq!(validate_deep_link_path(""), Err(DeepLinkPathError::Empty));
    }

    #[test]
    fn validate_deep_link_path_rejects_relative_path() {
        assert_eq!(
            validate_deep_link_path("relative/path"),
            Err(DeepLinkPathError::NotAbsolute)
        );
        // Tilde paths cannot be expanded on the frontend; reject them
        // here so the user sees a clear error rather than landing in a
        // literal `~/foo` directory.
        assert_eq!(
            validate_deep_link_path("~/foo"),
            Err(DeepLinkPathError::NotAbsolute)
        );
    }

    #[test]
    fn validate_deep_link_path_rejects_oversized_path() {
        let oversized = "/".to_string() + &"a".repeat(DEEP_LINK_PATH_MAX_LEN);
        assert_eq!(
            validate_deep_link_path(&oversized),
            Err(DeepLinkPathError::TooLong)
        );
    }

    #[test]
    fn validate_deep_link_path_rejects_control_characters() {
        // Each of these is a different attack-shape probe; all must be
        // silently dropped (no LogError to the browser).
        for input in [
            "/path/with\0null",
            "/path/with\nnewline",
            "/path/with\rreturn",
            "/path/with\x07bell",
            "/path/with\x1bescape",
        ] {
            let err = validate_deep_link_path(input).expect_err("should reject control");
            assert_eq!(err, DeepLinkPathError::ControlCharacter);
            assert!(err.is_silent_drop());
        }
    }

    #[test]
    fn validate_deep_link_path_rejects_single_quote() {
        assert_eq!(
            validate_deep_link_path("/path/with'quote"),
            Err(DeepLinkPathError::SingleQuote)
        );
    }

    #[test]
    fn validate_deep_link_path_rejects_shell_metacharacters() {
        for (c, input) in [
            (';', "/path/with;semicolon"),
            ('&', "/path/with&amp"),
            ('|', "/path/with|pipe"),
            ('`', "/path/with`backtick"),
            ('$', "/path/with$dollar"),
            ('!', "/path/with!bang"),
            ('<', "/path/with<lt"),
            ('>', "/path/with>gt"),
            ('*', "/path/with*star"),
            ('?', "/path/with?qmark"),
            ('(', "/path/with(paren"),
            (')', "/path/with)paren"),
            ('{', "/path/with{brace"),
            ('}', "/path/with}brace"),
            ('\\', "/path/with\\backslash"),
            ('"', "/path/with\"quote"),
        ] {
            assert_eq!(
                validate_deep_link_path(input),
                Err(DeepLinkPathError::ShellMetacharacter(c)),
                "input {:?}",
                input
            );
        }
    }

    #[test]
    fn validate_deep_link_path_rejects_parent_component() {
        assert_eq!(
            validate_deep_link_path("/foo/../bar"),
            Err(DeepLinkPathError::ParentComponent)
        );
        assert_eq!(
            validate_deep_link_path("/.."),
            Err(DeepLinkPathError::ParentComponent)
        );
    }

    #[test]
    fn validate_deep_link_path_treats_double_encoded_path_as_not_absolute() {
        // Axum decodes the URL-encoding once. A doubly-encoded `?path=%252Ftmp`
        // therefore reaches the validator as the literal string `%2Ftmp`,
        // which does not start with '/' and so trips NotAbsolute. This
        // is the expected behavior — the user gets a visible LogError.
        assert_eq!(
            validate_deep_link_path("%2Ftmp"),
            Err(DeepLinkPathError::NotAbsolute)
        );
    }

    #[test]
    fn deep_link_path_error_silent_drop_classification() {
        // Only ControlCharacter is treated as an attack probe; every
        // other rejection class surfaces a human-readable LogError.
        assert!(DeepLinkPathError::ControlCharacter.is_silent_drop());
        for err in [
            DeepLinkPathError::Empty,
            DeepLinkPathError::NotAbsolute,
            DeepLinkPathError::TooLong,
            DeepLinkPathError::SingleQuote,
            DeepLinkPathError::ShellMetacharacter(';'),
            DeepLinkPathError::ParentComponent,
        ] {
            assert!(!err.is_silent_drop(), "{:?} should surface to user", err);
        }
    }
}
