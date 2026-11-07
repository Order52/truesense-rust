//! The adapter from the theme engine's key names to this laptop's lighting cells.
//!
//! `ts-fx` draws on the 103 keys of its own layout and names each one (`escape`, `q_key`,
//! `kp_enter`). This laptop's keyboard is lit by 128 numbered cells (`column * 6 + row`), and which
//! cell belongs to which physical key was measured on the laptop, with two corrections to the
//! community map (Esc and CapsLock are swapped relative to it). The only reliable bridge is the
//! verified [`KeyMap`]: each engine name is translated to the name the key map knows, and the
//! cells come from there, so a correction made to the map reaches the themes without a second
//! table to keep in step.
#![forbid(unsafe_code)]

use ts_fx::cells::CellMap;

use crate::keymap::{KeyMap, NUM_CELLS};

/// For each key of the engine's layout, the name of the same key in the [`KeyMap`].
///
/// Every one of the 103 keys is listed (a test holds the list to the layout, both ways). The map's
/// names are the capture's own labels in lower case, so a few are not what a person would type:
/// the numpad minus is `-` and the main-row minus is `minus`, the Menu key is `options`, the
/// media keys are `backward`, `playpause` and `forward`.
pub const KEY_NAMES: [(&str, &str); 103] = [
    ("escape", "esc"),
    ("f1", "f1"),
    ("f2", "f2"),
    ("f3", "f3"),
    ("f4", "f4"),
    ("f5", "f5"),
    ("f6", "f6"),
    ("f7", "f7"),
    ("f8", "f8"),
    ("f9", "f9"),
    ("f10", "f10"),
    ("f11", "f11"),
    ("f12", "f12"),
    ("print_screen", "prtsc"),
    ("insert", "ins"),
    ("delete", "del"),
    ("rewind", "backward"),
    ("play_pause", "playpause"),
    ("fast_forward", "forward"),
    ("power", "power"),
    ("backtick", "`"),
    ("number_row_1", "1"),
    ("number_row_2", "2"),
    ("number_row_3", "3"),
    ("number_row_4", "4"),
    ("number_row_5", "5"),
    ("number_row_6", "6"),
    ("number_row_7", "7"),
    ("number_row_8", "8"),
    ("number_row_9", "9"),
    ("number_row_0", "0"),
    ("dash", "minus"),
    ("equal", "="),
    ("backspace", "backspace"),
    ("predator", "predator"),
    ("num_lock", "numlock"),
    ("kp_forward_slash", "num/"),
    ("kp_asterisk", "*"),
    ("tab", "tab"),
    ("q_key", "q"),
    ("w_key", "w"),
    ("e_key", "e"),
    ("r_key", "r"),
    ("t_key", "t"),
    ("y_key", "y"),
    ("u_key", "u"),
    ("i_key", "i"),
    ("o_key", "o"),
    ("p_key", "p"),
    ("left_bracket", "["),
    ("right_bracket", "]"),
    ("backslash", "\\"),
    ("kp_7", "num7"),
    ("kp_8", "num8"),
    ("kp_9", "num9"),
    ("kp_minus", "-"),
    ("capslock", "caps"),
    ("a_key", "a"),
    ("s_key", "s"),
    ("d_key", "d"),
    ("f_key", "f"),
    ("g_key", "g"),
    ("h_key", "h"),
    ("j_key", "j"),
    ("k_key", "k"),
    ("l_key", "l"),
    ("semicolon", ";"),
    ("apostrophe", "'"),
    ("enter", "enter"),
    ("kp_4", "num4"),
    ("kp_5", "num5"),
    ("kp_6", "num6"),
    ("kp_plus", "+"),
    ("shift_l", "lshift"),
    ("z_key", "z"),
    ("x_key", "x"),
    ("c_key", "c"),
    ("v_key", "v"),
    ("b_key", "b"),
    ("n_key", "n"),
    ("m_key", "m"),
    ("comma", ","),
    ("dot", "."),
    ("slash", "/"),
    ("shift_r", "rshift"),
    ("up_arrow", "up"),
    ("kp_1", "num1"),
    ("kp_2", "num2"),
    ("kp_3", "num3"),
    ("ctrl_l", "ctrl"),
    ("fn", "fn"),
    ("win", "win"),
    ("alt_l", "lalt"),
    ("space", "space"),
    ("alt_r", "ralt"),
    ("menu", "options"),
    ("ctrl_r", "rctrl"),
    ("left_arrow", "left"),
    ("down_arrow", "down"),
    ("right_arrow", "right"),
    ("kp_0", "num0"),
    ("kp_dot", "num."),
    ("kp_enter", "numenter"),
];

/// The key map as the theme engine's cell numbering.
impl CellMap for KeyMap {
    fn cell_count(&self) -> usize {
        NUM_CELLS
    }

    fn cells_of(&self, key: &str) -> Vec<usize> {
        KEY_NAMES
            .iter()
            .find(|(fx, _)| *fx == key)
            .and_then(|(_, ours)| self.key(ours))
            .map(<[usize]>::to_vec)
            .unwrap_or_default()
    }
}
