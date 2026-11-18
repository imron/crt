//! Application configuration loaded from `~/.crt/config.toml`.
//!
//! All visual styling is driven by [`StyleConfig`] so that colors are
//! never hardcoded in the rendering code. Every field has a sensible
//! default so the config file is entirely optional — any subset of keys
//! can be specified and the rest will fall back to defaults.

use std::ops::Deref;
use std::path::PathBuf;

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Color newtype
// ---------------------------------------------------------------------------

/// A color value that deserializes from TOML strings and derefs to
/// `ratatui::style::Color`.
///
/// Supported formats:
/// - Named colors: `"red"`, `"green"`, `"cyan"`, `"dark_gray"`, etc.
/// - Hex RGB: `"#RRGGBB"` (e.g. `"#23303A"`)
/// - 256-color index: `"238"` (bare integer as string)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub ratatui::style::Color);

impl Color {
    pub const fn new(c: ratatui::style::Color) -> Self {
        Self(c)
    }
}

impl Deref for Color {
    type Target = ratatui::style::Color;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Color> for ratatui::style::Color {
    fn from(c: Color) -> Self {
        c.0
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        parse_color(&s).map(Color).map_err(serde::de::Error::custom)
    }
}

fn parse_color(s: &str) -> Result<ratatui::style::Color, String> {
    use ratatui::style::Color as C;

    // Hex: #RRGGBB
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() != 6 {
            return Err(format!("invalid hex color: {s} (expected #RRGGBB)"));
        }
        let r =
            u8::from_str_radix(&hex[0..2], 16).map_err(|_| format!("invalid hex color: {s}"))?;
        let g =
            u8::from_str_radix(&hex[2..4], 16).map_err(|_| format!("invalid hex color: {s}"))?;
        let b =
            u8::from_str_radix(&hex[4..6], 16).map_err(|_| format!("invalid hex color: {s}"))?;
        return Ok(C::Rgb(r, g, b));
    }

    // 256-color index (bare integer).
    if let Ok(n) = s.parse::<u8>() {
        return Ok(C::Indexed(n));
    }

    // Named colors.
    match s.to_lowercase().as_str() {
        "black" => Ok(C::Black),
        "red" => Ok(C::Red),
        "green" => Ok(C::Green),
        "yellow" => Ok(C::Yellow),
        "blue" => Ok(C::Blue),
        "magenta" => Ok(C::Magenta),
        "cyan" => Ok(C::Cyan),
        "gray" | "grey" => Ok(C::Gray),
        "dark_gray" | "dark_grey" | "darkgray" | "darkgrey" => Ok(C::DarkGray),
        "light_red" | "lightred" => Ok(C::LightRed),
        "light_green" | "lightgreen" => Ok(C::LightGreen),
        "light_blue" | "lightblue" => Ok(C::LightBlue),
        "light_cyan" | "lightcyan" => Ok(C::LightCyan),
        "light_yellow" | "lightyellow" => Ok(C::LightYellow),
        "light_magenta" | "lightmagenta" => Ok(C::LightMagenta),
        "white" => Ok(C::White),
        "reset" => Ok(C::Reset),
        _ => Err(format!("unknown color: {s}")),
    }
}

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

/// Root configuration structure, mirrors `~/.crt/config.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub style: StyleConfig,
    pub layout: LayoutConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            style: StyleConfig::default(),
            layout: LayoutConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Diff algorithm
// ---------------------------------------------------------------------------

/// Available diff algorithms. Myers, patience, and minimal are handled
/// in-process by libgit2. Histogram requires shelling out to git.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffAlgorithm {
    /// Standard Myers diff (git default).
    Myers,
    /// Patience diff — better for structural changes.
    Patience,
    /// Minimal diff — spends extra time to find smallest diff.
    Minimal,
    /// Histogram diff — similar to patience but faster on some inputs.
    /// Requires shelling out to `git diff`.
    Histogram,
}

impl DiffAlgorithm {
    /// Cycle to the next algorithm.
    pub fn next(self) -> Self {
        match self {
            Self::Myers => Self::Patience,
            Self::Patience => Self::Minimal,
            Self::Minimal => Self::Histogram,
            Self::Histogram => Self::Myers,
        }
    }

    /// Short label for display in the title bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Myers => "myers",
            Self::Patience => "patience",
            Self::Minimal => "minimal",
            Self::Histogram => "histogram",
        }
    }
}

impl<'de> Deserialize<'de> for DiffAlgorithm {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "myers" => Ok(Self::Myers),
            "patience" => Ok(Self::Patience),
            "minimal" => Ok(Self::Minimal),
            "histogram" => Ok(Self::Histogram),
            _ => Err(serde::de::Error::custom(format!(
                "unknown diff algorithm: {s} (expected myers, patience, minimal, or histogram)"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Layout config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    /// Maximum width of the file list pane in columns.
    pub file_list_width: u16,
    /// Diff algorithm. `None` means "use git config, then fall back to patience".
    pub diff_algorithm: Option<DiffAlgorithm>,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            file_list_width: 40,
            diff_algorithm: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Style config
// ---------------------------------------------------------------------------

/// All visual styling for the TUI.
///
/// Organized into logical sections that mirror the TOML layout:
///
/// ```toml
/// [style.diff]
/// addition_bg = "#23303A"
/// addition_fg = "green"
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StyleConfig {
    /// Application background color.
    pub bg: Color,
    pub diff: DiffStyle,
    pub files: FilesStyle,
    pub panel: PanelStyle,
    pub status: StatusStyle,
    pub help: HelpStyle,
    pub selection: SelectionStyle,
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self {
            bg: Color::new(C::Rgb(0, 0, 0)), // #000000 pitch black
            diff: DiffStyle::default(),
            files: FilesStyle::default(),
            panel: PanelStyle::default(),
            status: StatusStyle::default(),
            help: HelpStyle::default(),
            selection: SelectionStyle::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Diff colors
// ---------------------------------------------------------------------------

/// Convenience alias to keep defaults readable.
use ratatui::style::Color as C;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DiffStyle {
    /// Addition line foreground.
    pub addition_fg: Color,
    /// Addition line background.
    pub addition_bg: Color,
    /// Emphasized addition background (word-level change highlight).
    pub addition_emphasis_bg: Color,
    /// Deletion line foreground.
    pub deletion_fg: Color,
    /// Deletion line background.
    pub deletion_bg: Color,
    /// Emphasized deletion background (word-level change highlight).
    pub deletion_emphasis_bg: Color,
    /// Context / unchanged line foreground.
    pub context_fg: Color,
    /// Line-number gutter foreground.
    pub gutter_fg: Color,
    /// Blame annotation foreground.
    pub blame_fg: Color,
    /// Placeholder text (binary file, empty file, etc.).
    pub placeholder_fg: Color,
    /// Reviewed file summary text.
    pub reviewed_fg: Color,
    /// Background for the current cursor line in the diff.
    pub cursor_line_bg: Color,
}

impl Default for DiffStyle {
    fn default() -> Self {
        Self {
            addition_fg: Color::new(C::Green),
            addition_bg: Color::new(C::Rgb(35, 48, 58)), // #23303A
            addition_emphasis_bg: Color::new(C::Rgb(50, 70, 85)), // #324655 — brighter
            deletion_fg: Color::new(C::Red),
            deletion_bg: Color::new(C::Rgb(52, 35, 44)), // #34232C
            deletion_emphasis_bg: Color::new(C::Rgb(75, 50, 60)), // #4B323C — brighter
            context_fg: Color::new(C::Gray),
            gutter_fg: Color::new(C::DarkGray),
            blame_fg: Color::new(C::Rgb(140, 140, 160)), // #8C8CA0 — blue-grey
            placeholder_fg: Color::new(C::DarkGray),
            reviewed_fg: Color::new(C::Green),
            cursor_line_bg: Color::new(C::Rgb(50, 50, 65)), // #323241 — subtle highlight
        }
    }
}

// ---------------------------------------------------------------------------
// File list colors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FilesStyle {
    /// Section header separator line.
    pub separator_fg: Color,
    /// Selected file path (when pane is focused).
    pub selected_fg: Color,
    /// Selected file path (when pane is unfocused).
    pub selected_unfocused_fg: Color,
    /// Unselected file path.
    pub text_fg: Color,
    /// Change-kind indicator (+, -, R).
    pub kind_fg: Color,
    /// Unreviewed marker color.
    pub unreviewed_fg: Color,
    /// Reviewed marker color.
    pub reviewed_fg: Color,
    /// Changed marker color.
    pub changed_fg: Color,
}

impl Default for FilesStyle {
    fn default() -> Self {
        Self {
            separator_fg: Color::new(C::DarkGray),
            selected_fg: Color::new(C::Yellow),
            selected_unfocused_fg: Color::new(C::White),
            text_fg: Color::new(C::Gray),
            kind_fg: Color::new(C::DarkGray),
            unreviewed_fg: Color::new(C::Red),
            reviewed_fg: Color::new(C::Green),
            changed_fg: Color::new(C::Yellow),
        }
    }
}

// ---------------------------------------------------------------------------
// Panel / border colors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PanelStyle {
    /// Pane border when focused.
    pub focused_fg: Color,
    /// Pane border when unfocused.
    pub unfocused_fg: Color,
}

impl Default for PanelStyle {
    fn default() -> Self {
        Self {
            focused_fg: Color::new(C::Cyan),
            unfocused_fg: Color::new(C::DarkGray),
        }
    }
}

// ---------------------------------------------------------------------------
// Status bar colors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StatusStyle {
    /// Default bar foreground.
    pub bar_fg: Color,
    /// Default bar background.
    pub bar_bg: Color,
    /// Warning (Ctrl-C) bar foreground.
    pub warning_fg: Color,
    /// Warning (Ctrl-C) bar background.
    pub warning_bg: Color,
    /// Transient message foreground.
    pub message_fg: Color,
    /// Transient message background.
    pub message_bg: Color,
    /// Help hint foreground.
    pub hint_fg: Color,
    /// Help hint background.
    pub hint_bg: Color,
}

impl Default for StatusStyle {
    fn default() -> Self {
        Self {
            bar_fg: Color::new(C::White),
            bar_bg: Color::new(C::DarkGray),
            warning_fg: Color::new(C::White),
            warning_bg: Color::new(C::Red),
            message_fg: Color::new(C::Black),
            message_bg: Color::new(C::Yellow),
            hint_fg: Color::new(C::Gray),
            hint_bg: Color::new(C::DarkGray),
        }
    }
}

// ---------------------------------------------------------------------------
// Help overlay colors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HelpStyle {
    /// Help box border and title.
    pub border_fg: Color,
    /// Help text foreground.
    pub text_fg: Color,
    /// Help box background fill.
    pub bg: Color,
    /// Background dimming color applied to all cells behind the overlay.
    pub dim_fg: Color,
}

impl Default for HelpStyle {
    fn default() -> Self {
        Self {
            border_fg: Color::new(C::Cyan),
            text_fg: Color::new(C::White),
            bg: Color::new(C::Black),
            dim_fg: Color::new(C::DarkGray),
        }
    }
}

// ---------------------------------------------------------------------------
// Mouse selection colors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SelectionStyle {
    /// Selection highlight foreground.
    pub fg: Color,
    /// Selection highlight background.
    pub bg: Color,
}

impl Default for SelectionStyle {
    fn default() -> Self {
        Self {
            fg: Color::new(C::White),
            bg: Color::new(C::Indexed(238)),
        }
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Load configuration from `~/.crt/config.toml`, falling back to defaults
/// if the file does not exist or is unparseable.
pub fn load() -> Config {
    match config_path() {
        Some(path) if path.exists() => match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<Config>(&content) {
                Ok(cfg) => cfg,
                Err(e) => {
                    eprintln!("warning: failed to parse {}: {e}", path.display());
                    Config::default()
                }
            },
            Err(e) => {
                eprintln!("warning: failed to read {}: {e}", path.display());
                Config::default()
            }
        },
        _ => Config::default(),
    }
}

/// Return `~/.crt/config.toml` if `$HOME` is set.
pub fn config_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".crt").join("config.toml"))
}

/// Persist the layout section of the config file without disturbing
/// other sections. Reads the existing file (if any), updates or inserts
/// `[layout]`, and writes it back.
pub fn save_layout(path: &std::path::Path, layout: &LayoutConfig) {
    // Read existing content.
    let existing = std::fs::read_to_string(path).unwrap_or_default();

    // Parse as a TOML table so we can surgically update [layout].
    let mut doc: toml::Table = toml::from_str(&existing).unwrap_or_default();

    let mut layout_table = toml::Table::new();
    layout_table.insert(
        "file_list_width".into(),
        toml::Value::Integer(layout.file_list_width as i64),
    );
    if let Some(algo) = layout.diff_algorithm {
        layout_table.insert(
            "diff_algorithm".into(),
            toml::Value::String(algo.label().to_string()),
        );
    }
    doc.insert("layout".into(), toml::Value::Table(layout_table));

    if let Ok(content) = toml::to_string_pretty(&doc) {
        // Ensure parent directory exists.
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, content);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_color() {
        assert_eq!(parse_color("#23303A").unwrap(), C::Rgb(35, 48, 58));
        assert_eq!(parse_color("#FF0000").unwrap(), C::Rgb(255, 0, 0));
        assert_eq!(parse_color("#000000").unwrap(), C::Rgb(0, 0, 0));
    }

    #[test]
    fn test_parse_named_colors() {
        assert_eq!(parse_color("red").unwrap(), C::Red);
        assert_eq!(parse_color("Green").unwrap(), C::Green);
        assert_eq!(parse_color("dark_gray").unwrap(), C::DarkGray);
        assert_eq!(parse_color("darkgrey").unwrap(), C::DarkGray);
        assert_eq!(parse_color("CYAN").unwrap(), C::Cyan);
    }

    #[test]
    fn test_parse_indexed_color() {
        assert_eq!(parse_color("238").unwrap(), C::Indexed(238));
        assert_eq!(parse_color("0").unwrap(), C::Indexed(0));
        assert_eq!(parse_color("255").unwrap(), C::Indexed(255));
    }

    #[test]
    fn test_parse_invalid_color() {
        assert!(parse_color("not_a_color").is_err());
        assert!(parse_color("#GG0000").is_err());
        assert!(parse_color("#12345").is_err());
    }

    #[test]
    fn test_default_config() {
        let cfg = Config::default();
        assert_eq!(*cfg.style.diff.addition_fg, C::Green);
        assert_eq!(*cfg.style.diff.addition_bg, C::Rgb(35, 48, 58));
        assert_eq!(*cfg.style.diff.deletion_fg, C::Red);
        assert_eq!(*cfg.style.diff.deletion_bg, C::Rgb(52, 35, 44));
        assert_eq!(*cfg.style.panel.focused_fg, C::Cyan);
    }

    #[test]
    fn test_partial_toml_override() {
        let toml_str = r##"
[style.diff]
addition_bg = "#112233"
"##;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        // Overridden value.
        assert_eq!(*cfg.style.diff.addition_bg, C::Rgb(0x11, 0x22, 0x33));
        // Default values preserved.
        assert_eq!(*cfg.style.diff.addition_fg, C::Green);
        assert_eq!(*cfg.style.diff.deletion_fg, C::Red);
        assert_eq!(*cfg.style.panel.focused_fg, C::Cyan);
    }

    #[test]
    fn test_full_toml_roundtrip() {
        let toml_str = r##"
[style.diff]
addition_fg = "green"
addition_bg = "#23303A"
deletion_fg = "red"
deletion_bg = "#34232C"
context_fg = "gray"
gutter_fg = "dark_gray"
placeholder_fg = "dark_gray"
reviewed_fg = "green"

[style.files]
separator_fg = "dark_gray"
selected_fg = "yellow"
selected_unfocused_fg = "white"
text_fg = "gray"
kind_fg = "dark_gray"
unreviewed_fg = "red"
reviewed_fg = "green"
changed_fg = "yellow"

[style.panel]
focused_fg = "cyan"
unfocused_fg = "dark_gray"

[style.status]
bar_fg = "white"
bar_bg = "dark_gray"
warning_fg = "white"
warning_bg = "red"
message_fg = "black"
message_bg = "yellow"
hint_fg = "gray"
hint_bg = "dark_gray"

[style.help]
border_fg = "cyan"
text_fg = "white"
bg = "black"
dim_fg = "dark_gray"

[style.selection]
fg = "white"
bg = "238"
"##;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(*cfg.style.diff.addition_bg, C::Rgb(35, 48, 58));
        assert_eq!(*cfg.style.selection.bg, C::Indexed(238));
    }

    #[test]
    fn test_color_deref() {
        let c = Color::new(C::Cyan);
        // Deref to ratatui::style::Color.
        let rc: ratatui::style::Color = *c;
        assert_eq!(rc, C::Cyan);
        // Into conversion.
        let rc2: ratatui::style::Color = c.into();
        assert_eq!(rc2, C::Cyan);
    }
}
