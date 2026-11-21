//! Tiny CPU raster toolkit for the Dusty rig: premultiplied RGBA layers, affine draws with
//! bilinear sampling, anti-aliased ellipses / strokes / polygons, masks and blur.

pub type Px = [f32; 4]; // premultiplied r, g, b, a in 0..1

#[derive(Clone, Copy, Debug)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Affine {
    pub const ID: Affine = Affine { a: 1., b: 0., c: 0., d: 1., e: 0., f: 0. };
    pub fn translate(x: f32, y: f32) -> Self {
        Affine { e: x, f: y, ..Self::ID }
    }
    pub fn scale(sx: f32, sy: f32) -> Self {
        Affine { a: sx, d: sy, ..Self::ID }
    }
    pub fn rotate(rad: f32) -> Self {
        let (s, c) = rad.sin_cos();
        Affine { a: c, b: s, c: -s, d: c, e: 0., f: 0. }
    }
    /// self ∘ other: apply `other` first, then `self` (like canvas ctx.transform order)
    pub fn then(self, m: Affine) -> Affine {
        // returns self * m  (point p → self(m(p)))
        Affine {
            a: self.a * m.a + self.c * m.b,
            b: self.b * m.a + self.d * m.b,
            c: self.a * m.c + self.c * m.d,
            d: self.b * m.c + self.d * m.d,
            e: self.a * m.e + self.c * m.f + self.e,
            f: self.b * m.e + self.d * m.f + self.f,
        }
    }
    /// canvas-style "rotate about a point"
    pub fn rotate_about(self, (px, py): (f32, f32), rad: f32) -> Affine {
        self.then(Affine::translate(px, py)).then(Affine::rotate(rad)).then(Affine::translate(-px, -py))
    }
    pub fn scale_about(self, (px, py): (f32, f32), sx: f32, sy: f32) -> Affine {
        self.then(Affine::translate(px, py)).then(Affine::scale(sx, sy)).then(Affine::translate(-px, -py))
    }
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }
    pub fn inverse(&self) -> Affine {
        let det = self.a * self.d - self.b * self.c;
        let id = if det.abs() < 1e-9 { 0. } else { 1. / det };
        Affine {
            a: self.d * id,
            b: -self.b * id,
            c: -self.c * id,
            d: self.a * id,
            e: (self.c * self.f - self.d * self.e) * id,
            f: (self.b * self.e - self.a * self.f) * id,
        }
    }
    pub fn scale_factor(&self) -> f32 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }
}

/// A premultiplied image placed at (x0, y0) in rig coordinates.
#[derive(Clone)]
pub struct Layer {
    pub x0: i32,
    pub y0: i32,
    pub w: usize,
    pub h: usize,
    pub px: Vec<Px>,
}

impl Layer {
    pub fn empty(w: usize, h: usize) -> Self {
        Layer { x0: 0, y0: 0, w, h, px: vec![[0.; 4]; w * h] }
    }
    /// From straight-alpha RGBA8 multiplied by a coverage mask.
    pub fn from_rgba_masked(src: &[[u8; 4]], w: usize, h: usize, mask: &[f32]) -> Self {
        let mut l = Layer::empty(w, h);
        for i in 0..w * h {
            let a = src[i][3] as f32 / 255. * mask[i];
            if a > 0. {
                l.px[i] = [src[i][0] as f32 / 255. * a, src[i][1] as f32 / 255. * a, src[i][2] as f32 / 255. * a, a];
            }
        }
        l.trim()
    }
    /// Crop to the bounding box of non-transparent pixels (fewer pixels to push every frame).
    pub fn trim(self) -> Self {
        let (mut x0, mut y0, mut x1, mut y1) = (self.w, self.h, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.px[y * self.w + x][3] > 0.002 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 <= x0 {
            return Layer { x0: 0, y0: 0, w: 1, h: 1, px: vec![[0.; 4]] };
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let mut px = Vec::with_capacity(w * h);
        for y in y0..y1 {
            px.extend_from_slice(&self.px[y * self.w + x0..y * self.w + x1]);
        }
        Layer { x0: self.x0 + x0 as i32, y0: self.y0 + y0 as i32, w, h, px }
    }
    /// src over self (same size, same origin)
    pub fn over(&mut self, top: &Layer) {
        for y in 0..top.h {
            for x in 0..top.w {
                let (tx, ty) = (x as i32 + top.x0 - self.x0, y as i32 + top.y0 - self.y0);
                if tx < 0 || ty < 0 || tx >= self.w as i32 || ty >= self.h as i32 {
                    continue;
                }
                let s = top.px[y * top.w + x];
                let d = &mut self.px[ty as usize * self.w + tx as usize];
                let k = 1. - s[3];
                for c in 0..4 {
                    d[c] = s[c] + d[c] * k;
                }
            }
        }
    }
    #[inline]
    fn sample(&self, x: f32, y: f32) -> Px {
        // bilinear, pixel centres at +0.5, transparent outside
        let fx = x - self.x0 as f32 - 0.5;
        let fy = y - self.y0 as f32 - 0.5;
        let ix = fx.floor();
        let iy = fy.floor();
        let (tx, ty) = (fx - ix, fy - iy);
        let (ix, iy) = (ix as i32, iy as i32);
        let get = |x: i32, y: i32| -> Px {
            if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
                [0.; 4]
            } else {
                self.px[y as usize * self.w + x as usize]
            }
        };
        let (p00, p10, p01, p11) = (get(ix, iy), get(ix + 1, iy), get(ix, iy + 1), get(ix + 1, iy + 1));
        let mut o = [0.; 4];
        for c in 0..4 {
            let top = p00[c] + (p10[c] - p00[c]) * tx;
            let bot = p01[c] + (p11[c] - p01[c]) * tx;
            o[c] = top + (bot - top) * ty;
        }
        o
    }
}

/// The frame being composed.
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Px>,
}

#[inline]
fn blend(d: &mut Px, s: Px) {
    let k = 1. - s[3];
    d[0] = s[0] + d[0] * k;
    d[1] = s[1] + d[1] * k;
    d[2] = s[2] + d[2] * k;
    d[3] = s[3] + d[3] * k;
}

pub fn rgba(hex: u32, a: f32) -> Px {
    let r = ((hex >> 16) & 255) as f32 / 255.;
    let g = ((hex >> 8) & 255) as f32 / 255.;
    let b = (hex & 255) as f32 / 255.;
    [r * a, g * a, b * a, a]
}
fn scaled(c: Px, k: f32) -> Px {
    [c[0] * k, c[1] * k, c[2] * k, c[3] * k]
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Self {
        Canvas { w, h, px: vec![[0.; 4]; w * h] }
    }
    pub fn clear(&mut self) {
        self.px.iter_mut().for_each(|p| *p = [0.; 4]);
    }

    fn bbox(&self, pts: &[(f32, f32)], pad: f32) -> Option<(usize, usize, usize, usize)> {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in pts {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let x0 = (x0 - pad).floor().max(0.) as usize;
        let y0 = (y0 - pad).floor().max(0.) as usize;
        let x1 = ((x1 + pad).ceil().max(0.) as usize).min(self.w);
        let y1 = ((y1 + pad).ceil().max(0.) as usize).min(self.h);
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
    }

    /// Draw a layer through `m` (rig coords → canvas px). `clip` sees rig coords of the source pixel.
    pub fn draw_layer(&mut self, l: &Layer, m: Affine, alpha: f32, clip: Option<&dyn Fn(f32, f32) -> bool>) {
        self.draw_layer_in(l, m, alpha, clip, None)
    }

    /// Like `draw_layer`, but only visits the canvas pixels covered by `region` (a quad in layer
    /// coordinates). Bending ears draw ~30 narrow bands; without this each band scans the whole ear.
    pub fn draw_layer_in(&mut self, l: &Layer, m: Affine, alpha: f32, clip: Option<&dyn Fn(f32, f32) -> bool>, region: Option<[(f32, f32); 4]>) {
        let (x0, y0) = (l.x0 as f32, l.y0 as f32);
        let (x1, y1) = (x0 + l.w as f32, y0 + l.h as f32);
        let quad = region.unwrap_or([(x0, y0), (x1, y0), (x0, y1), (x1, y1)]);
        let corners = quad.map(|(x, y)| m.apply(x, y));
        let Some((bx0, by0, bx1, by1)) = self.bbox(&corners, 1.) else { return };
        let inv = m.inverse();
        for py in by0..by1 {
            let cy = py as f32 + 0.5;
            let (mut sx, mut sy) = inv.apply(bx0 as f32 + 0.5, cy);
            for px in bx0..bx1 {
                if sx >= x0 - 1. && sy >= y0 - 1. && sx <= x1 + 1. && sy <= y1 + 1. && clip.is_none_or(|f| f(sx, sy)) {
                    let s = l.sample(sx, sy);
                    if s[3] > 0.0005 {
                        blend(&mut self.px[py * self.w + px], scaled(s, alpha));
                    }
                }
                sx += inv.a;
                sy += inv.b;
            }
        }
    }

    /// Filled ellipse in local coords through `m`; `clip_ell` optionally clips to another local ellipse.
    pub fn fill_ellipse(&mut self, m: Affine, c: (f32, f32), r: (f32, f32), color: Px, clip_ell: Option<((f32, f32), (f32, f32))>) {
        let pts: Vec<(f32, f32)> = [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)].iter().map(|(dx, dy)| m.apply(c.0 + dx * r.0, c.1 + dy * r.1)).collect();
        let Some((bx0, by0, bx1, by1)) = self.bbox(&pts, 2.) else { return };
        let inv = m.inverse();
        let px_per_unit = m.scale_factor();
        for py in by0..by1 {
            for px in bx0..bx1 {
                let (lx, ly) = inv.apply(px as f32 + 0.5, py as f32 + 0.5);
                let cov = ellipse_cov(lx, ly, c, r, px_per_unit);
                if cov <= 0. {
                    continue;
                }
                let cov = match clip_ell {
                    Some((cc, rr)) => cov * ellipse_cov(lx, ly, cc, rr, px_per_unit),
                    None => cov,
                };
                if cov > 0. {
                    blend(&mut self.px[py * self.w + px], scaled(color, cov));
                }
            }
        }
    }

    /// Soft radial blob (blush, shadows): alpha falls off from centre to rim.
    pub fn fill_radial(&mut self, m: Affine, c: (f32, f32), r: (f32, f32), color: Px) {
        let pts: Vec<(f32, f32)> = [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)].iter().map(|(dx, dy)| m.apply(c.0 + dx * r.0, c.1 + dy * r.1)).collect();
        let Some((bx0, by0, bx1, by1)) = self.bbox(&pts, 1.) else { return };
        let inv = m.inverse();
        for py in by0..by1 {
            for px in bx0..bx1 {
                let (lx, ly) = inv.apply(px as f32 + 0.5, py as f32 + 0.5);
                let d = (((lx - c.0) / r.0).powi(2) + ((ly - c.1) / r.1).powi(2)).sqrt();
                if d < 1. {
                    let k = (1. - d).powf(1.3);
                    blend(&mut self.px[py * self.w + px], scaled(color, k));
                }
            }
        }
    }

    /// Round-capped polyline stroke (points in local coords).
    pub fn stroke(&mut self, m: Affine, pts: &[(f32, f32)], width: f32, color: Px) {
        if pts.len() < 2 {
            return;
        }
        let tp: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| m.apply(x, y)).collect();
        let hw = width * m.scale_factor() / 2.;
        let Some((bx0, by0, bx1, by1)) = self.bbox(&tp, hw + 2.) else { return };
        for py in by0..by1 {
            for px in bx0..bx1 {
                let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
                let mut dmin = f32::MAX;
                for s in tp.windows(2) {
                    dmin = dmin.min(seg_dist(x, y, s[0], s[1]));
                }
                let cov = (hw - dmin + 0.5).clamp(0., 1.);
                if cov > 0. {
                    blend(&mut self.px[py * self.w + px], scaled(color, cov));
                }
            }
        }
    }

    /// Filled polygon (even-odd), 4×4 supersampled.
    pub fn fill_poly(&mut self, m: Affine, pts: &[(f32, f32)], color: Px) {
        let tp: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| m.apply(x, y)).collect();
        let Some((bx0, by0, bx1, by1)) = self.bbox(&tp, 1.) else { return };
        for py in by0..by1 {
            for px in bx0..bx1 {
                let mut hits = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if point_in_poly(px as f32 + (sx as f32 + 0.5) / 4., py as f32 + (sy as f32 + 0.5) / 4., &tp) {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    blend(&mut self.px[py * self.w + px], scaled(color, hits as f32 / 16.));
                }
            }
        }
    }

    /// Straight-alpha BGRA8, which is what GPUI's image sprites expect.
    pub fn to_bgra(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.w * self.h * 4);
        for p in &self.px {
            let a = p[3].clamp(0., 1.);
            if a < 1e-4 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let un = |v: f32| ((v / a).clamp(0., 1.) * 255. + 0.5) as u8;
                out.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), (a * 255. + 0.5) as u8]);
            }
        }
        out
    }
}

fn ellipse_cov(x: f32, y: f32, c: (f32, f32), r: (f32, f32), px_per_unit: f32) -> f32 {
    let nx = (x - c.0) / r.0;
    let ny = (y - c.1) / r.1;
    let d = (nx * nx + ny * ny).sqrt();
    // distance to rim in pixels ≈ (1 - d) * min radius * scale
    ((1. - d) * r.0.min(r.1) * px_per_unit + 0.5).clamp(0., 1.)
}

fn seg_dist(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-6 { 0. } else { (((x - a.0) * dx + (y - a.1) * dy) / len2).clamp(0., 1.) };
    let (px, py) = (a.0 + dx * t, a.1 + dy * t);
    ((x - px).powi(2) + (y - py).powi(2)).sqrt()
}

pub fn point_in_poly(x: f32, y: f32, p: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = p.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = p[i];
        let (xj, yj) = p[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

// ───────────── masks (rig build time) ─────────────

/// Coverage mask of a polygon, 4×4 supersampled.
pub fn poly_mask(w: usize, h: usize, pts: &[(f32, f32)]) -> Vec<f32> {
    let mut m = vec![0.; w * h];
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let xa = x0.floor().max(0.) as usize;
    let ya = y0.floor().max(0.) as usize;
    let xb = (x1.ceil().max(0.) as usize + 1).min(w);
    let yb = (y1.ceil().max(0.) as usize + 1).min(h);
    for y in ya..yb {
        for x in xa..xb {
            let mut hits = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    if point_in_poly(x as f32 + (sx as f32 + 0.5) / 4., y as f32 + (sy as f32 + 0.5) / 4., pts) {
                        hits += 1;
                    }
                }
            }
            m[y * w + x] = hits as f32 / 16.;
        }
    }
    m
}

pub fn ellipse_mask(w: usize, h: usize, c: (f32, f32), r: (f32, f32)) -> Vec<f32> {
    let mut m = vec![0.; w * h];
    for y in 0..h {
        for x in 0..w {
            m[y * w + x] = ellipse_cov(x as f32 + 0.5, y as f32 + 0.5, c, r, 1.);
        }
    }
    m
}

/// Approximate Gaussian blur: three box-blur passes.
pub fn blur(m: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let mut a = m.to_vec();
    let mut b = vec![0.; w * h];
    for _ in 0..3 {
        for y in 0..h {
            for x in 0..w {
                let (lo, hi) = (x.saturating_sub(radius), (x + radius).min(w - 1));
                let s: f32 = a[y * w + lo..=y * w + hi].iter().sum();
                b[y * w + x] = s / (hi - lo + 1) as f32;
            }
        }
        for y in 0..h {
            for x in 0..w {
                let (lo, hi) = (y.saturating_sub(radius), (y + radius).min(h - 1));
                let mut s = 0.;
                for yy in lo..=hi {
                    s += b[yy * w + x];
                }
                a[y * w + x] = s / (hi - lo + 1) as f32;
            }
        }
    }
    a
}

/// Quadratic curve through midpoints (same as the HTML prototype's `_smooth`).
pub fn smooth(pts: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let mut out = vec![pts[0]];
    for i in 1..pts.len() - 1 {
        let start = *out.last().unwrap();
        let ctrl = pts[i];
        let end = ((pts[i].0 + pts[i + 1].0) / 2., (pts[i].1 + pts[i + 1].1) / 2.);
        for s in 1..=8 {
            let t = s as f32 / 8.;
            let u = 1. - t;
            out.push((u * u * start.0 + 2. * u * t * ctrl.0 + t * t * end.0, u * u * start.1 + 2. * u * t * ctrl.1 + t * t * end.1));
        }
    }
    out.push(*pts.last().unwrap());
    out
}

/// Sampled quadratic Bézier.
pub fn quad(p0: (f32, f32), c: (f32, f32), p1: (f32, f32)) -> Vec<(f32, f32)> {
    (0..=16)
        .map(|i| {
            let t = i as f32 / 16.;
            let u = 1. - t;
            (u * u * p0.0 + 2. * u * t * c.0 + t * t * p1.0, u * u * p0.1 + 2. * u * t * c.1 + t * t * p1.1)
        })
        .collect()
}

// ───────────── backend-neutral drawing interface ─────────────

/// One band of a bending ear: only source pixels whose distance along `u` from `p` is in [u0, u1).
#[derive(Clone, Copy)]
pub struct Band {
    pub p: (f32, f32),
    pub u: (f32, f32),
    pub u0: f32,
    pub u1: f32,
}

/// What Dusty's renderer draws with. `Canvas` (CPU) and the Metal recorder (GPU) both implement it,
/// so both backends draw exactly the same picture from one description.
pub trait Painter {
    fn side(&self) -> usize;
    fn begin(&mut self);
    fn draw_layer(&mut self, l: &Layer, m: Affine, alpha: f32, clip: Option<&dyn Fn(f32, f32) -> bool>);
    fn draw_band(&mut self, l: &Layer, m: Affine, band: Band, region: [(f32, f32); 4]);
    fn fill_ellipse(&mut self, m: Affine, c: (f32, f32), r: (f32, f32), color: Px, clip_ell: Option<((f32, f32), (f32, f32))>);
    fn fill_radial(&mut self, m: Affine, c: (f32, f32), r: (f32, f32), color: Px);
    fn stroke(&mut self, m: Affine, pts: &[(f32, f32)], width: f32, color: Px);
    fn fill_poly(&mut self, m: Affine, pts: &[(f32, f32)], color: Px);
}

impl Painter for Canvas {
    fn side(&self) -> usize {
        self.w
    }
    fn begin(&mut self) {
        self.clear();
    }
    fn draw_layer(&mut self, l: &Layer, m: Affine, alpha: f32, clip: Option<&dyn Fn(f32, f32) -> bool>) {
        Canvas::draw_layer(self, l, m, alpha, clip)
    }
    fn draw_band(&mut self, l: &Layer, m: Affine, b: Band, region: [(f32, f32); 4]) {
        let clip = move |sx: f32, sy: f32| {
            let along = (sx - b.p.0) * b.u.0 + (sy - b.p.1) * b.u.1;
            along >= b.u0 && along < b.u1
        };
        self.draw_layer_in(l, m, 1., Some(&clip), Some(region))
    }
    fn fill_ellipse(&mut self, m: Affine, c: (f32, f32), r: (f32, f32), color: Px, clip_ell: Option<((f32, f32), (f32, f32))>) {
        Canvas::fill_ellipse(self, m, c, r, color, clip_ell)
    }
    fn fill_radial(&mut self, m: Affine, c: (f32, f32), r: (f32, f32), color: Px) {
        Canvas::fill_radial(self, m, c, r, color)
    }
    fn stroke(&mut self, m: Affine, pts: &[(f32, f32)], width: f32, color: Px) {
        Canvas::stroke(self, m, pts, width, color)
    }
    fn fill_poly(&mut self, m: Affine, pts: &[(f32, f32)], color: Px) {
        Canvas::fill_poly(self, m, pts, color)
    }
}
