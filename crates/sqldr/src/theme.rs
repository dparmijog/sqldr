//! Color palettes for the TUI. A [`Theme`] only carries the handful of
//! semantic roles the UI actually uses (focused borders/selections, muted
//! chrome, and the running/error/success status colors) — never raw
//! per-widget colors — so every pane and overlay repaints consistently the
//! moment the user switches themes in the options dialog (`Ctrl+O`).

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    /// Focused pane borders, active selections, primary highlights.
    pub accent: Color,
    /// Unfocused pane borders, secondary/dim chrome.
    pub muted: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
}

const DARK: Theme = Theme {
    name: "Dark",
    accent: Color::Cyan,
    muted: Color::DarkGray,
    error: Color::Red,
    warning: Color::Yellow,
    success: Color::Green,
};

const DRACULA: Theme = Theme {
    name: "Dracula",
    accent: Color::Rgb(0xbd, 0x93, 0xf9),
    muted: Color::Rgb(0x62, 0x72, 0xa4),
    error: Color::Rgb(0xff, 0x55, 0x55),
    warning: Color::Rgb(0xf1, 0xfa, 0x8c),
    success: Color::Rgb(0x50, 0xfa, 0x7b),
};

const NORD: Theme = Theme {
    name: "Nord",
    accent: Color::Rgb(0x88, 0xc0, 0xd0),
    muted: Color::Rgb(0x4c, 0x56, 0x6a),
    error: Color::Rgb(0xbf, 0x61, 0x6a),
    warning: Color::Rgb(0xeb, 0xcb, 0x8b),
    success: Color::Rgb(0xa3, 0xbe, 0x8c),
};

const CATPPUCCIN_MOCHA: Theme = Theme {
    name: "Catppuccin Mocha",
    accent: Color::Rgb(0xcb, 0xa6, 0xf7),
    muted: Color::Rgb(0x6c, 0x70, 0x86),
    error: Color::Rgb(0xf3, 0x8b, 0xa8),
    warning: Color::Rgb(0xf9, 0xe2, 0xaf),
    success: Color::Rgb(0xa6, 0xe3, 0xa1),
};

const GRUVBOX_DARK: Theme = Theme {
    name: "Gruvbox Dark",
    accent: Color::Rgb(0x83, 0xa5, 0x98),
    muted: Color::Rgb(0x92, 0x83, 0x74),
    error: Color::Rgb(0xfb, 0x49, 0x34),
    warning: Color::Rgb(0xfa, 0xbd, 0x2f),
    success: Color::Rgb(0xb8, 0xbb, 0x26),
};

impl Theme {
    pub const ALL: &'static [Theme] = &[DARK, DRACULA, NORD, CATPPUCCIN_MOCHA, GRUVBOX_DARK];

    /// Looks up a theme by name (case-insensitive), falling back to the
    /// default when `name` doesn't match anything — an unknown value in
    /// `config.toml` (e.g. from an older/newer sqldr) should never prevent
    /// startup.
    pub fn by_name(name: &str) -> Theme {
        Self::ALL.iter().find(|t| t.name.eq_ignore_ascii_case(name)).copied().unwrap_or_default()
    }
}

impl Default for Theme {
    fn default() -> Self {
        DARK
    }
}
