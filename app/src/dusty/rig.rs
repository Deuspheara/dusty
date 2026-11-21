//! Cuts the character-sheet art into rig layers (port of the HTML prototype's mascot.js, whose cut
//! lines were validated at 0 holes / 0.3% difference from the reference at rest).

use super::raster::*;

/// The front-facing cat in sheet coordinates.
pub const CROP: (f32, f32, usize, usize) = (95., 0., 460, 460);

pub fn p(x: f32, y: f32) -> (f32, f32) {
    (x - CROP.0, y - CROP.1)
}
fn pts(v: &[(f32, f32)]) -> Vec<(f32, f32)> {
    v.iter().map(|&(x, y)| p(x, y)).collect()
}

pub const CHIN: &[(f32, f32)] = &[
    (118., 240.), (124., 252.), (130., 266.), (135., 277.), (150., 291.), (175., 303.), (200., 311.), (225., 314.5), (250., 316.),
    (280., 316.), (320., 315.), (347., 311.), (385., 302.), (410., 293.), (430., 269.), (442., 252.), (446., 227.),
];
const HEAD_TOP: &[(f32, f32)] = &[
    (90., 196.), (118., 182.), (140., 178.), (153., 170.), (231., 106.), (236., 89.), (270., 76.), (300., 72.), (330., 78.), (350., 87.),
    (356., 104.), (383., 122.), (426., 168.), (440., 186.), (478., 192.), (480., 262.), (452., 266.),
];
const DOME: &[(f32, f32)] = &[(132., 182.), (148., 165.), (187., 133.), (226., 101.), (260., 88.), (300., 81.), (340., 92.), (362., 103.), (386., 117.), (430., 163.), (444., 184.)];
const EAR_L: &[(f32, f32)] = &[(90., 186.), (96., 100.), (122., 14.), (176., 2.), (224., 22.), (270., 50.), (276., 74.), (258., 84.), (240., 92.), (229., 101.), (232., 109.), (196., 143.), (158., 176.), (128., 186.)];
const EAR_L_SOFT: &[(f32, f32)] = &[(70., 200.), (80., 96.), (110., -10.), (180., -20.), (250., 10.), (290., 50.), (290., 84.), (250., 100.), (232., 109.), (196., 143.), (158., 176.), (128., 186.), (100., 196.)];
const EAR_R: &[(f32, f32)] = &[(350., 106.), (340., 94.), (334., 82.), (344., 62.), (380., 44.), (420., 30.), (462., 26.), (484., 44.), (486., 188.), (438., 192.), (423., 176.), (380., 128.)];
const EAR_R_SOFT: &[(f32, f32)] = &[(330., 112.), (320., 80.), (340., 48.), (380., 24.), (420., 10.), (474., 8.), (500., 40.), (502., 204.), (438., 192.), (423., 176.), (380., 128.), (350., 106.)];
const TAIL: &[(f32, f32)] = &[(360., 460.), (360., 352.), (398., 346.), (406., 340.), (400., 310.), (410., 298.), (430., 274.), (442., 257.), (452., 266.), (480., 262.), (480., 248.), (556., 248.), (556., 460.)];
const BODY_RIGHT: &[(f32, f32)] = &[(405., 279.), (412., 300.), (414., 335.), (410., 380.), (410., 430.), (402., 460.)];
const KNOT: &[(f32, f32)] = &[(344., 300.), (360., 296.), (380., 292.), (402., 287.), (415., 298.), (407., 318.), (415., 347.), (397., 349.), (376., 331.), (360., 331.), (344., 326.)];

pub const EYES: [((f32, f32), (f32, f32)); 2] = [((217., 217.), (31., 35.)), ((352., 217.), (30., 35.))];
pub const MOUTH: ((f32, f32), (f32, f32)) = ((284., 247.), (31., 12.));
pub const NECK: (f32, f32) = (280., 318.);
pub const GROUND: (f32, f32) = (278., 443.);
pub const EAR_L_PIVOT: (f32, f32) = (187., 136.);
pub const EAR_L_TIP: (f32, f32) = (163., 38.);
pub const EAR_R_PIVOT: (f32, f32) = (395., 132.);
pub const EAR_R_TIP: (f32, f32) = (434., 56.);
pub const TAIL_PIVOT: (f32, f32) = (408., 330.);

pub struct Rig {
    pub w: usize,
    pub h: usize,
    pub tail: Layer,
    pub body: Layer,
    pub ear_l: Layer,
    pub ear_r: Layer,
    pub head: Layer,
    pub eyes: [Layer; 2],
    pub mouth: Layer,
    pub knot: Layer,
    pub line: u32,
}

pub fn chin_y(x: f32, dy: f32) -> f32 {
    for i in 1..CHIN.len() {
        if x <= CHIN[i].0 {
            let (x0, y0) = CHIN[i - 1];
            let (x1, y1) = CHIN[i];
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0) + dy;
        }
    }
    CHIN[CHIN.len() - 1].1 + dy
}

pub fn build(png: &[u8]) -> anyhow::Result<Rig> {
    let sheet = image::load_from_memory(png)?.to_rgba8();
    let (w, h) = (CROP.2, CROP.3);
    let mut base: Vec<[u8; 4]> = vec![[0; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x as u32 + CROP.0 as u32, y as u32 + CROP.1 as u32);
            if sx < sheet.width() && sy < sheet.height() {
                base[y * w + x] = sheet.get_pixel(sx, sy).0;
            }
        }
    }
    remove_background(&mut base, w, h);
    let sample = |img: &[[u8; 4]], (x, y): (f32, f32)| -> u32 {
        let (cx, cy) = p(x, y);
        let px = img[cy as usize * w + cx as usize];
        ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32
    };
    let line = sample(&base, (280., 316.));
    let fur = sample(&base, (320., 112.));

    // clean: eye sockets and mouth filled in from their surroundings
    let mut clean = base.clone();
    for (c, r) in EYES {
        inpaint_ellipse(&mut clean, w, h, p(c.0, c.1), (r.0 + 2., r.1 + 2.));
    }
    inpaint_ellipse(&mut clean, w, h, p(MOUTH.0 .0, MOUTH.0 .1), (MOUTH.1 .0 + 2., MOUTH.1 .1 + 2.));

    // body source: clean + the neck strip under the chin smeared up from the bandana/fur
    let mut neck = clean.clone();
    for sx in 140..=404 {
        let x = (sx as f32 - CROP.0) as usize;
        let src = chin_y(sx as f32, 7.).round() as usize;
        if x >= w || src >= h {
            continue;
        }
        let s = neck[src * w + x];
        let (ya, yb) = (chin_y(sx as f32, -16.).round().max(0.) as usize, chin_y(sx as f32, 3.).round() as usize);
        for y in ya..=yb.min(h - 1) {
            neck[y * w + x] = s;
        }
    }

    let head_poly: Vec<(f32, f32)> = HEAD_TOP
        .iter()
        .copied()
        .chain(CHIN.iter().filter(|(x, _)| *x >= 150. && *x <= 440.).rev().map(|&(x, y)| (x, y + 5.)))
        .chain([(128., 294.), (108., 282.), (82., 282.)])
        .collect();
    let body_poly: Vec<(f32, f32)> = std::iter::once((150., chin_y(150., -16.)))
        .chain(CHIN.iter().filter(|(x, _)| *x > 150. && *x < 405.).map(|&(x, y)| (x, y - 16.)))
        .chain(BODY_RIGHT.iter().copied())
        .chain([(130., 460.), (120., 300.)])
        .collect();

    let masked = |img: &[[u8; 4]], m: &[f32]| Layer::from_rgba_masked(img, w, h, m);
    let feathered = |hard: &[(f32, f32)], soft: &[(f32, f32)]| {
        let s = blur(&poly_mask(w, h, &pts(soft)), w, h, 2);
        let hm = poly_mask(w, h, &pts(hard));
        let m: Vec<f32> = s.iter().zip(&hm).map(|(a, b)| a * b).collect();
        masked(&base, &m)
    };

    // head = a complete rounded skull (fur + outline) under the ears, then the real head pixels
    let mut head_canvas = Canvas::new(w, h);
    let dome = smooth(&pts(DOME));
    let mut dome_fill = dome.clone();
    dome_fill.push(p(444., 240.));
    dome_fill.push(p(132., 240.));
    head_canvas.fill_poly(Affine::ID, &dome_fill, rgba(fur, 1.));
    head_canvas.stroke(Affine::ID, &dome, 6., rgba(line, 1.));
    let mut head = Layer { x0: 0, y0: 0, w, h, px: head_canvas.px };
    head.over(&Layer { x0: 0, y0: 0, ..masked_full(&clean, w, h, &poly_mask(w, h, &pts(&head_poly))) });

    let eyes = EYES.map(|(c, r)| masked(&base, &ellipse_mask(w, h, p(c.0, c.1), (r.0 + 2., r.1 + 2.))));
    Ok(Rig {
        w,
        h,
        tail: masked(&base, &poly_mask(w, h, &pts(TAIL))),
        body: masked(&neck, &poly_mask(w, h, &pts(&body_poly))),
        ear_l: feathered(EAR_L, EAR_L_SOFT),
        ear_r: feathered(EAR_R, EAR_R_SOFT),
        head: head.trim(),
        eyes,
        mouth: masked(&base, &ellipse_mask(w, h, p(MOUTH.0 .0, MOUTH.0 .1), (MOUTH.1 .0 + 2., MOUTH.1 .1 + 2.))),
        knot: masked(&base, &poly_mask(w, h, &pts(KNOT))),
        line,
    })
}

/// Untrimmed masked layer (same size as the rig), for compositing at build time.
fn masked_full(img: &[[u8; 4]], w: usize, h: usize, m: &[f32]) -> Layer {
    let mut l = Layer::empty(w, h);
    for i in 0..w * h {
        let a = img[i][3] as f32 / 255. * m[i];
        l.px[i] = [img[i][0] as f32 / 255. * a, img[i][1] as f32 / 255. * a, img[i][2] as f32 / 255. * a, a];
    }
    l
}

/// Flood-fill the paper (white or transparent) from the border; soften the anti-aliased rim.
fn remove_background(d: &mut [[u8; 4]], w: usize, h: usize) {
    let paper = |p: [u8; 4]| p[3] < 16 || (p[0] > 228 && p[1] > 228 && p[2] > 228);
    let mut bg = vec![false; w * h];
    let mut stack: Vec<usize> = vec![];
    for x in 0..w {
        stack.push(x);
        stack.push((h - 1) * w + x);
    }
    for y in 0..h {
        stack.push(y * w);
        stack.push(y * w + w - 1);
    }
    while let Some(i) = stack.pop() {
        if bg[i] || !paper(d[i]) {
            continue;
        }
        bg[i] = true;
        let (x, y) = (i % w, i / w);
        if x > 0 { stack.push(i - 1); }
        if x + 1 < w { stack.push(i + 1); }
        if y > 0 { stack.push(i - w); }
        if y + 1 < h { stack.push(i + w); }
    }
    let mut near = vec![false; w * h];
    for i in 0..w * h {
        if !bg[i] {
            continue;
        }
        let (x, y) = ((i % w) as i32, (i / w) as i32);
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (xx, yy) = (x + dx, y + dy);
                if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                    near[yy as usize * w + xx as usize] = true;
                }
            }
        }
    }
    for i in 0..w * h {
        if bg[i] {
            d[i] = [0, 0, 0, 0];
            continue;
        }
        if !near[i] {
            continue;
        }
        let px = d[i];
        let l = (px[0] as f32 + px[1] as f32 + px[2] as f32) / 3.;
        if l < 120. {
            continue;
        }
        let t = ((255. - l) / (255. - 92.)).clamp(0., 1.);
        let un = |c: u8| (((c as f32 - (1. - t) * 255.) / t.max(0.05)).clamp(0., 255.)) as u8;
        d[i] = [un(px[0]), un(px[1]), un(px[2]), (t * px[3] as f32) as u8];
    }
}

/// Fill an ellipse from its surroundings: inverse-distance blend of the nearest outside pixel in 4 directions.
fn inpaint_ellipse(d: &mut [[u8; 4]], w: usize, h: usize, (cx, cy): (f32, f32), (rx, ry): (f32, f32)) {
    let inside = |x: i32, y: i32| ((x as f32 - cx) / rx).powi(2) + ((y as f32 - cy) / ry).powi(2) <= 1.;
    let src = d.to_vec();
    for y in (cy - ry).floor() as i32..=(cy + ry).ceil() as i32 {
        for x in (cx - rx).floor() as i32..=(cx + rx).ceil() as i32 {
            if !inside(x, y) || x < 0 || y < 0 || x as usize >= w || y as usize >= h {
                continue;
            }
            let mut acc = [0f32; 4];
            let mut wsum = 0.;
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let mut k = 1;
                while inside(x + dx * k, y + dy * k) {
                    k += 1;
                }
                let (xx, yy) = (x + dx * (k + 1), y + dy * (k + 1));
                if xx < 0 || yy < 0 || xx as usize >= w || yy as usize >= h {
                    continue;
                }
                let s = src[yy as usize * w + xx as usize];
                let wt = 1. / (k * k) as f32;
                for c in 0..4 {
                    acc[c] += s[c] as f32 * wt;
                }
                wsum += wt;
            }
            if wsum > 0. {
                d[y as usize * w + x as usize] = acc.map(|v| (v / wsum) as u8);
            }
        }
    }
}
