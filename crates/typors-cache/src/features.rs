//! Runtime switches for the streaming pipeline and the persistent cache.
//!
//! Defaults are on; an explicit `off`/`0`/`false`/`no` turns a switch off. The
//! values are read once per process so a frame cannot see a half-changed mode.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    /// Viewport-first layout with background measurement.
    pub streaming: bool,
    /// Persistent cache under the resolved cache root.
    pub cache: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            streaming: true,
            cache: true,
        }
    }
}

impl Features {
    /// Parse `TYPORS_STREAMING` and `TYPORS_CACHE`.
    pub fn from_env() -> Self {
        Self {
            streaming: parse_flag(std::env::var("TYPORS_STREAMING").ok().as_deref(), true),
            cache: parse_flag(std::env::var("TYPORS_CACHE").ok().as_deref(), true),
        }
    }

    /// The process-wide switches, parsed on first use.
    pub fn global() -> Features {
        static FEATURES: OnceLock<Features> = OnceLock::new();
        *FEATURES.get_or_init(Features::from_env)
    }
}

/// `None` keeps the default; an off word disables, anything else enables.
pub fn parse_flag(value: Option<&str>, default: bool) -> bool {
    match value.map(str::trim) {
        None | Some("") => default,
        Some(text) => !matches!(
            text.to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no" | "disabled"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_words_disable_and_unknown_words_enable() {
        for word in ["off", "OFF", "0", "false", "no", "disabled"] {
            assert!(!parse_flag(Some(word), true), "{word}");
        }
        for word in ["on", "1", "true", "yes", "anything"] {
            assert!(parse_flag(Some(word), false), "{word}");
        }
        assert!(parse_flag(None, true));
        assert!(!parse_flag(None, false));
        assert!(parse_flag(Some(""), true));
    }

    #[test]
    fn defaults_are_on() {
        assert_eq!(Features::default(), Features { streaming: true, cache: true });
    }
}
