//! Turning a slot into a texture. A dmabuf goes to GTK as it is, and stays in video memory. A display
//! with no way to take one, like Broadway, gets a copy, if the buffer is something that can be read.

use super::ring::read_pixels;
use gtk::prelude::*;
use gtk::{gdk, glib};
use guesthouse_frame_socket::{Colour, Descriptor};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Arc;

/// DRM's names for the formats a copy can show.
const XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
const ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
const SINGLE_PLANE: u32 = 1;

fn memory_format(fourcc: u32) -> Option<gdk::MemoryFormat> {
    match fourcc {
        XRGB8888 => Some(gdk::MemoryFormat::B8g8r8x8),
        ARGB8888 => Some(gdk::MemoryFormat::B8g8r8a8),
        _ => None,
    }
}

fn colour_state(colour: Colour) -> Option<gdk::ColorState> {
    let params = gdk::CicpParams::new();
    params.set_color_primaries(colour.primaries.into());
    params.set_transfer_function(colour.transfer.into());
    params.set_matrix_coefficients(colour.matrix.into());
    params.set_range(if colour.full_range { gdk::CicpRange::Full } else { gdk::CicpRange::Narrow });
    params.build_color_state().ok()
}

/// A texture on the buffer itself. `released` runs when GTK has finished with it.
fn dmabuf_texture(
    display: &gdk::Display,
    file: Arc<OwnedFd>,
    descriptor: &Descriptor,
    released: Box<dyn FnOnce() + Send>,
) -> Result<gdk::Texture, Box<dyn FnOnce() + Send>> {
    let mut builder = gdk::DmabufTextureBuilder::new()
        .set_display(display)
        .set_width(descriptor.width)
        .set_height(descriptor.height)
        .set_fourcc(descriptor.format.fourcc)
        .set_modifier(descriptor.format.modifier)
        .set_n_planes(SINGLE_PLANE)
        .set_offset(0, descriptor.offset as u32)
        .set_stride(0, descriptor.stride);
    if descriptor.colour != Colour::SRGB
        && let Some(state) = colour_state(descriptor.colour)
    {
        builder = builder.set_color_state(Some(&state));
    }
    let fd = file.as_raw_fd();
    let shared = Arc::new(std::sync::Mutex::new(Some(released)));
    let on_release = shared.clone();
    // SAFETY: GTK reads `fd` for as long as the texture lives, and tells this closure when that ends.
    // The closure owns a reference to the file, so the descriptor stays open until then.
    let built = unsafe {
        builder.set_fd(0, fd).build_with_release_func(move || {
            drop(file);
            if let Some(released) = on_release.lock().ok().and_then(|mut slot| slot.take()) {
                released();
            }
        })
    };
    built.map_err(|_| shared.lock().ok().and_then(|mut slot| slot.take()).unwrap_or_else(|| Box::new(|| {})))
}

fn copied_texture(file: &OwnedFd, descriptor: &Descriptor) -> io::Result<gdk::Texture> {
    let format = memory_format(descriptor.format.fourcc).ok_or(io::ErrorKind::Unsupported)?;
    let pixels = read_pixels(file, descriptor)?;
    let texture = gdk::MemoryTexture::new(
        descriptor.width as i32,
        descriptor.height as i32,
        format,
        &glib::Bytes::from_owned(pixels),
        descriptor.stride as usize,
    );
    Ok(texture.upcast())
}

/// A new texture of this slot's picture. `released` runs when the slot can be written again, even if
/// no texture could be made.
pub fn texture(
    display: &gdk::Display,
    file: Arc<OwnedFd>,
    descriptor: &Descriptor,
    released: Box<dyn FnOnce() + Send>,
) -> Option<gdk::Texture> {
    let reading = file.clone();
    match dmabuf_texture(display, file, descriptor, released) {
        Ok(texture) => Some(texture),
        Err(released) => {
            let copy = copied_texture(&reading, descriptor).ok();
            released();
            copy
        }
    }
}
