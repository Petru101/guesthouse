//! Locking the pointer to the window and reading how far it moves, for games.
//!
//! GTK has no API for the two Wayland protocols this takes, so they're used directly on GTK's own
//! connection. These objects live on a queue of their own, so GTK's queue never sees them. GTK goes on
//! reading the socket for everyone, and `take_motion` picks up what landed on this queue. Nothing here
//! reads or waits but the one round trip at the start, and a hung guest can't get in the way of it.

use gdk_wayland::prelude::*;
use gdk_wayland::{WaylandDevice, WaylandDisplay, WaylandSurface};
use gtk::gdk;
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::pointer_constraints::zv1::client::zwp_locked_pointer_v1::{
    self, ZwpLockedPointerV1,
};
use wayland_protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1::{
    Lifetime, ZwpPointerConstraintsV1,
};
use wayland_protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1;
use wayland_protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_v1::{
    self, ZwpRelativePointerV1,
};

const PROTOCOL_VERSION: u32 = 1;

pub trait PointerLock {
    /// Asks for the lock on a surface, and says if it could. It starts when the pointer is on it.
    fn lock(&mut self, surface: &gdk::Surface) -> bool;
    fn unlock(&mut self);
    /// Whole units moved since the last call, without the compositor's acceleration.
    fn take_motion(&mut self) -> (i64, i64);
    /// The compositor has granted the lock.
    fn held(&self) -> bool;
    /// The compositor has ended it, as when the window loses focus.
    fn ended(&self) -> bool;
}

/// What the compositor has said.
#[derive(Default)]
struct Events {
    held: bool,
    ended: bool,
    x: f64,
    y: f64,
}

impl Dispatch<WlRegistry, GlobalListContents> for Events {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpPointerConstraintsV1, ()> for Events {
    fn event(
        _: &mut Self,
        _: &ZwpPointerConstraintsV1,
        _: <ZwpPointerConstraintsV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpRelativePointerManagerV1, ()> for Events {
    fn event(
        _: &mut Self,
        _: &ZwpRelativePointerManagerV1,
        _: <ZwpRelativePointerManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpLockedPointerV1, ()> for Events {
    fn event(
        events: &mut Self,
        _: &ZwpLockedPointerV1,
        event: zwp_locked_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_locked_pointer_v1::Event::Locked => events.held = true,
            zwp_locked_pointer_v1::Event::Unlocked => {
                events.held = false;
                events.ended = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpRelativePointerV1, ()> for Events {
    fn event(
        events: &mut Self,
        _: &ZwpRelativePointerV1,
        event: zwp_relative_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_relative_pointer_v1::Event::RelativeMotion { dx_unaccel, dy_unaccel, .. } = event {
            events.x += dx_unaccel;
            events.y += dy_unaccel;
        }
    }
}

struct WaylandLock {
    display: WaylandDisplay,
    queue: EventQueue<Events>,
    events: Events,
    constraints: ZwpPointerConstraintsV1,
    relative_manager: ZwpRelativePointerManagerV1,
    relative: Option<ZwpRelativePointerV1>,
    locked: Option<ZwpLockedPointerV1>,
}

fn bind<I>(globals: &GlobalList, handle: &QueueHandle<Events>) -> Option<I>
where
    I: Proxy + 'static,
    Events: Dispatch<I, ()>,
{
    globals.bind(handle, PROTOCOL_VERSION..=PROTOCOL_VERSION, ()).ok()
}

impl WaylandLock {
    fn new(display: WaylandDisplay) -> Option<Self> {
        let backend = display.wl_display()?.backend().upgrade()?;
        let connection = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<Events>(&connection).ok()?;
        let handle = queue.handle();
        Some(Self {
            constraints: bind(&globals, &handle)?,
            relative_manager: bind(&globals, &handle)?,
            display,
            queue,
            events: Events::default(),
            relative: None,
            locked: None,
        })
    }
}

impl PointerLock for WaylandLock {
    fn lock(&mut self, surface: &gdk::Surface) -> bool {
        // A seat has no pointer while no mouse is plugged in, so look each time.
        let seat = self.display.default_seat();
        let device = seat.and_then(|seat| seat.pointer());
        let device = device.and_then(|device| device.downcast::<WaylandDevice>().ok());
        let Some(pointer) = device.and_then(|device| device.wl_pointer()) else { return false };
        let surface = surface.downcast_ref::<WaylandSurface>().and_then(|surface| surface.wl_surface());
        let Some(surface) = surface else { return false };
        let handle = self.queue.handle();
        self.events = Events::default();
        self.relative = Some(self.relative_manager.get_relative_pointer(&pointer, &handle, ()));
        let lock = self.constraints.lock_pointer(&surface, &pointer, None, Lifetime::Oneshot, &handle, ());
        self.locked = Some(lock);
        let _ = self.queue.flush();
        true
    }

    fn unlock(&mut self) {
        if let Some(relative) = self.relative.take() {
            relative.destroy();
        }
        if let Some(locked) = self.locked.take() {
            locked.destroy();
        }
        self.events.held = false;
        let _ = self.queue.flush();
    }

    /// What is left over counts towards the next call.
    fn take_motion(&mut self) -> (i64, i64) {
        let _ = self.queue.dispatch_pending(&mut self.events);
        let (x, y) = (self.events.x.trunc(), self.events.y.trunc());
        self.events.x -= x;
        self.events.y -= y;
        (x as i64, y as i64)
    }

    fn held(&self) -> bool {
        self.events.held
    }

    fn ended(&self) -> bool {
        self.events.ended
    }
}

/// The display's lock. `None` if this isn't Wayland, or the compositor can't do it.
pub fn pointer_lock(display: &gdk::Display) -> Option<Box<dyn PointerLock>> {
    let display = display.clone().downcast::<WaylandDisplay>().ok()?;
    Some(Box::new(WaylandLock::new(display)?))
}
