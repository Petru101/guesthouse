//! Frames from QEMU's D-Bus display, which is what firmware, an install and any machine with no GPU
//! show. One thread answers QEMU, and it waits for nothing in GTK, because QEMU waits for some of these
//! answers with the guest stopped. A frame taken while the next one arrives can show a little of both,
//! and that next frame puts it right.

use super::display_attach::attach;
use super::display_surface::{PIXMAN_A8R8G8B8, PIXMAN_X8R8G8B8, Surface};
use super::frames::FrameSource;
use gtk::gdk::prelude::*;
use gtk::glib::variant::{FromVariant, Handle};
use gtk::{gdk, gio, glib};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const LISTENER_PATH: &str = "/org/qemu/Display1/Listener";
const LISTENER: &str = "org.qemu.Display1.Listener";
const LISTENER_MAP: &str = "org.qemu.Display1.Listener.Unix.Map";
/// Only what QEMU calls here.
const LISTENER_XML: &str = r#"<node>
  <interface name="org.qemu.Display1.Listener">
    <method name="Scanout">
      <arg type="u" direction="in"/><arg type="u" direction="in"/><arg type="u" direction="in"/>
      <arg type="u" direction="in"/><arg type="ay" direction="in"/>
    </method>
    <method name="Update">
      <arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="i" direction="in"/>
      <arg type="i" direction="in"/><arg type="u" direction="in"/><arg type="u" direction="in"/>
      <arg type="ay" direction="in"/>
    </method>
    <method name="Disable"/>
    <method name="MouseSet">
      <arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="i" direction="in"/>
    </method>
    <method name="CursorDefine">
      <arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="i" direction="in"/>
      <arg type="i" direction="in"/><arg type="ay" direction="in"/>
    </method>
    <property name="Interfaces" type="as" access="read"/>
  </interface>
  <interface name="org.qemu.Display1.Listener.Unix.Map">
    <method name="ScanoutMap">
      <arg type="h" direction="in"/><arg type="u" direction="in"/><arg type="u" direction="in"/>
      <arg type="u" direction="in"/><arg type="u" direction="in"/><arg type="u" direction="in"/>
    </method>
    <method name="UpdateMap">
      <arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="i" direction="in"/>
      <arg type="i" direction="in"/>
    </method>
  </interface>
</node>"#;
const BYTES_PER_PIXEL: i32 = 4;

/// The pointer's shape, from QEMU.
struct Cursor {
    width: i32,
    height: i32,
    hot_x: i32,
    hot_y: i32,
    pixels: Vec<u8>,
}

/// What the thread hands to GTK's side.
#[derive(Default)]
struct Shared {
    surface: Option<Surface>,
    dirty: bool,
    cursor: Option<Cursor>,
    cursor_on: Option<bool>,
    cursor_changed: bool,
}

fn number<T: FromVariant>(arguments: &glib::Variant, index: usize) -> Option<T> {
    arguments.child_value(index).get()
}

fn bytes(arguments: &glib::Variant, index: usize) -> glib::Bytes {
    arguments.child_value(index).data_as_bytes()
}

fn on_scanout(shared: &mut Shared, arguments: &glib::Variant) -> Option<()> {
    let (width, height) = (number(arguments, 0)?, number(arguments, 1)?);
    let (stride, format) = (number(arguments, 2)?, number(arguments, 3)?);
    shared.surface = Surface::new(width, height, stride, format, bytes(arguments, 4).to_vec());
    shared.dirty = true;
    Some(())
}

fn on_update(shared: &mut Shared, arguments: &glib::Variant) -> Option<()> {
    let (x, y) = (number(arguments, 0)?, number(arguments, 1)?);
    let (width, height) = (number(arguments, 2)?, number(arguments, 3)?);
    let stride = number(arguments, 4)?;
    shared.surface.as_mut()?.write(x, y, width, height, stride, &bytes(arguments, 6));
    shared.dirty = true;
    Some(())
}

fn on_scanout_map(shared: &mut Shared, arguments: &glib::Variant, message: &gio::DBusMessage) -> Option<()> {
    let handle: Handle = number(arguments, 0)?;
    let file = message.unix_fd_list()?.get(handle.0).ok()?;
    let offset: u32 = number(arguments, 1)?;
    let (width, height) = (number(arguments, 2)?, number(arguments, 3)?);
    let (stride, format) = (number(arguments, 4)?, number(arguments, 5)?);
    shared.surface = Some(Surface::shared(width, height, stride, format, file, offset.into()));
    shared.dirty = true;
    Some(())
}

fn on_cursor_define(shared: &mut Shared, arguments: &glib::Variant) -> Option<()> {
    let (width, height) = (number(arguments, 0)?, number(arguments, 1)?);
    let (hot_x, hot_y) = (number(arguments, 2)?, number(arguments, 3)?);
    let pixels = bytes(arguments, 4).to_vec();
    shared.cursor = Some(Cursor { width, height, hot_x, hot_y, pixels });
    shared.cursor_changed = true;
    Some(())
}

fn on_call(shared: &Mutex<Shared>, method: &str, arguments: &glib::Variant, message: &gio::DBusMessage) {
    let Ok(mut shared) = shared.lock() else { return };
    let shared = &mut *shared;
    match method {
        "Scanout" => on_scanout(shared, arguments),
        "Update" => on_update(shared, arguments),
        "ScanoutMap" => on_scanout_map(shared, arguments, message),
        "UpdateMap" => {
            shared.dirty = true;
            Some(())
        }
        "CursorDefine" => on_cursor_define(shared, arguments),
        "MouseSet" => number::<i32>(arguments, 2).map(|on| {
            shared.cursor_on = Some(on != 0);
            shared.cursor_changed = true;
        }),
        _ => None,
    };
}

fn register(connection: &gio::DBusConnection, shared: &Arc<Mutex<Shared>>) -> Result<(), glib::Error> {
    let info = gio::DBusNodeInfo::for_xml(LISTENER_XML)?;
    for name in [LISTENER, LISTENER_MAP] {
        let Some(interface) = info.lookup_interface(name) else { continue };
        let calling = shared.clone();
        connection
            .register_object(LISTENER_PATH, &interface)
            .method_call(move |_, _, _, _, method, arguments, invocation| {
                on_call(&calling, method, &arguments, &invocation.message());
                invocation.return_value(None);
            })
            .property(|_, _, _, _, _| vec![LISTENER_MAP].to_variant())
            .build()?;
    }
    Ok(())
}

/// The thread's whole life: joins QEMU's display, and answers it until one side stops.
fn run(monitor: &Path, shared: &Arc<Mutex<Shared>>, alive: &Arc<AtomicBool>, context: &glib::MainContext) {
    let serve = || -> Option<()> {
        let connection = attach(monitor).ok()?;
        let ending = alive.clone();
        let waking = context.clone();
        connection.connect_closed(move |_, _, _| {
            ending.store(false, Ordering::Relaxed);
            waking.wakeup();
        });
        register(&connection, shared).ok()?;
        connection.start_message_processing();
        while alive.load(Ordering::Relaxed) {
            context.iteration(true);
        }
        Some(())
    };
    let _ = context.with_thread_default(serve);
    alive.store(false, Ordering::Relaxed);
}

pub struct DbusFrames {
    shared: Arc<Mutex<Shared>>,
    alive: Arc<AtomicBool>,
    context: glib::MainContext,
}

impl DbusFrames {
    /// Returns at once. Whether QEMU is there shows in `alive`.
    pub fn connect(monitor: PathBuf) -> Self {
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let context = glib::MainContext::new();
        let (running, living, waking) = (shared.clone(), alive.clone(), context.clone());
        thread::spawn(move || run(&monitor, &running, &living, &waking));
        Self { shared, alive, context }
    }
}

impl Drop for DbusFrames {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
        self.context.wakeup();
    }
}

fn memory_format(format: u32) -> Option<gdk::MemoryFormat> {
    match format {
        PIXMAN_X8R8G8B8 => Some(gdk::MemoryFormat::B8g8r8x8),
        PIXMAN_A8R8G8B8 => Some(gdk::MemoryFormat::B8g8r8a8),
        _ => None,
    }
}

impl FrameSource for DbusFrames {
    fn newest(&mut self) -> Option<gdk::Texture> {
        let mut shared = self.shared.lock().ok()?;
        if !std::mem::take(&mut shared.dirty) {
            return None;
        }
        let surface = shared.surface.as_mut()?;
        surface.refresh();
        let format = memory_format(surface.format)?;
        let pixels = glib::Bytes::from(&surface.pixels);
        let texture = gdk::MemoryTexture::new(
            surface.width as i32,
            surface.height as i32,
            format,
            &pixels,
            surface.stride as usize,
        );
        Some(texture.upcast())
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// A guest on VGA draws its pointer into the frame and QEMU says nothing about it.
    fn new_cursor(&mut self) -> Option<gdk::Cursor> {
        let mut shared = self.shared.lock().ok()?;
        if !std::mem::take(&mut shared.cursor_changed) {
            return None;
        }
        if shared.cursor_on == Some(false) {
            return gdk::Cursor::from_name("none", None);
        }
        let cursor = shared.cursor.as_ref()?;
        let texture = gdk::MemoryTexture::new(
            cursor.width,
            cursor.height,
            gdk::MemoryFormat::B8g8r8a8,
            &glib::Bytes::from(&cursor.pixels),
            (cursor.width * BYTES_PER_PIXEL) as usize,
        );
        Some(gdk::Cursor::from_texture(&texture, cursor.hot_x, cursor.hot_y, None))
    }
}
