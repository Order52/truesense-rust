#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! One test per key name: the engine's key, the name this map knows it by, and the cell it
//! lights. The expected cells were written down from the engine layout's own per-key slots (the
//! community map) with the two corrections the owner verified on this laptop (Esc is cell 2 and
//! CapsLock is cell 5), not read back from the adapter, so a wrong row of `KEY_NAMES` cannot
//! agree with itself.

use std::collections::BTreeSet;

use ts_fx::cells::{CellMap, CellPainter};
use ts_fx::layout::ph16_71;

use super::cells::KEY_NAMES;
use crate::keymap::{KeyMap, NUM_CELLS};

/// Engine key name and the one cell it lights.
const EXPECTED: [(&str, usize); 103] = [
    ("escape", 2),
    ("f1", 11),
    ("f2", 17),
    ("f3", 23),
    ("f4", 29),
    ("f5", 35),
    ("f6", 41),
    ("f7", 47),
    ("f8", 53),
    ("f9", 59),
    ("f10", 65),
    ("f11", 71),
    ("f12", 77),
    ("print_screen", 83),
    ("insert", 89),
    ("delete", 95),
    ("rewind", 104),
    ("play_pause", 105),
    ("fast_forward", 103),
    ("power", 107),
    ("backtick", 4),
    ("number_row_1", 10),
    ("number_row_2", 16),
    ("number_row_3", 22),
    ("number_row_4", 28),
    ("number_row_5", 34),
    ("number_row_6", 40),
    ("number_row_7", 46),
    ("number_row_8", 52),
    ("number_row_9", 58),
    ("number_row_0", 64),
    ("dash", 70),
    ("equal", 76),
    ("backspace", 82),
    ("predator", 106),
    ("num_lock", 112),
    ("kp_forward_slash", 118),
    ("kp_asterisk", 124),
    ("tab", 3),
    ("q_key", 15),
    ("w_key", 21),
    ("e_key", 27),
    ("r_key", 33),
    ("t_key", 39),
    ("y_key", 45),
    ("u_key", 51),
    ("i_key", 57),
    ("o_key", 63),
    ("p_key", 69),
    ("left_bracket", 75),
    ("right_bracket", 81),
    ("backslash", 13),
    ("kp_7", 99),
    ("kp_8", 111),
    ("kp_9", 117),
    ("kp_minus", 123),
    ("capslock", 5),
    ("a_key", 20),
    ("s_key", 26),
    ("d_key", 32),
    ("f_key", 38),
    ("g_key", 44),
    ("h_key", 50),
    ("j_key", 56),
    ("k_key", 62),
    ("l_key", 68),
    ("semicolon", 74),
    ("apostrophe", 80),
    ("enter", 86),
    ("kp_4", 101),
    ("kp_5", 110),
    ("kp_6", 116),
    ("kp_plus", 122),
    ("shift_l", 1),
    ("z_key", 19),
    ("x_key", 25),
    ("c_key", 31),
    ("v_key", 37),
    ("b_key", 43),
    ("n_key", 49),
    ("m_key", 55),
    ("comma", 61),
    ("dot", 67),
    ("slash", 73),
    ("shift_r", 85),
    ("up_arrow", 90),
    ("kp_1", 100),
    ("kp_2", 109),
    ("kp_3", 115),
    ("ctrl_l", 0),
    ("fn", 6),
    ("win", 12),
    ("alt_l", 18),
    ("space", 24),
    ("alt_r", 60),
    ("menu", 72),
    ("ctrl_r", 78),
    ("left_arrow", 84),
    ("down_arrow", 96),
    ("right_arrow", 102),
    ("kp_0", 108),
    ("kp_dot", 114),
    ("kp_enter", 120),
];

/// One `#[test]` per key: the engine key name lights exactly the cell written beside it.
macro_rules! key_tests {
    ($($test:ident: $key:literal => $cell:literal,)*) => {
        $(
            #[test]
            fn $test() {
                assert_eq!(KeyMap::verified().cells_of($key), vec![$cell], $key);
            }
        )*
    };
}

key_tests! {
    key_escape: "escape" => 2,
    key_f1: "f1" => 11,
    key_f2: "f2" => 17,
    key_f3: "f3" => 23,
    key_f4: "f4" => 29,
    key_f5: "f5" => 35,
    key_f6: "f6" => 41,
    key_f7: "f7" => 47,
    key_f8: "f8" => 53,
    key_f9: "f9" => 59,
    key_f10: "f10" => 65,
    key_f11: "f11" => 71,
    key_f12: "f12" => 77,
    key_print_screen: "print_screen" => 83,
    key_insert: "insert" => 89,
    key_delete: "delete" => 95,
    key_rewind: "rewind" => 104,
    key_play_pause: "play_pause" => 105,
    key_fast_forward: "fast_forward" => 103,
    key_power: "power" => 107,
    key_backtick: "backtick" => 4,
    key_number_row_1: "number_row_1" => 10,
    key_number_row_2: "number_row_2" => 16,
    key_number_row_3: "number_row_3" => 22,
    key_number_row_4: "number_row_4" => 28,
    key_number_row_5: "number_row_5" => 34,
    key_number_row_6: "number_row_6" => 40,
    key_number_row_7: "number_row_7" => 46,
    key_number_row_8: "number_row_8" => 52,
    key_number_row_9: "number_row_9" => 58,
    key_number_row_0: "number_row_0" => 64,
    key_dash: "dash" => 70,
    key_equal: "equal" => 76,
    key_backspace: "backspace" => 82,
    key_predator: "predator" => 106,
    key_num_lock: "num_lock" => 112,
    key_kp_forward_slash: "kp_forward_slash" => 118,
    key_kp_asterisk: "kp_asterisk" => 124,
    key_tab: "tab" => 3,
    key_q_key: "q_key" => 15,
    key_w_key: "w_key" => 21,
    key_e_key: "e_key" => 27,
    key_r_key: "r_key" => 33,
    key_t_key: "t_key" => 39,
    key_y_key: "y_key" => 45,
    key_u_key: "u_key" => 51,
    key_i_key: "i_key" => 57,
    key_o_key: "o_key" => 63,
    key_p_key: "p_key" => 69,
    key_left_bracket: "left_bracket" => 75,
    key_right_bracket: "right_bracket" => 81,
    key_backslash: "backslash" => 13,
    key_kp_7: "kp_7" => 99,
    key_kp_8: "kp_8" => 111,
    key_kp_9: "kp_9" => 117,
    key_kp_minus: "kp_minus" => 123,
    key_capslock: "capslock" => 5,
    key_a_key: "a_key" => 20,
    key_s_key: "s_key" => 26,
    key_d_key: "d_key" => 32,
    key_f_key: "f_key" => 38,
    key_g_key: "g_key" => 44,
    key_h_key: "h_key" => 50,
    key_j_key: "j_key" => 56,
    key_k_key: "k_key" => 62,
    key_l_key: "l_key" => 68,
    key_semicolon: "semicolon" => 74,
    key_apostrophe: "apostrophe" => 80,
    key_enter: "enter" => 86,
    key_kp_4: "kp_4" => 101,
    key_kp_5: "kp_5" => 110,
    key_kp_6: "kp_6" => 116,
    key_kp_plus: "kp_plus" => 122,
    key_shift_l: "shift_l" => 1,
    key_z_key: "z_key" => 19,
    key_x_key: "x_key" => 25,
    key_c_key: "c_key" => 31,
    key_v_key: "v_key" => 37,
    key_b_key: "b_key" => 43,
    key_n_key: "n_key" => 49,
    key_m_key: "m_key" => 55,
    key_comma: "comma" => 61,
    key_dot: "dot" => 67,
    key_slash: "slash" => 73,
    key_shift_r: "shift_r" => 85,
    key_up_arrow: "up_arrow" => 90,
    key_kp_1: "kp_1" => 100,
    key_kp_2: "kp_2" => 109,
    key_kp_3: "kp_3" => 115,
    key_ctrl_l: "ctrl_l" => 0,
    key_fn: "fn" => 6,
    key_win: "win" => 12,
    key_alt_l: "alt_l" => 18,
    key_space: "space" => 24,
    key_alt_r: "alt_r" => 60,
    key_menu: "menu" => 72,
    key_ctrl_r: "ctrl_r" => 78,
    key_left_arrow: "left_arrow" => 84,
    key_down_arrow: "down_arrow" => 96,
    key_right_arrow: "right_arrow" => 102,
    key_kp_0: "kp_0" => 108,
    key_kp_dot: "kp_dot" => 114,
    key_kp_enter: "kp_enter" => 120,
}

#[test]
fn the_expected_table_and_the_name_table_cover_the_layout_exactly() {
    let layout: Vec<&str> = ph16_71().iter().map(|k| k.name.as_str()).collect();
    let expected: Vec<&str> = EXPECTED.iter().map(|(k, _)| *k).collect();
    let named: Vec<&str> = KEY_NAMES.iter().map(|(k, _)| *k).collect();
    assert_eq!(expected, layout, "the expected cells follow the layout's order");
    assert_eq!(named, layout, "every layout key has a name, in the layout's order");
    let ours: BTreeSet<&str> = KEY_NAMES.iter().map(|(_, o)| *o).collect();
    assert_eq!(ours.len(), 103, "no two engine keys share a key-map name");
}

#[test]
fn the_cells_are_the_layouts_own_slots_except_the_two_verified_corrections() {
    let map = KeyMap::verified();
    let mut differ = Vec::new();
    for key in ph16_71().iter() {
        if map.cells_of(&key.name) != vec![usize::from(key.slot)] {
            differ.push(key.name.as_str());
        }
    }
    assert_eq!(differ, ["escape", "capslock"]);
}

#[test]
fn no_two_keys_light_the_same_cell_and_all_are_in_range() {
    let map = KeyMap::verified();
    let mut seen = BTreeSet::new();
    for key in ph16_71().iter() {
        for cell in map.cells_of(&key.name) {
            assert!(cell < NUM_CELLS, "{}", key.name);
            assert!(seen.insert(cell), "cell {cell} lit by two keys ({})", key.name);
        }
    }
    assert_eq!(seen.len(), 103);
}

#[test]
fn an_unknown_engine_key_has_no_cell() {
    assert!(KeyMap::verified().cells_of("hyper").is_empty());
    assert!(KeyMap::verified().cells_of("").is_empty());
    // The key map's own names are not the engine's.
    assert!(KeyMap::verified().cells_of("esc").is_empty());
}

#[test]
fn a_frame_paints_every_key_on_its_cell_and_leaves_the_rest_dark() {
    let painter = CellPainter::new(KeyMap::verified());
    assert!(painter.unmapped().is_empty(), "{:?}", painter.unmapped());
    assert_eq!(painter.cell_count(), NUM_CELLS);
    // Key i gets the colour [i + 1, 7, 9]; read it back through the cell of each key.
    let frame: Vec<[u8; 3]> = (0..103).map(|i| [i as u8 + 1, 7, 9]).collect();
    let picture = painter.paint(&frame);
    assert_eq!(picture.len(), NUM_CELLS);
    for (i, (_, cell)) in EXPECTED.iter().enumerate() {
        assert_eq!(picture[*cell], [i as u8 + 1, 7, 9], "{}", EXPECTED[i].0);
    }
    let lit: BTreeSet<usize> = EXPECTED.iter().map(|(_, c)| *c).collect();
    for cell in (0..NUM_CELLS).filter(|c| !lit.contains(c)) {
        assert_eq!(picture[cell], [0, 0, 0], "cell {cell} has no key and stays dark");
    }
}
