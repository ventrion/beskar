//! Keyboard input abstraction (spec §112: navigation state).
//!
//! The reducer consumes [`Key`] values, not crossterm types, so state
//! transitions are unit-testable without a terminal. [`key_of`] converts
//! real terminal events at the I/O boundary only.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// A logical key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character (the payload of `KeyCode::Char`).
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    /// Ctrl+C: quits from anywhere (raw mode delivers it as a key event).
    Interrupt,
    /// Any key the TUI does not interpret.
    Other,
}

impl Key {
    /// The character for printable keys, `None` otherwise.
    pub fn char(self) -> Option<char> {
        match self {
            Key::Char(c) => Some(c),
            _ => None,
        }
    }

    /// Whether the key is one of the vertical movement keys the lists use.
    pub fn is_down(self) -> bool {
        matches!(self, Key::Down | Key::Char('j'))
    }

    /// Whether the key is one of the vertical movement keys the lists use.
    pub fn is_up(self) -> bool {
        matches!(self, Key::Up | Key::Char('k'))
    }
}

/// Converts a crossterm event into a logical key. Release/repeat noise is
/// filtered: only press events are translated (crossterm emits release
/// events on some platforms/protocols).
pub fn key_of(event: KeyEvent) -> Option<Key> {
    if event.kind != KeyEventKind::Press {
        return None;
    }
    if event
        .modifiers
        .contains(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        return match event.code {
            KeyCode::Char('c') | KeyCode::Char('C') => Some(Key::Interrupt),
            _ => None,
        };
    }
    Some(match event.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        _ => Key::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventState};

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    #[test]
    fn translates_press_events() {
        assert_eq!(key_of(press(KeyCode::Char('q'))), Some(Key::Char('q')));
        assert_eq!(key_of(press(KeyCode::Enter)), Some(Key::Enter));
        assert_eq!(key_of(press(KeyCode::Esc)), Some(Key::Esc));
        assert_eq!(key_of(press(KeyCode::BackTab)), Some(Key::BackTab));
        assert_eq!(key_of(press(KeyCode::PageDown)), Some(Key::PageDown));
        assert_eq!(key_of(press(KeyCode::F(3))), Some(Key::Other));
    }

    #[test]
    fn ignores_release_events() {
        let release = KeyEvent {
            kind: KeyEventKind::Release,
            ..press(KeyCode::Char('x'))
        };
        assert_eq!(key_of(release), None);
    }

    #[test]
    fn ctrl_c_interrupts_but_other_chords_are_ignored() {
        let mut event = press(KeyCode::Char('c'));
        event.modifiers = KeyModifiers::CONTROL;
        assert_eq!(key_of(event), Some(Key::Interrupt));
        let mut chord = press(KeyCode::Char('r'));
        chord.modifiers = KeyModifiers::CONTROL;
        assert_eq!(key_of(chord), None);
    }
}
