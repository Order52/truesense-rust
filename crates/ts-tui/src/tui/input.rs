//! What the user can do, as the screen understands it, and its translation from crossterm.
//!
//! The screen has its own key type so that nothing outside this file knows about the terminal
//! library: the tests press `Key::Char('l')`, the loop translates whatever the terminal sent.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

/// A key press. A printable character is its own key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Char(char),
    Enter,
    Tab,
    BackTab,
    Esc,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// Ctrl-L: redraw the whole terminal after another program wrote over it.
    Redraw,
}

/// One thing the user did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Input {
    Key(Key),
    /// The button went down at this cell. Acting on the press (no click delay) is what lets a
    /// slider be grabbed.
    Press {
        x: i32,
        y: i32,
    },
    /// The wheel: -1 up, +1 down.
    Wheel(i32),
    /// The pointer moved with the button held.
    Drag {
        x: i32,
        y: i32,
    },
    /// The button came up.
    Release,
    /// The terminal changed size.
    Resize,
    /// Ctrl-C: leave at once (after sending what was chosen).
    Quit,
}

/// Translates a terminal event; events the screen has no use for are `None`.
pub(crate) fn translate(event: Event) -> Option<Input> {
    match event {
        Event::Key(key) => translate_key(key),
        Event::Mouse(mouse) => translate_mouse(mouse),
        Event::Resize(..) => Some(Input::Resize),
        _ => None,
    }
}

fn translate_key(ev: KeyEvent) -> Option<Input> {
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let key = match ev.code {
        KeyCode::Char('c' | 'C') if ctrl => return Some(Input::Quit),
        KeyCode::Char('l' | 'L') if ctrl => Key::Redraw,
        KeyCode::Char(_) if ctrl || ev.modifiers.contains(KeyModifiers::ALT) => return None,
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        _ => return None,
    };
    Some(Input::Key(key))
}

fn translate_mouse(ev: MouseEvent) -> Option<Input> {
    let (x, y) = (i32::from(ev.column), i32::from(ev.row));
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => Some(Input::Press { x, y }),
        MouseEventKind::Drag(MouseButton::Left) => Some(Input::Drag { x, y }),
        MouseEventKind::Up(MouseButton::Left) => Some(Input::Release),
        MouseEventKind::ScrollUp => Some(Input::Wheel(-1)),
        MouseEventKind::ScrollDown => Some(Input::Wheel(1)),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use crossterm::event::{KeyEventState, MouseEvent};

    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE })
    }

    fn mouse(kind: MouseEventKind) -> Event {
        Event::Mouse(MouseEvent { kind, column: 7, row: 3, modifiers: KeyModifiers::NONE })
    }

    #[test]
    fn keys_translate_to_the_screens_own_keys() {
        let none = KeyModifiers::NONE;
        let table = [
            (key(KeyCode::Char('q'), none), Some(Input::Key(Key::Char('q')))),
            (key(KeyCode::Char('Q'), KeyModifiers::SHIFT), Some(Input::Key(Key::Char('Q')))),
            (key(KeyCode::Enter, none), Some(Input::Key(Key::Enter))),
            (key(KeyCode::BackTab, KeyModifiers::SHIFT), Some(Input::Key(Key::BackTab))),
            (key(KeyCode::Char('c'), KeyModifiers::CONTROL), Some(Input::Quit)),
            (key(KeyCode::Char('l'), KeyModifiers::CONTROL), Some(Input::Key(Key::Redraw))),
            (key(KeyCode::Char('x'), KeyModifiers::CONTROL), None),
            (key(KeyCode::Char('x'), KeyModifiers::ALT), None),
            (key(KeyCode::F(5), none), None),
            (key(KeyCode::PageDown, none), Some(Input::Key(Key::PageDown))),
            (Event::Resize(80, 24), Some(Input::Resize)),
            (Event::FocusGained, None),
        ];
        for (event, want) in table {
            let shown = format!("{event:?}");
            assert_eq!(translate(event), want, "{shown}");
        }
    }

    #[test]
    fn a_released_key_is_not_a_second_press() {
        let release = Event::Key(KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        });
        assert_eq!(translate(release), None);
    }

    #[test]
    fn the_left_button_presses_drags_and_releases_and_the_wheel_scrolls() {
        let table = [
            (MouseEventKind::Down(MouseButton::Left), Some(Input::Press { x: 7, y: 3 })),
            (MouseEventKind::Drag(MouseButton::Left), Some(Input::Drag { x: 7, y: 3 })),
            (MouseEventKind::Up(MouseButton::Left), Some(Input::Release)),
            (MouseEventKind::ScrollUp, Some(Input::Wheel(-1))),
            (MouseEventKind::ScrollDown, Some(Input::Wheel(1))),
            (MouseEventKind::Down(MouseButton::Right), None),
            (MouseEventKind::Moved, None),
        ];
        for (kind, want) in table {
            assert_eq!(translate(mouse(kind)), want, "{kind:?}");
        }
    }
}
