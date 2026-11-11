//! Where the screens of this desktop are, in the units the pointer moves in.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1 as manager, zxdg_output_v1 as xdg};

use crate::config::{key, Config};
use crate::layout::{Layout, Rect};

#[derive(Default)]
struct Found {
    logical: Vec<(i32, i32, i32, i32)>,
    modes: Vec<(i32, i32, i32)>,
}

pub fn detect() -> Option<Layout> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, mut queue) = registry_queue_init::<Found>(&conn).ok()?;
    let qh = queue.handle();
    let outputs: Vec<wl_output::WlOutput> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == "wl_output")
            .enumerate()
            .map(|(i, g)| globals.registry().bind(g.name, g.version.min(2), &qh, i))
            .collect()
    });
    if outputs.is_empty() {
        return None;
    }
    let mut found = Found { logical: vec![(0, 0, 0, 0); outputs.len()], modes: vec![(0, 0, 1); outputs.len()] };
    if let Ok(mgr) = globals.bind::<manager::ZxdgOutputManagerV1, _, _>(&qh, 1..=3, ()) {
        for (i, out) in outputs.iter().enumerate() {
            mgr.get_xdg_output(out, &qh, i);
        }
    }
    queue.roundtrip(&mut found).ok()?;
    queue.roundtrip(&mut found).ok()?;
    layout(&found)
}

fn layout(found: &Found) -> Option<Layout> {
    let placed = Layout::new(found.logical.iter().map(|&(x, y, w, h)| Rect { x, y, w, h }));
    if !placed.is_empty() {
        return Some(placed);
    }
    let mut x = 0;
    let side_by_side = found.modes.iter().map(|&(mw, mh, scale)| {
        let s = scale.max(1);
        let r = Rect { x, y: 0, w: mw / s, h: mh / s };
        x += r.w.max(0);
        r
    });
    Some(Layout::new(side_by_side)).filter(|l| !l.is_empty())
}

pub fn size(cfg: &Config) -> Layout {
    detect().unwrap_or_else(|| fallback(cfg))
}

pub fn fallback(cfg: &Config) -> Layout {
    Layout::single(cfg.i64(key::SCREEN_WIDTH) as i32, cfg.i64(key::SCREEN_HEIGHT) as i32)
}

pub fn recent() -> Option<Layout> {
    static RECENT: Mutex<Option<(Instant, Layout)>> = Mutex::new(None);
    let mut held = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, layout)) = held.as_ref().filter(|(when, _)| when.elapsed() < Duration::from_secs(10)) {
        return Some(layout.clone());
    }
    let found = detect()?;
    *held = Some((Instant::now(), found.clone()));
    Some(found)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Found {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_output::WlOutput, usize> for Found {
    fn event(state: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event, &i: &usize, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_output::Event::Mode { flags, width, height, .. } => {
                let current = matches!(flags, wayland_client::WEnum::Value(f) if f.contains(wl_output::Mode::Current));
                if current {
                    state.modes[i].0 = width;
                    state.modes[i].1 = height;
                }
            }
            wl_output::Event::Scale { factor } => state.modes[i].2 = factor,
            _ => {}
        }
    }
}

impl Dispatch<manager::ZxdgOutputManagerV1, ()> for Found {
    fn event(_: &mut Self, _: &manager::ZxdgOutputManagerV1, _: manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<xdg::ZxdgOutputV1, usize> for Found {
    fn event(state: &mut Self, _: &xdg::ZxdgOutputV1, event: xdg::Event, &i: &usize, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            xdg::Event::LogicalPosition { x, y } => {
                state.logical[i].0 = x;
                state.logical[i].1 = y;
            }
            xdg::Event::LogicalSize { width, height } => {
                state.logical[i].2 = width;
                state.logical[i].3 = height;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_is_kept_where_the_desktop_put_it() {
        let two = Found { logical: vec![(1290, 0, 1920, 1080), (3210, 0, 1289, 859)], modes: vec![] };
        assert_eq!(layout(&two).map(|l| l.bounds()), Some((1290, 0, 4499, 1080)));
        let scaled = Found { logical: vec![(0, 0, 0, 0)], modes: vec![(3840, 2160, 2)] };
        assert_eq!(layout(&scaled), Some(Layout::single(1920, 1080)));
        let unplaced = Found { logical: vec![(0, 0, 0, 0); 2], modes: vec![(1920, 1080, 1), (2256, 1504, 2)] };
        assert_eq!(layout(&unplaced).map(|l| l.reach()), Some((3048, 1080)));
        assert_eq!(layout(&Found::default()), None);
    }

    #[test]
    #[ignore = "needs a desktop"]
    fn reads_the_real_desktop() {
        let layout = detect().expect("no screens from the desktop");
        eprintln!("desktop is {layout:?}");
        assert!(!layout.is_empty());
    }
}
