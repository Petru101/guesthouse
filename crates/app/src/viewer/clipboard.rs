//! Shares text between the host's clipboard and the guest's while a screen is open.
//!
//! Wayland only shows a window the clipboard, and only lets it set it, while it has keyboard focus. So
//! what is on the host's clipboard reaches the guest when the user comes to this window, and what the
//! guest copies reaches the host then too. A thread asks the guest, as the agent can take its time.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use guesthouse_app_core::agent::{
    MOST_CLIPBOARD_TEXT, read_guest_clipboard, start_guest_clipboard, stop_guest_clipboard,
    write_guest_clipboard,
};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::machine::Machine;
use guesthouse_daemon::slot::Slot;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

const BETWEEN_LOOKS: Duration = Duration::from_secs(1);
const BETWEEN_STARTS: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Texts {
    /// The text both sides have.
    last: Option<String>,
    for_host: Option<String>,
}

pub struct SharedClipboard {
    clipboard: gdk::Clipboard,
    window: adw::ApplicationWindow,
    changed: Option<glib::SignalHandlerId>,
    active: Option<glib::SignalHandlerId>,
    /// Dropping this ends the thread, which is why it's kept.
    _for_guest: Sender<String>,
}

fn give_host(window: &adw::ApplicationWindow, clipboard: &gdk::Clipboard, texts: &RefCell<Texts>) {
    if window.is_active()
        && let Some(text) = texts.borrow_mut().for_host.take()
    {
        clipboard.set_text(&text);
    }
}

fn read_host(clipboard: &gdk::Clipboard, texts: &Rc<RefCell<Texts>>, for_guest: &Sender<String>) {
    // What this just set is local, and it knows that text.
    if clipboard.is_local() {
        return;
    }
    let (texts, for_guest) = (texts.clone(), for_guest.clone());
    clipboard.read_text_async(gio::Cancellable::NONE, move |result| {
        // Not text, or the program that owns it has gone.
        let Ok(Some(text)) = result else { return };
        let text = text.to_string();
        let mut texts = texts.borrow_mut();
        if !text.is_empty() && texts.last.as_ref() != Some(&text) && text.len() <= MOST_CLIPBOARD_TEXT {
            texts.last = Some(text.clone());
            let _ = for_guest.send(text);
        }
    });
}

/// Says whether the helper answered.
fn look_at_guest(
    context: &Context,
    machine: &Machine,
    slot: Option<Slot>,
    waiting: &mut Option<String>,
    from_guest: &async_channel::Sender<String>,
) -> bool {
    if let Some(text) = waiting.as_ref()
        && !write_guest_clipboard(context, machine, text, slot).unwrap_or(false)
    {
        return false;
    }
    *waiting = None;
    match read_guest_clipboard(context, machine, slot) {
        Ok(Some(text)) => {
            if !text.is_empty() && text.len() <= MOST_CLIPBOARD_TEXT {
                let _ = from_guest.try_send(text);
            }
            true
        }
        _ => false,
    }
}

/// A guest that restarted has lost its helper, so one that stops answering is started again.
fn run(
    context: &Context,
    machine: &Machine,
    slot: Option<Slot>,
    wake: &Receiver<String>,
    from_guest: &async_channel::Sender<String>,
) {
    let mut started = false;
    let mut waiting = None;
    loop {
        if !started {
            started = start_guest_clipboard(context, machine, slot).unwrap_or(false);
        }
        started = started && look_at_guest(context, machine, slot, &mut waiting, from_guest);
        match wake.recv_timeout(if started { BETWEEN_LOOKS } else { BETWEEN_STARTS }) {
            Ok(text) => waiting = Some(text),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    stop_guest_clipboard(context, machine, slot);
}

impl SharedClipboard {
    pub fn new(
        window: &adw::ApplicationWindow,
        context: Arc<Context>,
        machine: Machine,
        slot: Option<Slot>,
    ) -> Self {
        let clipboard = WidgetExt::display(window).clipboard();
        let texts: Rc<RefCell<Texts>> = Rc::default();
        let (for_guest, wake) = channel();
        let (from_guest, guest_texts) = async_channel::unbounded();
        thread::spawn(move || run(&context, &machine, slot, &wake, &from_guest));
        let (taking, giving, to_host) = (texts.clone(), window.clone(), clipboard.clone());
        glib::spawn_future_local(async move {
            while let Ok(text) = guest_texts.recv().await {
                let mut held = taking.borrow_mut();
                if held.last.as_ref() != Some(&text) {
                    held.last = Some(text.clone());
                    held.for_host = Some(text);
                    drop(held);
                    give_host(&giving, &to_host, &taking);
                }
            }
        });
        let (watching, sending) = (texts.clone(), for_guest.clone());
        let changed = clipboard.connect_changed(move |clipboard| read_host(clipboard, &watching, &sending));
        let (held, to_host) = (texts.clone(), clipboard.clone());
        let active = window.connect_is_active_notify(move |window| give_host(window, &to_host, &held));
        read_host(&clipboard, &texts, &for_guest);
        Self {
            clipboard,
            window: window.clone(),
            changed: Some(changed),
            active: Some(active),
            _for_guest: for_guest,
        }
    }
}

impl Drop for SharedClipboard {
    fn drop(&mut self) {
        if let Some(handler) = self.changed.take() {
            self.clipboard.disconnect(handler);
        }
        if let Some(handler) = self.active.take() {
            self.window.disconnect(handler);
        }
    }
}
