//! Persisted typors preferences.
//!
//! Pure model + JSON persistence, deliberately independent of GPUI so defaults,
//! validation and corrupt-file recovery are unit-testable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_PAGE_WIDTH: f32 = 860.0;
pub const DEFAULT_FONT_FAMILY: &str = "Open Sans";
pub const DEFAULT_FONT_SIZE: f32 = 16.0;
pub const DEFAULT_LINE_HEIGHT: f32 = 1.6;
pub const DEFAULT_BLINK_MS: u32 = 530;
pub const DEFAULT_COMPLETION_DELAY_MS: u32 = 150;
pub const DEFAULT_CACHE_MAX_MB: u32 = 512;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(default)]
    pub files: Files,
    #[serde(default)]
    pub editing: Editing,
    #[serde(default)]
    pub shortcuts: BTreeMap<String, String>,
    #[serde(default)]
    pub language: Language,
}

fn default_version() -> u32 {
    SCHEMA_VERSION
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            appearance: Appearance::default(),
            files: Files::default(),
            editing: Editing::default(),
            shortcuts: default_shortcuts(),
            language: Language::System,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    Light,
    Dark,
    System,
}

impl Default for ThemeChoice {
    fn default() -> Self {
        Self::Light
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Appearance {
    #[serde(default)]
    pub theme: ThemeChoice,
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_line_height")]
    pub line_height: f32,
    #[serde(default = "default_page_width")]
    pub page_width: f32,
}

fn default_font_family() -> String {
    DEFAULT_FONT_FAMILY.to_string()
}
fn default_font_size() -> f32 {
    DEFAULT_FONT_SIZE
}
fn default_line_height() -> f32 {
    DEFAULT_LINE_HEIGHT
}
fn default_page_width() -> f32 {
    DEFAULT_PAGE_WIDTH
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Light,
            font_family: default_font_family(),
            font_size: DEFAULT_FONT_SIZE,
            line_height: DEFAULT_LINE_HEIGHT,
            page_width: DEFAULT_PAGE_WIDTH,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Files {
    #[serde(default)]
    pub autosave: Autosave,
    #[serde(default = "default_true")]
    pub prompt_on_close: bool,
    #[serde(default)]
    pub restore_last_session: bool,
    #[serde(default)]
    pub default_save_directory: Option<String>,
    /// Persist decoded images, chart/Mermaid rasters, math layouts and block
    /// geometry under the cache root.
    #[serde(default = "default_true")]
    pub cache_enabled: bool,
    /// Upper bound for the whole cache directory tree, in megabytes.
    #[serde(default = "default_cache_max_mb")]
    pub cache_max_mb: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Autosave {
    Off,
    Seconds30,
    Seconds60,
    OnFocusLoss,
}

impl Default for Autosave {
    fn default() -> Self {
        Self::Off
    }
}

impl Files {
    pub fn interval(&self) -> Option<std::time::Duration> {
        match self.autosave {
            Autosave::Seconds30 => Some(std::time::Duration::from_secs(30)),
            Autosave::Seconds60 => Some(std::time::Duration::from_secs(60)),
            Autosave::OnFocusLoss | Autosave::Off => None,
        }
    }
}

impl Default for Files {
    fn default() -> Self {
        Self {
            autosave: Autosave::Off,
            prompt_on_close: true,
            restore_last_session: false,
            default_save_directory: None,
            cache_enabled: true,
            cache_max_mb: DEFAULT_CACHE_MAX_MB,
        }
    }
}

fn default_cache_max_mb() -> u32 {
    DEFAULT_CACHE_MAX_MB
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Editing {
    #[serde(default = "default_true")]
    pub completion_enabled: bool,
    #[serde(default = "default_completion_delay")]
    pub completion_delay_ms: u32,
    #[serde(default = "default_true")]
    pub blink_enabled: bool,
    #[serde(default = "default_blink_ms")]
    pub blink_interval_ms: u32,
    #[serde(default = "default_true")]
    pub pair_brackets: bool,
}

fn default_completion_delay() -> u32 {
    DEFAULT_COMPLETION_DELAY_MS
}
fn default_blink_ms() -> u32 {
    DEFAULT_BLINK_MS
}

impl Default for Editing {
    fn default() -> Self {
        Self {
            completion_enabled: true,
            completion_delay_ms: DEFAULT_COMPLETION_DELAY_MS,
            blink_enabled: true,
            blink_interval_ms: DEFAULT_BLINK_MS,
            pair_brackets: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Zh,
    En,
    System,
}

impl Default for Language {
    fn default() -> Self {
        Self::System
    }
}

impl Language {
    pub fn resolved(self) -> Self {
        match self {
            Self::System => {
                let locale = std::env::var("LANG").unwrap_or_default();
                if locale.to_ascii_lowercase().starts_with("en") {
                    Self::En
                } else {
                    Self::Zh
                }
            }
            other => other,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub field: &'static str,
    pub message: String,
}

impl Settings {
    pub fn validate(&self) -> Vec<FieldError> {
        let mut errors = Vec::new();
        let a = &self.appearance;
        if a.font_family.trim().is_empty() {
            errors.push(err("font_family", "字体不能为空"));
        }
        if !(12.0..=24.0).contains(&a.font_size) {
            errors.push(err("font_size", "字号必须在 12–24 之间"));
        }
        if !(1.2..=2.0).contains(&a.line_height) {
            errors.push(err("line_height", "行高必须在 1.2–2.0 之间"));
        }
        if !(600.0..=1200.0).contains(&a.page_width) {
            errors.push(err("page_width", "页宽必须在 600–1200 之间"));
        }
        if self.editing.completion_delay_ms > 500 {
            errors.push(err("completion_delay_ms", "提示延迟必须在 0–500 ms 之间"));
        }
        if !(300..=1200).contains(&self.editing.blink_interval_ms) {
            errors.push(err("blink_interval_ms", "闪烁间隔必须在 300–1200 ms 之间"));
        }
        if self.version != SCHEMA_VERSION {
            errors.push(err("version", "无法识别的设置版本"));
        }
        if let Some(dir) = &self.files.default_save_directory
            && dir.trim().is_empty()
        {
            errors.push(err("default_save_directory", "默认保存目录不能为空"));
        }
        if !(64..=8192).contains(&self.files.cache_max_mb) {
            errors.push(err("cache_max_mb", "缓存上限必须在 64–8192 MB 之间"));
        }
        errors
    }

    /// Path used on this platform. `TYPORS_SETTINGS_PATH` overrides it for tests.
    pub fn path() -> PathBuf {
        if let Ok(path) = std::env::var("TYPORS_SETTINGS_PATH") {
            return PathBuf::from(path);
        }
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("typors").join("settings.json")
    }

    pub fn load() -> (Self, Option<String>) {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> (Self, Option<String>) {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!("无法读取设置 {}：{error}", path.display())),
                );
            }
        };
        match serde_json::from_str::<Settings>(&raw) {
            Ok(settings) => {
                let errors = settings.validate();
                if errors.is_empty() {
                    (settings, None)
                } else {
                    let warning = errors
                        .iter()
                        .map(|error| format!("{}: {}", error.field, error.message))
                        .collect::<Vec<_>>()
                        .join("; ");
                    (
                        Self::default(),
                        Some(format!("设置无效，已使用默认值：{warning}")),
                    )
                }
            }
            Err(error) => {
                let backup = path.with_extension("json.bak");
                let _ = std::fs::copy(path, &backup);
                (
                    Self::default(),
                    Some(format!(
                        "设置文件损坏，已使用默认值（备份：{}）：{error}",
                        backup.display()
                    )),
                )
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        let errors = self.validate();
        if !errors.is_empty() {
            return Err(errors
                .iter()
                .map(|e| format!("{}: {}", e.field, e.message))
                .collect::<Vec<_>>()
                .join("; "));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("无法创建设置目录 {}：{e}", parent.display()))?;
        }
        let json =
            serde_json::to_string_pretty(self).map_err(|e| format!("无法序列化设置：{e}"))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)
            .map_err(|e| format!("无法写入临时设置 {}：{e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("无法替换设置 {}：{e}", path.display()))
    }

    pub fn shortcut(&self, command: &str) -> Option<&str> {
        self.shortcuts.get(command).map(String::as_str)
    }

    pub fn set_shortcut(&mut self, command: &str, chord: &str) -> Result<(), String> {
        let chord = chord.trim();
        if chord.is_empty() {
            return Err("快捷键不能为空".to_string());
        }
        if let Some((other, _)) = self
            .shortcuts
            .iter()
            .find(|(key, value)| key.as_str() != command && value.eq_ignore_ascii_case(chord))
        {
            return Err(format!("快捷键已被“{other}”使用"));
        }
        self.shortcuts
            .insert(command.to_string(), chord.to_string());
        Ok(())
    }
}

fn err(field: &'static str, message: &str) -> FieldError {
    FieldError {
        field,
        message: message.to_string(),
    }
}

pub fn default_shortcuts() -> BTreeMap<String, String> {
    [
        ("New", "Ctrl+N"),
        ("Open", "Ctrl+O"),
        ("Save", "Ctrl+S"),
        ("SaveAs", "Ctrl+Shift+S"),
        ("Undo", "Ctrl+Z"),
        ("Redo", "Ctrl+Y"),
        ("Cut", "Ctrl+X"),
        ("Copy", "Ctrl+C"),
        ("Paste", "Ctrl+V"),
        ("SelectAll", "Ctrl+A"),
        ("Bold", "Ctrl+B"),
        ("Italic", "Ctrl+I"),
        ("InlineCode", "Ctrl+`"),
    ]
    .into_iter()
    .map(|(command, chord)| (command.to_string(), chord.to_string()))
    .collect()
}

pub fn is_dark(choice: ThemeChoice) -> bool {
    match choice {
        ThemeChoice::Dark => true,
        ThemeChoice::Light => false,
        ThemeChoice::System => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_typora_sized() {
        let settings = Settings::default();
        assert!(settings.validate().is_empty());
        assert_eq!(settings.appearance.font_family, "Open Sans");
        assert_eq!(settings.appearance.font_size, 16.0);
        assert_eq!(settings.appearance.line_height, 1.6);
        assert_eq!(settings.appearance.page_width, 860.0);
        assert_eq!(settings.editing.blink_interval_ms, 530);
        assert!(settings.editing.completion_enabled);
    }

    #[test]
    fn invalid_ranges_are_reported() {
        let mut settings = Settings::default();
        settings.appearance.font_size = 40.0;
        settings.appearance.line_height = 3.0;
        settings.appearance.page_width = 100.0;
        settings.editing.blink_interval_ms = 10;
        let errors = settings.validate();
        let fields: Vec<_> = errors.iter().map(|e| e.field).collect();
        assert!(fields.contains(&"font_size"));
        assert!(fields.contains(&"line_height"));
        assert!(fields.contains(&"page_width"));
        assert!(fields.contains(&"blink_interval_ms"));
    }

    #[test]
    fn round_trip_and_missing_fields_use_defaults() {
        let mut settings = Settings::default();
        settings.appearance.font_size = 18.0;
        settings.editing.completion_enabled = false;
        let json = serde_json::to_string(&settings).unwrap();
        let parsed: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, settings);

        let partial: Settings =
            serde_json::from_str(r#"{"appearance":{"font_size":20.0}}"#).unwrap();
        assert_eq!(partial.appearance.font_size, 20.0);
        assert_eq!(partial.appearance.page_width, 860.0);
        assert!(partial.files.prompt_on_close);
    }

    #[test]
    fn cache_settings_default_validate_and_load_without_the_field() {
        let settings = Settings::default();
        assert!(settings.files.cache_enabled);
        assert_eq!(settings.files.cache_max_mb, DEFAULT_CACHE_MAX_MB);

        let mut too_small = Settings::default();
        too_small.files.cache_max_mb = 8;
        assert!(too_small.validate().iter().any(|e| e.field == "cache_max_mb"));
        let mut too_large = Settings::default();
        too_large.files.cache_max_mb = 100_000;
        assert!(too_large.validate().iter().any(|e| e.field == "cache_max_mb"));

        // An older settings.json has no cache fields at all.
        let legacy: Settings =
            serde_json::from_str(r#"{"version":1,"files":{"autosave":"off"}}"#).unwrap();
        assert!(legacy.files.cache_enabled);
        assert_eq!(legacy.files.cache_max_mb, DEFAULT_CACHE_MAX_MB);
    }

    #[test]
    fn shortcut_conflicts_are_rejected() {
        let mut settings = Settings::default();
        assert!(settings.set_shortcut("Save", "Ctrl+O").is_err());
        assert!(settings.set_shortcut("Save", "Ctrl+Alt+S").is_ok());
        assert_eq!(settings.shortcut("Save"), Some("Ctrl+Alt+S"));
    }
}
