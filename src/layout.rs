//! The screens of one computer as rectangles in logical points, and where a pointer may be on them.

use serde_json::{json, Value};

use crate::capture::Side;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn holds(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout(Vec<Rect>);

impl Layout {
    pub fn new(screens: impl IntoIterator<Item = Rect>) -> Layout {
        Layout(screens.into_iter().filter(|r| r.w > 0 && r.h > 0).collect())
    }

    pub fn single(w: i32, h: i32) -> Layout {
        Layout::new([Rect { x: 0, y: 0, w: w.clamp(1, 1 << 16), h: h.clamp(1, 1 << 16) }])
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn bounds(&self) -> (i32, i32, i32, i32) {
        let mut it = self.0.iter();
        let Some(first) = it.next() else { return (0, 0, 1, 1) };
        it.fold((first.x, first.y, first.x + first.w, first.y + first.h), |b, r| {
            (b.0.min(r.x), b.1.min(r.y), b.2.max(r.x + r.w), b.3.max(r.y + r.h))
        })
    }

    pub fn reach(&self) -> (i32, i32) {
        let b = self.bounds();
        (b.2, b.3)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.0.iter().any(|r| r.holds(x, y))
    }

    pub fn to_json(&self) -> Value {
        Value::Array(self.0.iter().map(|r| json!([r.x, r.y, r.w, r.h])).collect())
    }

    pub fn from_json(screens: &Value, width: Option<i64>, height: Option<i64>) -> Layout {
        let limit = |v: &Value| v.as_i64().filter(|n| n.abs() <= 1 << 16).map(|n| n as i32);
        let list = screens.as_array().into_iter().flatten().filter_map(|s| {
            let n: Vec<i32> = s.as_array()?.iter().filter_map(limit).collect();
            (n.len() == 4).then(|| Rect { x: n[0], y: n[1], w: n[2], h: n[3] })
        });
        let found = Layout::new(list.take(32));
        if !found.is_empty() {
            return found;
        }
        let size = |v: Option<i64>, d: i32| v.unwrap_or(i64::from(d)).clamp(1, 1 << 16) as i32;
        Layout::single(size(width, 1920), size(height, 1080))
    }

    pub fn keep(&self, x: f64, y: f64) -> (f64, f64) {
        let on = |r: &Rect| x >= f64::from(r.x) && x < f64::from(r.x + r.w) && y >= f64::from(r.y) && y < f64::from(r.y + r.h);
        if self.0.iter().any(on) {
            return (x, y);
        }
        self.0
            .iter()
            .map(|r| {
                let cx = x.clamp(f64::from(r.x), f64::from(r.x + r.w - 1));
                let cy = y.clamp(f64::from(r.y), f64::from(r.y + r.h - 1));
                ((cx - x).hypot(cy - y), (cx, cy))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map_or((x, y), |(_, p)| p)
    }

    pub fn open_towards(&self, side: Side, x: f64, y: f64) -> bool {
        let (px, py) = (x.floor() as i32, y.floor() as i32);
        let (dx, dy) = match side {
            Side::Left => (-1, 0),
            Side::Right => (1, 0),
            Side::Top => (0, -1),
            Side::Bottom => (0, 1),
        };
        !self.contains(px + dx, py + dy)
    }

    pub fn landing(&self, side: Side, along: f64, inset: i32) -> (f64, f64) {
        let (x0, y0, x1, y1) = self.bounds();
        let across_y = matches!(side, Side::Left | Side::Right);
        let (lo, hi) = if across_y { (y0, y1) } else { (x0, x1) };
        let want = (f64::from(lo) + along.clamp(0.0, 1.0) * f64::from((hi - lo - 1).max(0))).round();
        let entry = match side {
            Side::Right => Side::Left,
            Side::Left => Side::Right,
            Side::Bottom => Side::Top,
            Side::Top => Side::Bottom,
        };
        let mut best: Option<(f64, i32, i32)> = None;
        for r in &self.0 {
            let (a0, a1) = if across_y { (r.y, r.y + r.h) } else { (r.x, r.x + r.w) };
            for t in a0..a1 {
                let (px, py) = match entry {
                    Side::Left => (r.x, t),
                    Side::Right => (r.x + r.w - 1, t),
                    Side::Top => (t, r.y),
                    Side::Bottom => (t, r.y + r.h - 1),
                };
                let miss = (f64::from(t) - want).abs();
                if best.is_none_or(|b| miss < b.0) && self.open_towards(entry, f64::from(px), f64::from(py)) {
                    best = Some((miss, px, py));
                }
            }
        }
        let Some((_, px, py)) = best else { return (f64::from(x0), f64::from(y0)) };
        let r = self.0.iter().find(|r| r.holds(px, py)).copied().unwrap_or(Rect { x: px, y: py, w: 1, h: 1 });
        let step = |n: i32| inset.min((n - 1).max(0));
        let (x, y) = match entry {
            Side::Left => (px + step(r.w), py),
            Side::Right => (px - step(r.w), py),
            Side::Top => (px, py + step(r.h)),
            Side::Bottom => (px, py - step(r.h)),
        };
        (f64::from(x), f64::from(y))
    }

    pub fn absolute(&self, x: i32, y: i32) -> (u32, u32, u32, u32) {
        let (kx, ky) = self.keep(f64::from(x), f64::from(y));
        let (x0, y0, x1, y1) = self.bounds();
        ((kx as i32 - x0) as u32, (ky as i32 - y0) as u32, (x1 - x0) as u32, (y1 - y0) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    fn laptop() -> Layout {
        Layout::new([r(1290, 0, 1920, 1080), r(3210, 0, 1289, 859)])
    }

    #[test]
    fn the_box_starts_where_the_first_screen_does_but_reach_counts_from_zero() {
        assert_eq!(laptop().bounds(), (1290, 0, 4499, 1080));
        assert_eq!(laptop().reach(), (4499, 1080));
    }

    #[test]
    fn coming_in_from_the_right_lands_on_the_screen_that_starts_at_1290_not_at_0() {
        assert_eq!(laptop().landing(Side::Right, 0.5, 8), (1298.0, 540.0));
        assert_eq!(laptop().landing(Side::Right, 0.0, 8), (1298.0, 0.0));
    }

    #[test]
    fn coming_in_from_the_left_lands_on_the_far_screen_scaled_size_and_all() {
        assert_eq!(laptop().landing(Side::Left, 0.5, 8), (4490.0, 540.0));
        assert_eq!(laptop().landing(Side::Left, 1.0, 8), (3201.0, 1079.0));
    }

    #[test]
    fn stacked_screens_enter_on_the_edge_of_the_screen_that_has_one_there() {
        let stacked = Layout::new([r(0, 0, 1920, 1080), r(0, 1080, 1280, 1024)]);
        assert_eq!(stacked.landing(Side::Bottom, 0.5, 8), (960.0, 8.0));
        assert_eq!(stacked.landing(Side::Top, 0.9, 8), (1727.0, 1071.0));
        assert_eq!(stacked.landing(Side::Left, 0.9, 8), (1271.0, 1893.0));
        assert_eq!(stacked.landing(Side::Left, 0.1, 8), (1911.0, 210.0));
    }

    #[test]
    fn an_edge_with_a_neighbour_beyond_it_is_not_an_entry() {
        let two = Layout::new([r(0, 0, 1000, 800), r(1000, 0, 1000, 800)]);
        assert_eq!(two.landing(Side::Right, 0.5, 8), (8.0, 400.0));
        assert_eq!(two.landing(Side::Left, 0.5, 8), (1991.0, 400.0));
    }

    #[test]
    fn the_pointer_is_kept_on_a_real_screen() {
        let l = laptop();
        assert_eq!(l.keep(100.0, 50.0), (1290.0, 50.0));
        assert_eq!(l.keep(3000.0, 500.0), (3000.0, 500.0));
        assert_eq!(l.keep(9000.0, 5000.0), (4498.0, 858.0));
    }

    #[test]
    fn the_made_up_mouse_is_placed_in_the_box_not_from_zero() {
        assert_eq!(laptop().absolute(1298, 540), (8, 540, 3209, 1080));
        assert_eq!(laptop().absolute(10, 10), (0, 10, 3209, 1080));
    }

    #[test]
    fn a_list_of_screens_survives_the_wire_and_old_width_and_height_still_work() {
        let l = laptop();
        assert_eq!(Layout::from_json(&l.to_json(), None, None), l);
        let old = Layout::from_json(&Value::Null, Some(2560), Some(1440));
        assert_eq!(old, Layout::single(2560, 1440));
        assert_eq!(Layout::from_json(&json!([[0, 0, -5, 10], "x", [1]]), Some(800), Some(600)), Layout::single(800, 600));
    }
}
