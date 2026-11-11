//! Where the pointer is on the other computer, and when it has been pushed home.

use super::*;
use crate::proto::{REL_X, REL_Y};

const CLICK: f64 = 120.0;
const WHEEL_UNITS: f64 = 15.0;
const INSET: i32 = 8;
const RETURN_WINDOW: f64 = 1.0;
const RETURN_PUSH: f64 = 16.0;

#[derive(Debug)]
pub(super) struct Away {
    pub(super) x: f64,
    pub(super) y: f64,
    layout: Layout,
    side: Side,
    push: f64,
    push_at: f64,
    rest: (f64, f64),
    wheel: [f64; 2],
}

impl Away {
    pub(super) fn new(side: Side, along: f64, layout: Layout) -> Away {
        let (x, y) = layout.landing(side, along, INSET);
        Away { x, y, layout, side, push: 0.0, push_at: 0.0, rest: (0.0, 0.0), wheel: [0.0; 2] }
    }

    fn home(&self) -> Side {
        self.side.opposite()
    }

    pub(super) fn step(&mut self, dx: f64, dy: f64, now: f64) -> (Vec<Event>, bool) {
        let (want_x, want_y) = (self.x + dx, self.y + dy);
        let (x, y) = self.layout.keep(want_x, want_y);
        let at_home_edge = self.layout.open_towards(self.home(), x, y);
        let (pressing, sent) = match self.side {
            Side::Right => ((x - want_x).max(0.0), (dx.max(x - self.x), dy)),
            Side::Left => ((want_x - x).max(0.0), (dx.min(x - self.x), dy)),
            Side::Bottom => ((y - want_y).max(0.0), (dx, dy.max(y - self.y))),
            Side::Top => ((want_y - y).max(0.0), (dx, dy.min(y - self.y))),
        };
        let (pressing, sent) = if at_home_edge { (pressing, sent) } else { (0.0, (dx, dy)) };
        self.x = x;
        self.y = y;
        let mut events = Vec::new();
        self.rest.0 += sent.0;
        self.rest.1 += sent.1;
        let (ix, iy) = (self.rest.0.round(), self.rest.1.round());
        self.rest.0 -= ix;
        self.rest.1 -= iy;
        if ix != 0.0 {
            events.push((EV_REL, REL_X, ix as i32));
        }
        if iy != 0.0 {
            events.push((EV_REL, REL_Y, iy as i32));
        }
        let along_home = match self.side {
            Side::Right | Side::Left => dy.abs(),
            Side::Top | Side::Bottom => dx.abs(),
        };
        if pressing > 0.0 && pressing >= along_home {
            if now - self.push_at > RETURN_WINDOW {
                self.push = 0.0;
            }
            self.push_at = now;
            self.push += pressing;
            if self.push >= RETURN_PUSH {
                self.push = 0.0;
                return (events, true);
            }
        } else if !self.touching_home() {
            self.push = 0.0;
        }
        (events, false)
    }

    fn touching_home(&self) -> bool {
        self.layout.open_towards(self.home(), self.x, self.y)
    }

    pub(super) fn along_home(&self) -> f64 {
        let (x0, y0, x1, y1) = self.layout.bounds();
        let frac = |v: f64, lo: i32, hi: i32| ((v - f64::from(lo)) / f64::from((hi - lo - 1).max(1))).clamp(0.0, 1.0);
        match self.side {
            Side::Left | Side::Right => frac(self.y, y0, y1),
            Side::Top | Side::Bottom => frac(self.x, x0, x1),
        }
    }

    pub(super) fn scroll(&mut self, axis: u32, value: f64, v120: i32) -> Option<Event> {
        let i = usize::from(axis == 1);
        self.wheel[i] += if v120 != 0 { f64::from(v120) } else { value / WHEEL_UNITS * CLICK };
        let clicks = (self.wheel[i] / CLICK).trunc();
        if clicks == 0.0 {
            return None;
        }
        self.wheel[i] -= clicks * CLICK;
        let clicks = clicks as i32;
        Some(if i == 1 { (EV_REL, REL_HWHEEL, clicks) } else { (EV_REL, REL_WHEEL, -clicks) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::greet::arrived;
    use crate::server::tests::shared;

    #[test]
    fn the_pointer_lands_just_inside_the_edge_facing_this_computer() {
        let one = Layout::single(1280, 720);
        let at = |side, along| {
            let a = Away::new(side, along, one.clone());
            (a.x as i32, a.y as i32)
        };
        assert_eq!(at(Side::Right, 0.5), (8, 360));
        assert_eq!(at(Side::Left, 0.0), (1271, 0));
        assert_eq!(at(Side::Bottom, 1.0), (1279, 8));
        assert_eq!(at(Side::Top, 0.25), (320, 711));
    }

    #[test]
    fn moves_are_sent_whole_and_a_nudge_home_comes_home() {
        let mut a = Away::new(Side::Right, 0.5, Layout::single(1280, 720));
        let (sent, home) = a.step(10.4, -0.3, 0.0);
        assert_eq!((sent, home), (vec![(EV_REL, REL_X, 10)], false));
        let (sent, _) = a.step(0.2, -0.3, 0.01);
        assert_eq!(sent, vec![(EV_REL, REL_X, 1), (EV_REL, REL_Y, -1)]);
        let (sent, home) = a.step(-18.6, 0.0, 0.02);
        assert_eq!((sent, home), (vec![(EV_REL, REL_X, -19)], false));
        assert!(a.touching_home());
        assert_eq!(a.step(-10.0, 0.0, 0.03), (vec![], false));
        assert_eq!(a.step(-10.0, 1.0, 0.04), (vec![(EV_REL, REL_Y, 1)], true));
        assert!((a.along_home() - 360.4 / 719.0).abs() < 1e-9);
        let mut b = Away::new(Side::Right, 0.5, Layout::single(1280, 720));
        assert!(b.step(-200.0, 0.0, 0.0).1);
    }

    #[test]
    fn sliding_along_the_edge_facing_home_never_comes_home() {
        let mut a = Away::new(Side::Bottom, 0.5, Layout::single(1280, 720));
        a.step(0.0, -100.0, 0.0);
        for i in 0..100 {
            let drift = if i % 3 == 0 { -2.0 } else { 0.0 };
            assert!(!a.step(8.0, drift, 0.01 * f64::from(i)).1, "move {i}");
        }
        for i in 0..10 {
            assert!(!a.step(0.0, -10.0, 10.0 + 2.0 * f64::from(i)).1);
        }
    }

    fn laptop() -> Layout {
        use crate::layout::Rect;
        Layout::new([Rect { x: 1290, y: 0, w: 1920, h: 1080 }, Rect { x: 3210, y: 0, w: 1289, h: 859 }])
    }

    fn pushes_home(a: &mut Away, dx: f64, dy: f64) -> Option<usize> {
        (0..20).find(|i| a.step(dx, dy, 0.01 * *i as f64).1)
    }

    #[test]
    fn an_empty_margin_is_no_travel_going_in_and_none_coming_back() {
        let mut a = Away::new(Side::Right, 0.5, laptop());
        assert_eq!((a.x, a.y), (1298.0, 540.0));
        let (sent, home) = a.step(-5000.0, 0.0, 0.0);
        assert_eq!((a.x, home), (1290.0, true));
        assert_eq!(sent, vec![(EV_REL, REL_X, -8)]);
        let mut b = Away::new(Side::Right, 0.5, laptop());
        b.step(-20.0, 0.0, 0.0);
        assert!(b.touching_home());
        assert!(pushes_home(&mut b, -4.0, 0.0).is_some_and(|n| n <= 4));
    }

    #[test]
    fn scaled_screens_count_in_points_and_a_short_screen_is_left_by_its_own_edge() {
        let mut a = Away::new(Side::Left, 0.5, laptop());
        assert_eq!((a.x, a.y), (4490.0, 540.0));
        assert!(pushes_home(&mut a, 6.0, 0.0).is_some_and(|n| n <= 4));
        let mut b = Away::new(Side::Left, 0.5, laptop());
        for _ in 0..40 {
            b.step(-100.0, 0.0, 0.0);
        }
        assert_eq!(b.x, 1290.0);
        assert!(!b.step(60.0, 0.0, 1.0).1);
        let mut c = Away::new(Side::Left, 1.0, laptop());
        assert_eq!((c.x, c.y), (3201.0, 1079.0));
        assert!(pushes_home(&mut c, 6.0, 0.0).is_some_and(|n| n <= 4));
    }

    #[test]
    fn stacked_screens_come_in_at_the_top_or_bottom_and_leave_by_the_same_edge() {
        use crate::layout::Rect;
        let stacked = Layout::new([Rect { x: 0, y: 0, w: 1920, h: 1080 }, Rect { x: 0, y: 1080, w: 1280, h: 1024 }]);
        let mut below = Away::new(Side::Bottom, 0.5, stacked.clone());
        assert_eq!((below.x, below.y), (960.0, 8.0));
        assert!(pushes_home(&mut below, 0.0, -6.0).is_some_and(|n| n <= 4));
        let mut above = Away::new(Side::Top, 0.9, stacked.clone());
        assert_eq!((above.x, above.y), (1727.0, 1071.0));
        assert!(pushes_home(&mut above, 0.0, 6.0).is_some_and(|n| n <= 4));
        let mut mid = Away::new(Side::Bottom, 0.1, stacked);
        for _ in 0..60 {
            assert!(!mid.step(0.0, 40.0, 0.0).1);
        }
        assert_eq!(mid.y, 2103.0);
    }

    #[test]
    fn a_screen_shorter_than_its_neighbour_does_not_end_the_push_early() {
        use crate::layout::Rect;
        let uneven = Layout::new([Rect { x: 0, y: 0, w: 1000, h: 800 }, Rect { x: 1000, y: 0, w: 1000, h: 500 }]);
        let mut a = Away::new(Side::Left, 0.0, uneven.clone());
        a.x = 990.0;
        a.y = 300.0;
        assert!(!a.step(20.0, 0.0, 0.0).1);
        assert_eq!(a.x, 1010.0);
        let mut b = Away::new(Side::Left, 0.0, uneven);
        b.x = 990.0;
        b.y = 700.0;
        assert!(pushes_home(&mut b, 8.0, 0.0).is_some_and(|n| n <= 4));
    }

    #[test]
    fn a_pong_carries_the_screens_and_the_gap_is_kept() {
        let _home = crate::config::tests::TestHome::new();
        let (me, _) = shared(json!({}));
        let (link, _near) = crate::link::tests::pair();
        lock(&me.peers).push(Peer { id: "lap".into(), name: "Lap".into(), layout: Layout::single(1920, 1080), link: link.clone() });
        arrived(&me, "lap", &link, proto::PONG, proto::pong(&laptop())[5..].to_vec());
        assert_eq!(lock(&me.peers)[0].layout, laptop());
        let plain = serde_json::to_vec(&json!({"width": 1920, "height": 1080})).unwrap();
        assert_eq!(proto::pong_layout(&plain), Some(Layout::single(1920, 1080)));
    }
}
