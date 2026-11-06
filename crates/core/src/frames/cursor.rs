//! The guest's hardware cursor, on its way to a viewer that draws it itself.
//!
//! A cursor moving makes no frame, so this has its own small path. Only the newest position matters,
//! so a viewer that's behind skips the ones in between. A shape comes in pieces, which are only kept
//! if they arrive in order: a driver that gets it wrong loses a cursor and nothing else.

use guesthouse_messages::indirect::{CursorMove, CursorPiece};

/// A whole picture. Pixels are BGRA with straight alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorShape {
    pub shape: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub pixels: Vec<u8>,
}

/// See the module documentation.
#[derive(Debug, Default)]
pub(crate) struct Cursor {
    assembling: Option<CursorShape>,
    pub shape: Option<CursorShape>,
    pub shape_unsent: bool,
    pub position: Option<CursorMove>,
    pub position_unsent: bool,
}

impl Cursor {
    pub fn piece_came(&mut self, piece: &CursorPiece<'_>) {
        if piece.offset == 0 {
            let mut pixels = Vec::with_capacity(piece.total as usize);
            pixels.extend_from_slice(piece.pixels);
            self.assembling = Some(CursorShape {
                shape: piece.shape,
                width: piece.width,
                height: piece.height,
                pitch: piece.pitch,
                hot_x: piece.hot_x,
                hot_y: piece.hot_y,
                pixels,
            });
        } else {
            let in_order = |shape: &CursorShape| {
                shape.shape == piece.shape && shape.pixels.len() == piece.offset as usize
            };
            match self.assembling.as_mut().filter(|shape| in_order(shape)) {
                Some(shape) => shape.pixels.extend_from_slice(piece.pixels),
                None => {
                    self.assembling = None;
                    return;
                }
            }
        }
        if self.assembling.as_ref().is_some_and(|shape| shape.pixels.len() == piece.total as usize) {
            self.shape = self.assembling.take();
            self.shape_unsent = true;
        }
    }

    pub fn moved(&mut self, position: CursorMove) {
        self.position = Some(position);
        self.position_unsent = true;
    }

    /// A viewer that has just asked needs what's already known.
    pub fn send_again(&mut self) {
        self.shape_unsent = self.shape.is_some();
        self.position_unsent = self.position.is_some();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BYTES: u32 = 16;

    fn piece(offset: u32, pixels: &[u8]) -> CursorPiece<'_> {
        CursorPiece {
            monitor: 0,
            shape: 1,
            width: 2,
            height: 2,
            pitch: 8,
            hot_x: 0,
            hot_y: 0,
            total: BYTES,
            offset,
            pixels,
        }
    }

    #[test]
    fn pieces_in_order_make_a_shape() {
        let mut cursor = Cursor::default();
        cursor.piece_came(&piece(0, &[1; 8]));
        assert!(cursor.shape.is_none());
        cursor.piece_came(&piece(8, &[2; 8]));
        assert_eq!(cursor.shape.unwrap().pixels, [[1; 8], [2; 8]].concat());
        assert!(cursor.shape_unsent);
    }

    #[test]
    fn a_piece_out_of_order_loses_the_shape_and_nothing_else() {
        let mut cursor = Cursor::default();
        cursor.piece_came(&piece(0, &[1; 4]));
        cursor.piece_came(&piece(8, &[2; 8]));
        assert!(cursor.shape.is_none());
        cursor.piece_came(&piece(4, &[3; 4]));
        assert!(cursor.shape.is_none());
    }

    #[test]
    fn a_new_shape_can_start_over_an_unfinished_one() {
        let mut cursor = Cursor::default();
        cursor.piece_came(&piece(0, &[1; 8]));
        cursor.piece_came(&piece(0, &[9; 16]));
        assert_eq!(cursor.shape.unwrap().pixels, [9; 16]);
    }
}
