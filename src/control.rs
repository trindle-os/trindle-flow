//! Moving the pointer and typing here, for the other computer's mouse and keyboard.

use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_keyboard, wl_pointer, wl_registry, wl_seat};
use wayland_client::{delegate_noop, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

use crate::layout::Layout;
use crate::proto::{Event, EV_KEY, EV_REL, REL_HWHEEL, REL_WHEEL, REL_X, REL_Y};

const WHEEL_STEP: f64 = 15.0;
const XKB_V1: u32 = 1;

pub fn not_allowed() -> String {
    "Flow has not been allowed to control your keyboard, mouse and clipboard, so the other computer cannot \
     control this one. Allow it in Settings, Permissions."
        .to_string()
}

pub struct Remote {
    conn: Connection,
    devices: Mutex<Devices>,
    lost: Arc<AtomicBool>,
}

struct Devices {
    pointer: ZwlrVirtualPointerV1,
    keyboard: ZwpVirtualKeyboardV1,
    started: Instant,
    keys: Vec<u32>,
    buttons: Vec<u32>,
}

#[derive(Default)]
struct Setup {
    keymap: Option<(u32, OwnedFd, u32)>,
}

impl Remote {
    pub fn start() -> Result<Remote, String> {
        let conn = Connection::connect_to_env().map_err(|_| "Flow cannot reach the desktop.".to_string())?;
        let (globals, mut queue) = registry_queue_init::<Setup>(&conn).map_err(|e| format!("the desktop did not answer ({e})"))?;
        let qh = queue.handle();
        let pointers: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).map_err(|_| not_allowed())?;
        let keyboards: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| not_allowed())?;
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).map_err(|_| "This desktop has no seat.".to_string())?;

        let real = seat.get_keyboard(&qh, ());
        let mut setup = Setup::default();
        for _ in 0..10 {
            if setup.keymap.is_some() {
                break;
            }
            queue.roundtrip(&mut setup).map_err(|e| format!("the desktop did not answer ({e})"))?;
        }
        let (format, fd, size) = setup.keymap.take().ok_or("The desktop gave no keyboard layout.")?;
        if format != XKB_V1 {
            return Err("The desktop's keyboard layout is of a kind Flow cannot pass on.".into());
        }
        let keyboard = keyboards.create_virtual_keyboard(&seat, &qh, ());
        keyboard.keymap(format, fd.as_fd(), size);
        let pointer = pointers.create_virtual_pointer(Some(&seat), &qh, ());
        if real.version() >= 3 {
            real.release();
        }
        queue.roundtrip(&mut setup).map_err(|e| format!("the desktop did not answer ({e})"))?;

        let lost = Arc::new(AtomicBool::new(false));
        let gone = lost.clone();
        std::thread::Builder::new()
            .name("control".into())
            .spawn(move || {
                while queue.blocking_dispatch(&mut setup).is_ok() {}
                gone.store(true, Ordering::Relaxed);
            })
            .map_err(|e| e.to_string())?;
        let devices = Devices { pointer, keyboard, started: Instant::now(), keys: Vec::new(), buttons: Vec::new() };
        Ok(Remote { conn, devices: Mutex::new(devices), lost })
    }

    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    pub fn emit(&self, events: &[Event]) {
        let mut d = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        let time = d.started.elapsed().as_millis() as u32;
        let (dx, dy, calls) = translate(events);
        if dx != 0.0 || dy != 0.0 {
            d.pointer.motion(time, dx, dy);
            d.pointer.frame();
        }
        for call in calls {
            match call {
                Call::Axis(axis, steps) => {
                    let axis = if axis == 1 { wl_pointer::Axis::HorizontalScroll } else { wl_pointer::Axis::VerticalScroll };
                    d.pointer.axis_source(wl_pointer::AxisSource::Wheel);
                    d.pointer.axis_discrete(time, axis, WHEEL_STEP * f64::from(steps), steps);
                    d.pointer.frame();
                }
                Call::Button(b, s) => {
                    d.pointer.button(time, b, if s == 1 { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released });
                    d.pointer.frame();
                    held(&mut d.buttons, b, s == 1);
                }
                Call::Key(k, s) => {
                    d.keyboard.key(time, k, s);
                    held(&mut d.keys, k, s == 1);
                }
            }
        }
        let _ = self.conn.flush();
    }

    pub fn place(&self, x: i32, y: i32, layout: &Layout) {
        let (x, y, w, h) = layout.absolute(x, y);
        let (w, h) = (w.max(1), h.max(1));
        let d = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        let time = d.started.elapsed().as_millis() as u32;
        d.pointer.motion_absolute(time, x.min(w - 1), y.min(h - 1), w, h);
        d.pointer.frame();
        let _ = self.conn.flush();
    }

    pub fn release_all(&self) {
        let mut d = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        let time = d.started.elapsed().as_millis() as u32;
        for k in std::mem::take(&mut d.keys) {
            d.keyboard.key(time, k, 0);
        }
        for b in std::mem::take(&mut d.buttons) {
            d.pointer.button(time, b, wl_pointer::ButtonState::Released);
            d.pointer.frame();
        }
        let _ = self.conn.flush();
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.release_all();
        hang_up(&self.conn);
    }
}

pub fn hang_up(conn: &Connection) {
    let _ = conn.flush();
    // SAFETY: shutdown on a descriptor the connection keeps open; it only
    // ends the traffic, and the owner still closes it.
    unsafe { libc::shutdown(std::os::fd::AsRawFd::as_raw_fd(&conn.backend().poll_fd()), libc::SHUT_RDWR) };
}

fn held(list: &mut Vec<u32>, code: u32, down: bool) {
    list.retain(|c| *c != code);
    if down {
        list.push(code);
    }
}

#[derive(Debug, PartialEq)]
enum Call {
    Axis(u32, i32),
    Button(u32, u32),
    Key(u32, u32),
}

fn translate(events: &[Event]) -> (f64, f64, Vec<Call>) {
    let (mut dx, mut dy) = (0.0, 0.0);
    let mut calls: Vec<Call> = Vec::new();
    for &(t, c, v) in events {
        match (t, c) {
            (EV_REL, REL_X) => dx += f64::from(v),
            (EV_REL, REL_Y) => dy += f64::from(v),
            (EV_REL, REL_WHEEL) => calls.push(Call::Axis(0, v.saturating_neg())),
            (EV_REL, REL_HWHEEL) => calls.push(Call::Axis(1, v)),
            (EV_KEY, code @ 0x110..=0x117) => calls.push(Call::Button(u32::from(code), state(v))),
            (EV_KEY, code) if v != 2 && (code < 0x100 || (352..=542).contains(&code)) => {
                calls.push(Call::Key(u32::from(code), state(v)));
            }
            _ => {}
        }
    }
    (dx, dy, calls)
}

fn state(v: i32) -> u32 {
    u32::from(v != 0)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Setup {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Setup {
    fn event(state: &mut Self, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_keyboard::Event::Keymap { format, fd, size } = event {
            let format = match format {
                WEnum::Value(f) => f as u32,
                WEnum::Unknown(n) => n,
            };
            state.keymap = Some((format, fd, size));
        }
    }
}

delegate_noop!(Setup: ignore wl_seat::WlSeat);
delegate_noop!(Setup: ZwlrVirtualPointerManagerV1);
delegate_noop!(Setup: ZwlrVirtualPointerV1);
delegate_noop!(Setup: ZwpVirtualKeyboardManagerV1);
delegate_noop!(Setup: ZwpVirtualKeyboardV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extreme_wheel_value_does_not_overflow() {
        assert_eq!(translate(&[(EV_REL, REL_WHEEL, i32::MIN)]).2, vec![Call::Axis(0, i32::MAX)]);
    }

    #[test]
    fn events_become_pointer_and_keyboard_requests() {
        let (rel, key) = (EV_REL, EV_KEY);
        let (dx, dy, calls) = translate(&[
            (rel, REL_X, 3),
            (rel, REL_X, 2),
            (rel, REL_Y, -4),
            (rel, REL_WHEEL, 1),
            (rel, 11, 120),
            (key, 0x110, 1),
            (key, 30, 1),
            (key, 30, 2),
            (key, 30, 0),
            (key, 0x150, 1),
        ]);
        assert_eq!((dx, dy), (5.0, -4.0));
        assert_eq!(calls, vec![Call::Axis(0, -1), Call::Button(0x110, 1), Call::Key(30, 1), Call::Key(30, 0)]);
    }

    #[test]
    fn what_is_held_is_remembered_until_let_go() {
        let mut keys = Vec::new();
        held(&mut keys, 42, true);
        held(&mut keys, 30, true);
        held(&mut keys, 42, true);
        assert_eq!(keys, [30, 42]);
        held(&mut keys, 30, false);
        assert_eq!(keys, [42]);
    }
}
