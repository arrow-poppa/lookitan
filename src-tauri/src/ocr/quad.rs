// Rotated text boxes, as Lookupper's detector makes them (REA: the contour lambda `0x140951940`
// takes the minimum-area rectangle of each text kernel, and `0x14094b2a0` orders its corners).
// A box is a rectangle given by its corners, top left, top right, bottom right, bottom left, where
// the top edge is the one closest to horizontal.

use image::{Rgba, RgbaImage};

pub type Point = [f32; 2];
pub type Quad = [Point; 4];

/// Below this angle a rectangle counts as upright, and the plain box is used: its crop holds the
/// line whole and reads the same, and the word boxes keep hugging the rows of ink. Lookupper
/// straightens every box.
const UPRIGHT_ANGLE: f32 = 2.0 * std::f32::consts::PI / 180.0;

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: Point, b: Point) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn len(a: Point) -> f32 {
    dot(a, a).sqrt()
}

fn cross(o: Point, a: Point, b: Point) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// The convex hull of some points, counter-clockwise (Andrew's monotone chain).
fn hull(mut points: Vec<Point>) -> Vec<Point> {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let mut lower: Vec<Point> = Vec::new();
    for &p in &points {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for &p in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// A rectangle as a centre, the unit vector along its top edge (pointing right, the edge closest
/// to horizontal) and its half width and half height along that edge and across it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub center: Point,
    pub u: Point,
    pub half_w: f32,
    pub half_h: f32,
}

impl Rect {
    /// The unit vector down the rectangle, across its top edge.
    pub fn v(&self) -> Point {
        [-self.u[1], self.u[0]]
    }

    pub fn corners(&self) -> Quad {
        let (c, u, v) = (self.center, self.u, self.v());
        let at = |a: f32, b: f32| [c[0] + u[0] * a + v[0] * b, c[1] + u[1] * a + v[1] * b];
        [at(-self.half_w, -self.half_h), at(self.half_w, -self.half_h), at(self.half_w, self.half_h), at(-self.half_w, self.half_h)]
    }

    /// Whether the rectangle is turned from the axes by more than a hair.
    pub fn rotated(&self) -> bool {
        self.u[1].atan2(self.u[0]).abs() > UPRIGHT_ANGLE
    }

    /// The rectangle grown by `d` on every side.
    pub fn grown(&self, d: f32) -> Rect {
        Rect { half_w: self.half_w + d, half_h: self.half_h + d, ..*self }
    }

    /// The point's position along and across the rectangle, from its centre.
    pub fn local(&self, p: Point) -> (f32, f32) {
        let rel = sub(p, self.center);
        (dot(rel, self.u), dot(rel, self.v()))
    }
}

/// The rectangle of a quad made by `Rect::corners`.
pub fn rect_of(q: &Quad) -> Rect {
    let top = sub(q[1], q[0]);
    let side = sub(q[3], q[0]);
    let width = len(top).max(f32::EPSILON);
    let center = [(q[0][0] + q[2][0]) / 2.0, (q[0][1] + q[2][1]) / 2.0];
    Rect { center, u: [top[0] / width, top[1] / width], half_w: width / 2.0, half_h: len(side) / 2.0 }
}

/// The smallest rectangle around some points, oriented as `Rect` says. `None` for no points.
pub fn min_area_rect(points: Vec<Point>) -> Option<Rect> {
    let hull = hull(points);
    let first = *hull.first()?;
    if hull.len() < 3 {
        let last = *hull.last()?;
        let (x0, y0, x1, y1) = (first[0].min(last[0]), first[1].min(last[1]), first[0].max(last[0]), first[1].max(last[1]));
        return Some(Rect { center: [(x0 + x1) / 2.0, (y0 + y1) / 2.0], u: [1.0, 0.0], half_w: (x1 - x0) / 2.0, half_h: (y1 - y0) / 2.0 });
    }
    let mut best: Option<(f32, Rect)> = None;
    for i in 0..hull.len() {
        let edge = sub(hull[(i + 1) % hull.len()], hull[i]);
        let l = len(edge);
        if l == 0.0 {
            continue;
        }
        // The edge's direction or its perpendicular, whichever is closer to horizontal, pointing right.
        let e = [edge[0] / l, edge[1] / l];
        let mut u = if e[0].abs() >= e[1].abs() { e } else { [-e[1], e[0]] };
        if u[0] < 0.0 || (u[0] == 0.0 && u[1] < 0.0) {
            u = [-u[0], -u[1]];
        }
        let v = [-u[1], u[0]];
        let (mut a0, mut a1, mut b0, mut b1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &p in &hull {
            let (a, b) = (dot(p, u), dot(p, v));
            a0 = a0.min(a);
            a1 = a1.max(a);
            b0 = b0.min(b);
            b1 = b1.max(b);
        }
        let area = (a1 - a0) * (b1 - b0);
        if best.is_none_or(|(old, _)| area < old - 1e-3) {
            let (a, b) = ((a0 + a1) / 2.0, (b0 + b1) / 2.0);
            let center = [u[0] * a + v[0] * b, u[1] * a + v[1] * b];
            best = Some((area, Rect { center, u, half_w: (a1 - a0) / 2.0, half_h: (b1 - b0) / 2.0 }));
        }
    }
    best.map(|(_, r)| r)
}

/// The axis-aligned box around a quad.
pub fn bounds(q: &Quad) -> (f32, f32, f32, f32) {
    let xs = q.iter().map(|p| p[0]);
    let ys = q.iter().map(|p| p[1]);
    (xs.clone().fold(f32::MAX, f32::min), ys.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max))
}

/// The size of the straightened crop of a quad, as PaddleOCR's `get_rotate_crop_image` (and
/// Lookupper's port of it) takes it: the longer of each pair of opposite sides.
pub fn crop_size(q: &Quad) -> (f32, f32) {
    let w = len(sub(q[1], q[0])).max(len(sub(q[2], q[3])));
    let h = len(sub(q[3], q[0])).max(len(sub(q[2], q[1])));
    (w, h)
}

/// The quad's area cut out of `img` and straightened to `w` x `h`, its top left corner at the
/// crop's top left. Bilinear, with the image's edge pixels repeated past its border.
pub fn warp(img: &RgbaImage, q: &Quad, w: u32, h: u32) -> RgbaImage {
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    let pixel = |x: i64, y: i64| img.get_pixel(x.clamp(0, iw - 1) as u32, y.clamp(0, ih - 1) as u32).0;
    let (top, side) = (sub(q[1], q[0]), sub(q[3], q[0]));
    RgbaImage::from_fn(w, h, |x, y| {
        let (s, t) = (x as f32 / w as f32, y as f32 / h as f32);
        let sx = q[0][0] + top[0] * s + side[0] * t;
        let sy = q[0][1] + top[1] * s + side[1] * t;
        let (fx, fy) = (sx.floor(), sy.floor());
        let (ax, ay) = (sx - fx, sy - fy);
        let (x0, y0) = (fx as i64, fy as i64);
        let (p00, p10, p01, p11) = (pixel(x0, y0), pixel(x0 + 1, y0), pixel(x0, y0 + 1), pixel(x0 + 1, y0 + 1));
        let mut out = [0u8; 4];
        for c in 0..4 {
            let top = p00[c] as f32 * (1.0 - ax) + p10[c] as f32 * ax;
            let bottom = p01[c] as f32 * (1.0 - ax) + p11[c] as f32 * ax;
            out[c] = (top * (1.0 - ay) + bottom * ay).round().clamp(0.0, 255.0) as u8;
        }
        Rgba(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// The corners of the pixels at `cells`, as the detector gives a text kernel.
    fn corners_of(cells: &[(i32, i32)]) -> Vec<Point> {
        cells.iter().flat_map(|&(x, y)| {
            let (x, y) = (x as f32, y as f32);
            [[x, y], [x + 1.0, y], [x, y + 1.0], [x + 1.0, y + 1.0]]
        }).collect()
    }

    #[test]
    fn an_upright_kernel_gives_its_box() {
        let cells: Vec<(i32, i32)> = (2..12).flat_map(|x| (5..8).map(move |y| (x, y))).collect();
        let r = min_area_rect(corners_of(&cells)).unwrap();
        assert!(!r.rotated());
        assert_eq!(bounds(&r.corners()), (2.0, 5.0, 12.0, 8.0));
        // A column too: its top edge is still the horizontal one.
        let column: Vec<(i32, i32)> = (0..3).flat_map(|x| (0..10).map(move |y| (x, y))).collect();
        let r = min_area_rect(corners_of(&column)).unwrap();
        assert!(!r.rotated() && close(r.half_w, 1.5) && close(r.half_h, 5.0));
    }

    #[test]
    fn a_tilted_kernel_gives_a_turned_rectangle_with_its_top_edge_on_top() {
        // A 20 x 4 bar rising 1 in 2 to the right.
        let cells: Vec<(i32, i32)> = (0..20).flat_map(|x| (0..4).map(move |y| (x, y - x / 2))).collect();
        let r = min_area_rect(corners_of(&cells)).unwrap();
        assert!(r.rotated());
        assert!(r.u[0] > 0.8 && r.u[1] < -0.3, "{:?}", r.u);
        let q = r.corners();
        assert!(q[0][1] < q[3][1] && q[0][0] < q[1][0], "top left above bottom left, left of top right: {q:?}");
        let (w, h) = crop_size(&q);
        assert!(w > 3.0 * h, "the long side is the width: {w} x {h}");
    }

    #[test]
    fn a_quad_round_trips_in_its_own_frame() {
        let r = Rect { center: [50.0, 40.0], u: [0.8, 0.6], half_w: 30.0, half_h: 5.0 };
        let back = rect_of(&r.corners());
        assert!(close(back.half_w, 30.0) && close(back.half_h, 5.0) && close(back.u[0], 0.8) && close(back.center[1], 40.0));
        assert!(close(r.local([50.0 + 0.8 * 30.0, 40.0 + 0.6 * 30.0]).0, 30.0));
    }

    #[test]
    fn warping_an_upright_quad_copies_its_pixels() {
        let img = RgbaImage::from_fn(10, 8, |x, y| Rgba([(x * 20) as u8, (y * 30) as u8, 0, 255]));
        let q = [[2.0, 1.0], [6.0, 1.0], [6.0, 4.0], [2.0, 4.0]];
        let out = warp(&img, &q, 4, 3);
        assert_eq!(out.dimensions(), (4, 3));
        assert_eq!(out.get_pixel(0, 0), img.get_pixel(2, 1));
        assert_eq!(out.get_pixel(3, 2), img.get_pixel(5, 3));
    }
}
