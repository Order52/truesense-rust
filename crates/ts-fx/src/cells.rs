//! The adapter between the engine's key layout and a keyboard that is lit by numbered cells.
//!
//! The engine draws on [`ph16_71`](crate::layout::ph16_71): 103 keys, each with a name
//! (`escape`, `w_key`, `kp_7`) and a position in key units, in the layout's order. A host such
//! as TrueSense lights a keyboard by cells instead (the PH16-71's controller takes 128 colours
//! in one upload, and which cell belongs to which physical key was measured on the laptop, not
//! taken from the layout table). The two are joined through the [`CellMap`] trait: the host says
//! which cells a layout key lights, [`CellPainter`] turns one engine frame into one cell
//! picture.
//!
//! The trait keeps this crate independent of the host's key tables: `ts-domain` implements it
//! over its verified key map, and its tests pin every key name to its cell. The positions the
//! effects use (a wave travelling across the keys, a ripple around a press) stay the engine's
//! own, so every golden frame remains valid; only the final step from key to cell is the host's.

use crate::layout::ph16_71;

/// How a host numbers its lighting cells, seen from the engine's key names.
pub trait CellMap {
    /// How many colours one frame holds (128 on the PH16-71, of which 126 are lit).
    fn cell_count(&self) -> usize;

    /// The cells the layout key `key` lights, by the engine's own key name (`escape`,
    /// `number_row_1`, `kp_enter`; the names of [`ph16_71`]). Empty for a key the host has no
    /// cell for, which leaves it dark rather than lighting a wrong cell. A key may have more than
    /// one cell.
    fn cells_of(&self, key: &str) -> Vec<usize>;
}

/// Turns engine frames (one colour per key of the PH16-71 layout) into cell pictures.
///
/// Built once per host and map, so painting a frame is a table walk without name lookups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellPainter {
    cells: usize,
    /// For each layout key, in layout order, the cells it lights (out-of-range ones dropped).
    targets: Vec<Vec<usize>>,
    unmapped: Vec<String>,
}

impl CellPainter {
    /// Resolves every key of the PH16-71 layout through `map`. A cell number the map gives that
    /// is not below its own [`CellMap::cell_count`] is ignored, so a faulty map cannot make a
    /// frame write outside its picture.
    pub fn new(map: &dyn CellMap) -> CellPainter {
        let cells = map.cell_count();
        let mut targets = Vec::new();
        let mut unmapped = Vec::new();
        for key in ph16_71().iter() {
            let mut own: Vec<usize> = map.cells_of(&key.name).into_iter().filter(|&c| c < cells).collect();
            own.sort_unstable();
            own.dedup();
            if own.is_empty() {
                unmapped.push(key.name.clone());
            }
            targets.push(own);
        }
        CellPainter { cells, targets, unmapped }
    }

    /// The layout keys the map has no cell for (they stay dark). Empty on a complete map.
    pub fn unmapped(&self) -> &[String] {
        &self.unmapped
    }

    /// How many colours a picture from [`CellPainter::paint`] has.
    pub fn cell_count(&self) -> usize {
        self.cells
    }

    /// The cells the layout key at index `index` lights.
    pub fn cells_of_index(&self, index: usize) -> &[usize] {
        self.targets.get(index).map_or(&[], Vec::as_slice)
    }

    /// One cell picture for one engine frame: every key's colour on its cells, every other cell
    /// black. A frame shorter or longer than the layout is used as far as it goes.
    pub fn paint(&self, frame: &[[u8; 3]]) -> Vec<[u8; 3]> {
        let mut out = vec![[0u8; 3]; self.cells];
        for (colour, cells) in frame.iter().zip(&self.targets) {
            for &cell in cells {
                if let Some(slot) = out.get_mut(cell) {
                    *slot = *colour;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A keyboard whose cell for the key at layout index i is i + 10, with `escape` unlit.
    struct Fake;

    impl CellMap for Fake {
        fn cell_count(&self) -> usize {
            200
        }

        fn cells_of(&self, key: &str) -> Vec<usize> {
            if key == "escape" {
                return Vec::new();
            }
            match ph16_71().iter().position(|k| k.name == key) {
                Some(i) => vec![i + 10],
                None => Vec::new(),
            }
        }
    }

    #[test]
    fn a_frame_lands_on_the_cells_of_its_keys_and_the_rest_stays_black() {
        let painter = CellPainter::new(&Fake);
        let frame: Vec<[u8; 3]> = (0..103).map(|i| [i as u8 + 1, 2, 3]).collect();
        let picture = painter.paint(&frame);
        assert_eq!(picture.len(), 200);
        assert_eq!(painter.unmapped(), ["escape"]);
        assert_eq!(picture[10], [0, 0, 0], "escape has no cell");
        assert_eq!(picture[11], [2, 2, 3], "f1 is the second key");
        assert_eq!(picture[10 + 102], [103, 2, 3]);
        assert_eq!(picture[0], [0, 0, 0]);
        assert_eq!(picture[199], [0, 0, 0]);
    }

    #[test]
    fn a_short_or_long_frame_is_painted_as_far_as_it_goes() {
        let painter = CellPainter::new(&Fake);
        assert_eq!(painter.paint(&[]).len(), 200);
        let long = vec![[9, 9, 9]; 500];
        assert_eq!(painter.paint(&long).len(), 200);
    }

    struct Wild;

    impl CellMap for Wild {
        fn cell_count(&self) -> usize {
            4
        }

        fn cells_of(&self, key: &str) -> Vec<usize> {
            if key == "escape" { vec![3, 3, 1_000_000, 0] } else { Vec::new() }
        }
    }

    #[test]
    fn a_faulty_map_cannot_write_outside_the_picture() {
        let painter = CellPainter::new(&Wild);
        assert_eq!(painter.cells_of_index(0), [0, 3], "sorted, deduplicated, in range");
        let picture = painter.paint(&vec![[7, 7, 7]; 103]);
        assert_eq!(picture, vec![[7, 7, 7], [0; 3], [0; 3], [7, 7, 7]]);
        assert_eq!(painter.unmapped().len(), 102);
        assert!(painter.cells_of_index(5000).is_empty());
    }
}
