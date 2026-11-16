//! Themes: built-in Dark, or Follow Omarchy, which reads the active Omarchy theme's
//! `colors.toml` and re-reads it whenever `omarchy theme set` bumps `theme.generation`.
//!
//! The choice is a per-user UI preference, saved in `$XDG_CONFIG_HOME/airbend/ui.toml`; it never
//! goes to the daemon.

use std::fs;
use std::path::PathBuf;

use rune::Color;
use rune_kit::{Kit, PaletteSeed};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    FollowOmarchy,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 2] = [ThemeChoice::FollowOmarchy, ThemeChoice::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::FollowOmarchy => "Follow Omarchy",
            ThemeChoice::Dark => "Dark",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: ThemeChoice,
    pub hide_unused_outputs: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            theme: if omarchy_dir().is_some() { ThemeChoice::FollowOmarchy } else { ThemeChoice::Dark },
            hide_unused_outputs: false,
        }
    }
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

fn prefs_path() -> Option<PathBuf> {
    Some(config_dir()?.join("airbend/ui.toml"))
}

impl Prefs {
    pub fn load() -> Prefs {
        prefs_path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = prefs_path().ok_or_else(|| anyhow::anyhow!("no config directory"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// The colours airbend takes from an Omarchy theme.
#[derive(Clone, Debug, PartialEq)]
pub struct OmarchyTheme {
    pub name: String,
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    /// Warnings and the live-temperature marker.
    pub warm: Color,
}

#[derive(Deserialize)]
struct ColorsToml {
    background: String,
    foreground: String,
    accent: Option<String>,
    blue: Option<String>,
    yellow: Option<String>,
    red: Option<String>,
}

/// `~/.local/state/omarchy/current`, when Omarchy is installed.
fn omarchy_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    let dir = base.join("omarchy/current");
    dir.join("theme/colors.toml").exists().then_some(dir)
}

/// A token that changes whenever the Omarchy theme does.
pub fn omarchy_generation() -> Option<String> {
    let dir = omarchy_dir()?;
    let generation = fs::read_to_string(dir.join("theme.generation")).unwrap_or_default();
    let mtime = fs::metadata(dir.join("theme/colors.toml")).and_then(|m| m.modified()).ok();
    Some(format!("{}:{mtime:?}", generation.trim()))
}

pub fn load_omarchy() -> Option<OmarchyTheme> {
    let dir = omarchy_dir()?;
    let raw: ColorsToml = toml::from_str(&fs::read_to_string(dir.join("theme/colors.toml")).ok()?).ok()?;
    let name = fs::read_to_string(dir.join("theme.name")).map(|s| s.trim().to_string()).unwrap_or_default();
    parse(&raw, name)
}

fn parse(raw: &ColorsToml, name: String) -> Option<OmarchyTheme> {
    let c = |s: &Option<String>| s.as_deref().and_then(rune::color::hex);
    let bg = rune::color::hex(&raw.background)?;
    let fg = rune::color::hex(&raw.foreground)?;
    let accent = c(&raw.accent).or_else(|| c(&raw.blue)).unwrap_or(fg);
    let warm = c(&raw.yellow).or_else(|| c(&raw.red)).unwrap_or(Color { r: 1.0, g: 0.55, b: 0.3, a: 1.0 });
    Some(OmarchyTheme { name, bg, fg, accent, warm })
}

/// Parses an Omarchy `colors.toml` body (for tests and demo scenes).
pub fn parse_colors(toml_body: &str, name: &str) -> Option<OmarchyTheme> {
    parse(&toml::from_str(toml_body).ok()?, name.into())
}

/// What the view paints with.
pub struct Look {
    pub kit: Kit,
    pub accent: Color,
    pub warm: Color,
    pub ok: Color,
    pub bad: Color,
}

pub fn look(choice: ThemeChoice, omarchy: Option<&OmarchyTheme>) -> Look {
    let ok = Color { r: 0.3, g: 0.8, b: 0.45, a: 1.0 };
    let bad = Color { r: 0.95, g: 0.38, b: 0.33, a: 1.0 };
    match (choice, omarchy) {
        (ThemeChoice::FollowOmarchy, Some(t)) => {
            let mut kit = Kit::from_palette(PaletteSeed { bg: t.bg, fg: t.fg, accent: t.accent });
            // Switches and accent badges take the theme's accent rather than kit blue.
            kit.c.bg_accent = t.accent;
            kit.c.text_accent = t.accent;
            Look { kit, accent: t.accent, warm: t.warm, ok, bad }
        }
        _ => Look {
            kit: {
                let mut kit = Kit::neutral(true);
                kit.c.text_accent = rune::tok::accent(0, true);
                kit
            },
            accent: rune::tok::accent(0, true),
            warm: Color { r: 1.0, g: 0.55, b: 0.3, a: 1.0 },
            ok,
            bad,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_omarchy_colors_file() {
        let t = parse_colors(
            "mode = \"dark\"\naccent = \"#b59790\"\nbackground = \"#0c0b0c\"\nforeground = \"#FAFCFB\"\nyellow = \"#6B5E73\"\n",
            "last-horizon",
        )
        .unwrap();
        assert!(t.accent.hex().to_lowercase().starts_with("#b59790"));
        assert!(t.fg.luminance() > t.bg.luminance());
    }

    #[test]
    fn falls_back_to_dark_without_omarchy() {
        let l = look(ThemeChoice::FollowOmarchy, None);
        assert!(l.kit.dark);
    }
}
