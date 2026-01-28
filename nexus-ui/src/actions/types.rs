//! Core types for the Action system.

use iced::keyboard::{self, Key};
use iced::Task;
use std::fmt;

use crate::msg::Message;
use crate::state::Nexus;

/// Unique identifier for an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActionId(pub &'static str);

impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Keyboard modifiers for key combinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,  // Command on macOS, Super/Win on other platforms
}

impl Modifiers {
    pub const NONE: Self = Self { ctrl: false, alt: false, shift: false, cmd: false };
    pub const CMD: Self = Self { ctrl: false, alt: false, shift: false, cmd: true };
    pub const CTRL: Self = Self { ctrl: true, alt: false, shift: false, cmd: false };
    pub const ALT: Self = Self { ctrl: false, alt: true, shift: false, cmd: false };
    pub const SHIFT: Self = Self { ctrl: false, alt: false, shift: true, cmd: false };
    pub const CMD_SHIFT: Self = Self { ctrl: false, alt: false, shift: true, cmd: true };
    pub const CTRL_SHIFT: Self = Self { ctrl: true, alt: false, shift: true, cmd: false };

    /// Check if these modifiers match the iced keyboard modifiers.
    pub fn matches(&self, iced_mods: &keyboard::Modifiers) -> bool {
        self.ctrl == iced_mods.control()
            && self.alt == iced_mods.alt()
            && self.shift == iced_mods.shift()
            && self.cmd == iced_mods.command()
    }
}

/// A key combination (modifiers + key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombo {
    pub modifiers: Modifiers,
    pub key: KeySpec,
}

impl KeyCombo {
    pub const fn new(modifiers: Modifiers, key: KeySpec) -> Self {
        Self { modifiers, key }
    }

    /// Check if this combo matches a key press event.
    pub fn matches(&self, key: &Key, modifiers: &keyboard::Modifiers) -> bool {
        self.modifiers.matches(modifiers) && self.key.matches(key)
    }

    /// Format for display (e.g., "⌘K", "Ctrl+C").
    pub fn display(&self) -> String {
        let mut result = String::new();

        #[cfg(target_os = "macos")]
        {
            if self.modifiers.ctrl { result.push('⌃'); }
            if self.modifiers.alt { result.push('⌥'); }
            if self.modifiers.shift { result.push('⇧'); }
            if self.modifiers.cmd { result.push('⌘'); }
        }

        #[cfg(not(target_os = "macos"))]
        {
            if self.modifiers.ctrl { result.push_str("Ctrl+"); }
            if self.modifiers.alt { result.push_str("Alt+"); }
            if self.modifiers.shift { result.push_str("Shift+"); }
            if self.modifiers.cmd { result.push_str("Super+"); }
        }

        result.push_str(&self.key.display());
        result
    }
}

/// Specification for a key (character or named key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySpec {
    Char(char),
    Named(keyboard::key::Named),
}

impl KeySpec {
    pub const fn char(c: char) -> Self {
        Self::Char(c)
    }

    pub const fn named(n: keyboard::key::Named) -> Self {
        Self::Named(n)
    }

    pub fn matches(&self, key: &Key) -> bool {
        match (self, key) {
            (KeySpec::Char(c), Key::Character(s)) => {
                s.to_lowercase().chars().next() == Some(c.to_ascii_lowercase())
            }
            (KeySpec::Named(n), Key::Named(key_named)) => n == key_named,
            _ => false,
        }
    }

    pub fn display(&self) -> String {
        match self {
            KeySpec::Char(c) => c.to_uppercase().to_string(),
            KeySpec::Named(n) => match n {
                keyboard::key::Named::Escape => "Esc".to_string(),
                keyboard::key::Named::Enter => "↩".to_string(),
                keyboard::key::Named::Tab => "⇥".to_string(),
                keyboard::key::Named::Backspace => "⌫".to_string(),
                keyboard::key::Named::Delete => "⌦".to_string(),
                keyboard::key::Named::ArrowUp => "↑".to_string(),
                keyboard::key::Named::ArrowDown => "↓".to_string(),
                keyboard::key::Named::ArrowLeft => "←".to_string(),
                keyboard::key::Named::ArrowRight => "→".to_string(),
                keyboard::key::Named::Space => "Space".to_string(),
                other => format!("{:?}", other),
            },
        }
    }
}

/// Context in which an action can be invoked.
/// Used for filtering in command palette and context menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionContext {
    /// Available everywhere.
    Global,
    /// Available when input is focused.
    InputFocused,
    /// Available when a block is focused/selected.
    BlockFocused,
    /// Available when text is selected.
    TextSelected,
    /// Available when agent is running.
    AgentRunning,
    /// Available when a shell command is running.
    ShellRunning,
}

/// Function type for checking if an action is enabled.
pub type IsEnabledFn = fn(&Nexus) -> bool;

/// Function type for executing an action.
pub type ExecuteFn = fn(&mut Nexus) -> Task<Message>;

/// A user-invokable action with metadata.
pub struct Action {
    /// Unique identifier.
    pub id: ActionId,
    /// Human-readable name (shown in palette).
    pub name: &'static str,
    /// Description of what this action does.
    pub description: &'static str,
    /// Additional keywords for fuzzy search.
    pub keywords: &'static [&'static str],
    /// Primary keyboard shortcut (if any).
    pub keybinding: Option<KeyCombo>,
    /// Context in which this action appears.
    pub context: ActionContext,
    /// Dynamic check for whether action is currently available.
    pub is_enabled: IsEnabledFn,
    /// Execute the action.
    pub execute: ExecuteFn,
}

impl Action {
    /// Check if this action is currently available.
    pub fn available(&self, state: &Nexus) -> bool {
        (self.is_enabled)(state)
    }

    /// Execute this action.
    pub fn run(&self, state: &mut Nexus) -> Task<Message> {
        (self.execute)(state)
    }

    /// Get searchable text for fuzzy matching.
    pub fn search_text(&self) -> String {
        let mut text = format!("{} {}", self.name, self.description);
        for kw in self.keywords {
            text.push(' ');
            text.push_str(kw);
        }
        text.to_lowercase()
    }
}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Action")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish()
    }
}
