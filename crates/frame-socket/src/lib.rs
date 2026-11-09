//! The socket frames reach a viewer on.
//!
//! The producer owns a ring of buffers and the viewer shows them. The viewer says what it can show
//! with [`Hello`]. The producer shares its buffers with [`Ring`], one file descriptor for each slot,
//! and announces each finished picture with [`Frame`]. A slot is the viewer's from then until its
//! [`Release`], and the producer leaves it alone meanwhile. An HDR picture's [`Frame`] follows a [`Light`].
//! A viewer that draws the cursor itself says so with [`Message::CursorWanted`], and from then on hears of
//! it by [`CursorShape`] and [`CursorMove`], as a cursor moving makes no frame. A new ring has a new generation, and
//! anything naming an older one is about buffers that have gone.
//!
//! Each message is one SEQPACKET packet, so nothing here has to find where a message ends. Nothing
//! knows where pictures come from: a buffer is a size and a descriptor, and colour is in CICP's terms.
//! A peer of another protocol version is refused, and the viewer can't be left guessing.

mod message;
mod shared;
mod socket;

pub use message::{
    Colour, CursorMove, CursorShape, DecodeError, Descriptor, Format, Frame, Hello, Light, MOST_FORMATS,
    MOST_SLOTS, Mastering, Message, Release, Ring, VERSION,
};
pub use shared::{sealed_bytes, sized_buffer};
pub use socket::{FrameListener, FrameSocket, Packet, SOCKET_NAME};
