//! The guest's cursor drawn in the window. While the mouse is captured the host's pointer is gone, so
//! this is the only cursor there is.

use super::guest_cursor::CursorPicture;
use gtk::prelude::*;
use std::cell::Cell;

/// Where it is on the widget, to tell when it needs moving.
type Place = (i32, i32, i32, i32);

pub struct CursorLayer {
    layer: gtk::Fixed,
    picture: gtk::Picture,
    place: Cell<Option<Place>>,
}

impl CursorLayer {
    pub fn new() -> Self {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Fill)
            .can_shrink(true)
            .can_target(false)
            .visible(false)
            .build();
        let layer = gtk::Fixed::builder().can_target(false).build();
        layer.put(&picture, 0.0, 0.0);
        Self { layer, picture, place: Cell::new(None) }
    }

    pub fn widget(&self) -> &gtk::Fixed {
        &self.layer
    }

    /// `x` and `y` are the picture's top left corner on the widget, and `scale` is the guest's pixels
    /// to the widget's.
    pub fn show(&self, cursor: &CursorPicture, x: f64, y: f64, scale: f64) {
        let width = (f64::from(cursor.texture.width()) * scale).round() as i32;
        let height = (f64::from(cursor.texture.height()) * scale).round() as i32;
        let place = (x.round() as i32, y.round() as i32, width, height);
        if self.place.get() == Some(place) {
            return;
        }
        self.place.set(Some(place));
        self.picture.set_paintable(Some(&cursor.texture));
        self.picture.set_size_request(width, height);
        self.layer.move_(&self.picture, f64::from(place.0), f64::from(place.1));
        self.picture.set_visible(true);
    }

    pub fn hide(&self) {
        if self.place.take().is_some() {
            self.picture.set_visible(false);
        }
    }
}
