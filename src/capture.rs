//! Taking this computer's mouse and keyboard while the pointer is over on the other computer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

#[allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables, non_upper_case_globals, non_snake_case,
        unused_imports, missing_docs, clippy::all)]
mod generated {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::backend as wayland_backend;
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/input_capture.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/input_capture.xml");
}

use generated::trindle_input_capture_manager_v1::{Side as WlSide, TrindleInputCaptureManagerV1 as Manager};
use generated::trindle_input_capture_v1::{self as cap, TrindleInputCaptureV1};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];

    pub fn opposite(self) -> Side {
        match self {
            Side::Right => Side::Left,
            Side::Left => Side::Right,
            Side::Bottom => Side::Top,
            Side::Top => Side::Bottom,
        }
    }

    pub fn parse(text: &str) -> Option<Side> {
        match text.trim().to_lowercase().as_str() {
            "left" => Some(Side::Left),
            "right" => Some(Side::Right),
            "top" | "above" => Some(Side::Top),
            "bottom" | "below" => Some(Side::Bottom),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "top",
            Side::Bottom => "bottom",
        }
    }

    pub fn said_screen(self) -> &'static str {
        match self {
            Side::Left => "on the left",
            Side::Right => "on the right",
            Side::Top => "at the top",
            Side::Bottom => "at the bottom",
        }
    }

    fn wire(self) -> WlSide {
        match self {
            Side::Left => WlSide::Left,
            Side::Right => WlSide::Right,
            Side::Top => WlSide::Top,
            Side::Bottom => WlSide::Bottom,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Said {
    Ready,
    Taken,
    NotNow,
    NotAsked,
    Activated(f64),
    Motion { dx: f64, dy: f64 },
    Button { button: u32, pressed: bool },
    Axis { axis: u32, value: f64, v120: i32 },
    Key { key: u32, pressed: bool },
    Deactivated { by_flow: bool },
}

pub fn not_allowed() -> String {
    "Flow has not been allowed to share your mouse and keyboard with another computer, so the pointer cannot \
     cross to it. Allow it in Settings, Permissions. If you have just updated Trindle, log out and back in once."
        .to_string()
}

pub struct Captures {
    conn: Connection,
    manager: Manager,
    qh: QueueHandle<Listen>,
    sides: HashMap<Side, TrindleInputCaptureV1>,
    lost: Arc<AtomicBool>,
}

pub type Tell = Box<dyn Fn(Option<(Side, Said)>) + Send>;

struct Listen(Tell);

impl Captures {
    pub fn start(tell: Tell) -> Result<Captures, String> {
        let conn = Connection::connect_to_env().map_err(|_| "Flow cannot reach the desktop.".to_string())?;
        let (globals, mut queue) = registry_queue_init::<Listen>(&conn).map_err(|e| format!("the desktop did not answer ({e})"))?;
        let qh = queue.handle();
        let manager: Manager = globals.bind(&qh, 1..=2, ()).map_err(|_| not_allowed())?;
        let mut listen = Listen(tell);
        queue.roundtrip(&mut listen).map_err(|e| format!("the desktop did not answer ({e})"))?;
        let lost = Arc::new(AtomicBool::new(false));
        let gone = lost.clone();
        std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                while queue.blocking_dispatch(&mut listen).is_ok() {}
                gone.store(true, Ordering::Relaxed);
                (listen.0)(None);
            })
            .map_err(|e| e.to_string())?;
        Ok(Captures { conn, manager, qh, sides: HashMap::new(), lost })
    }

    pub fn arm(&mut self, side: Side) {
        if let Some(old) = self.sides.remove(&side) {
            old.destroy();
        }
        let capture = self.manager.get_capture(side.wire(), &self.qh, side);
        self.sides.insert(side, capture);
        let _ = self.conn.flush();
    }

    pub fn disarm(&mut self, side: Side) {
        if let Some(old) = self.sides.remove(&side) {
            old.destroy();
            let _ = self.conn.flush();
        }
    }

    pub fn can_take(&self) -> bool {
        wayland_client::Proxy::version(&self.manager) >= 2
    }

    pub fn take(&self, side: Side) -> bool {
        match self.sides.get(&side) {
            Some(c) if self.can_take() => {
                c.take(side.wire() as u32);
                let _ = self.conn.flush();
                true
            }
            _ => false,
        }
    }

    pub fn set_manual(&self, manual: bool) {
        if !self.can_take() {
            return;
        }
        if let Some(c) = self.sides.values().next() {
            c.set_mode(if manual { cap::Mode::Manual } else { cap::Mode::Push });
            let _ = self.conn.flush();
        }
    }

    pub fn armed(&self, side: Side) -> bool {
        self.sides.contains_key(&side)
    }

    pub fn armed_sides(&self) -> Vec<Side> {
        self.sides.keys().copied().collect()
    }

    pub fn release(&self, side: Side, along: f64) {
        if let Some(c) = self.sides.get(&side) {
            c.release(along.clamp(0.0, 1.0));
            let _ = self.conn.flush();
        }
    }

    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }
}

impl Drop for Captures {
    fn drop(&mut self) {
        for (_, c) in self.sides.drain() {
            c.destroy();
        }
        crate::control::hang_up(&self.conn);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Listen {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<Manager, ()> for Listen {
    fn event(_: &mut Self, _: &Manager, _: <Manager as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<TrindleInputCaptureV1, Side> for Listen {
    fn event(state: &mut Self, _: &TrindleInputCaptureV1, event: cap::Event, side: &Side, _: &Connection, _: &QueueHandle<Self>) {
        (state.0)(Some((*side, said(event))));
    }
}

fn said(event: cap::Event) -> Said {
    match event {
        cap::Event::Ready => Said::Ready,
        cap::Event::Unavailable { reason } => match reason {
            WEnum::Value(cap::UnavailableReason::NotNow) => Said::NotNow,
            WEnum::Value(cap::UnavailableReason::NotAsked) => Said::NotAsked,
            _ => Said::Taken,
        },
        cap::Event::Activated { along } => Said::Activated(along),
        cap::Event::Motion { dx, dy, .. } => Said::Motion { dx, dy },
        cap::Event::Button { button, state, .. } => Said::Button { button, pressed: state != 0 },
        cap::Event::Axis { axis, value, v120, .. } => Said::Axis { axis, value, v120 },
        cap::Event::Key { key, state, .. } => Said::Key { key, pressed: state != 0 },
        cap::Event::Deactivated { reason } => {
            Said::Deactivated { by_flow: matches!(reason, WEnum::Value(cap::DeactivatedReason::Released)) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides_read_from_the_file_and_the_page() {
        assert_eq!(Side::parse("right"), Some(Side::Right));
        assert_eq!(Side::parse("Above"), Some(Side::Top));
        assert_eq!(Side::parse(" Below "), Some(Side::Bottom));
        assert_eq!(Side::parse("Left"), Some(Side::Left));
        assert_eq!(Side::parse("middle"), None);
        for s in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
            assert_eq!(Side::parse(s.name()), Some(s));
        }
    }

    #[test]
    fn what_the_desktop_says_is_read_plainly() {
        assert_eq!(said(cap::Event::Key { time: 1, key: 30, state: 1 }), Said::Key { key: 30, pressed: true });
        assert_eq!(said(cap::Event::Unavailable { reason: WEnum::Value(cap::UnavailableReason::NotNow) }), Said::NotNow);
        assert_eq!(
            said(cap::Event::Deactivated { reason: WEnum::Value(cap::DeactivatedReason::SafetyKeys) }),
            Said::Deactivated { by_flow: false }
        );
    }
}
