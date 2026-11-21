//! Metal backend for Dusty: the same picture as the CPU `Canvas`, drawn on the GPU.
//!
//! - every rig layer is a texture uploaded once; each draw is a transformed, bilinear-sampled quad
//!   (bending-ear bands discard pixels outside their strip in the fragment shader)
//! - eyes, mouths, fx and dust are anti-aliased shapes (ellipse / soft blob / capsule) plus triangles
//! - 4× MSAA, resolved, then a final pass converts premultiplied → straight-alpha BGRA, which is what
//!   GPUI's image sprites expect; the CPU only copies the finished frame out of shared memory.
//!
//! GPUI 0.2 can't composite our own texture with alpha (its `paint_surface` only takes YCbCr video
//! frames), so the frame goes through GPUI's normal image path; the expensive part (sampling and
//! blending ~200k pixels per layer) is now on the GPU.

use std::collections::HashMap;
use std::ffi::c_void;

use metal::*;

use super::raster::{Affine, Band, Layer, Painter, Px};

const SHADERS: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct LV { float2 pos; float2 src; };
struct LO { float4 pos [[position]]; float2 src; };
struct LU { float2 canvas; float2 origin; float2 size; float alpha; float band_on; float4 band; float2 band_u; float2 pad; };

vertex LO layer_vs(uint vid [[vertex_id]], constant LV* v [[buffer(0)]], constant LU& u [[buffer(1)]]) {
    LO o;
    float2 p = v[vid].pos;
    o.pos = float4(p.x / u.canvas.x * 2.0 - 1.0, 1.0 - p.y / u.canvas.y * 2.0, 0.0, 1.0);
    o.src = v[vid].src;
    return o;
}

fragment float4 layer_fs(LO in [[stage_in]], constant LU& u [[buffer(0)]], texture2d<float> t [[texture(0)]], sampler s [[sampler(0)]]) {
    if (u.band_on > 0.5) {
        float along = dot(in.src - u.band.xy, u.band.zw);
        if (along < u.band_u.x || along >= u.band_u.y) discard_fragment();
    }
    return t.sample(s, (in.src - u.origin) / u.size) * u.alpha;
}

// kind 0: ellipse (p0 = cx, cy, rx, ry; p1 = optional clip ellipse), 1: soft radial blob,
// 2: capsule (p0 = a.xy, b.xy in canvas px; p1.x = half width), 3: solid triangle
// 80 bytes, matching the Rust struct (a float3 here would be 16 bytes and break the stride)
struct SV { float2 pos; float2 local; float4 color; float4 p0; float4 p1; float kind; float pad0; float pad1; float pad2; };
struct SO { float4 pos [[position]]; float2 local; float2 canv; float4 color [[flat]]; float4 p0 [[flat]]; float4 p1 [[flat]]; float kind [[flat]]; };

vertex SO shape_vs(uint vid [[vertex_id]], constant SV* v [[buffer(0)]], constant float2& canvas [[buffer(1)]]) {
    SO o;
    float2 p = v[vid].pos;
    o.pos = float4(p.x / canvas.x * 2.0 - 1.0, 1.0 - p.y / canvas.y * 2.0, 0.0, 1.0);
    o.canv = p;
    o.local = v[vid].local;
    o.color = v[vid].color;
    o.p0 = v[vid].p0;
    o.p1 = v[vid].p1;
    o.kind = v[vid].kind;
    return o;
}

static float ellipse_cov(float2 l, float4 e) {
    float d = length((l - e.xy) / e.zw);
    float aa = max(fwidth(d), 1e-4);
    return clamp((1.0 - d) / aa + 0.5, 0.0, 1.0);
}

fragment float4 shape_fs(SO in [[stage_in]]) {
    float cov = 1.0;
    if (in.kind < 0.5) {
        cov = ellipse_cov(in.local, in.p0);
        if (in.p1.z > 0.0) cov *= ellipse_cov(in.local, in.p1);
    } else if (in.kind < 1.5) {
        float d = length((in.local - in.p0.xy) / in.p0.zw);
        cov = d < 1.0 ? pow(1.0 - d, 1.3) : 0.0;
    } else if (in.kind < 2.5) {
        float2 a = in.p0.xy, b = in.p0.zw, ab = b - a;
        float t = clamp(dot(in.canv - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
        cov = clamp(in.p1.x - length(in.canv - (a + ab * t)) + 0.5, 0.0, 1.0);
    }
    return in.color * cov;
}

struct FO { float4 pos [[position]]; };
vertex FO final_vs(uint vid [[vertex_id]]) {
    float2 p = float2((vid << 1) & 2, vid & 2);
    FO o; o.pos = float4(p * 2.0 - 1.0, 0.0, 1.0); return o;
}
fragment float4 final_fs(FO in [[stage_in]], texture2d<float> t [[texture(0)]]) {
    float4 c = t.read(uint2(in.pos.xy));
    return c.a > 0.0001 ? float4(c.rgb / c.a, c.a) : float4(0.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy)]
struct LV {
    pos: [f32; 2],
    src: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LU {
    canvas: [f32; 2],
    origin: [f32; 2],
    size: [f32; 2],
    alpha: f32,
    band_on: f32,
    band: [f32; 4],
    band_u: [f32; 2],
    pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SV {
    // layout must match the MSL `SV` exactly (80 bytes)
    pos: [f32; 2],
    local: [f32; 2],
    color: [f32; 4],
    p0: [f32; 4],
    p1: [f32; 4],
    kind: f32,
    pad: [f32; 3],
}

enum Cmd {
    Layer { key: usize, verts: [LV; 4], uni: LU },
    Shapes { start: usize, count: usize },
}

struct Targets {
    side: usize,
    msaa: Texture,
    resolved: Texture,
    out: Texture,
    /// linear shared buffer the GPU copies the finished frame into (un-tiling on the GPU, not the CPU)
    readback: Buffer,
}

pub struct Gpu {
    device: Device,
    queue: CommandQueue,
    layer_pso: RenderPipelineState,
    shape_pso: RenderPipelineState,
    final_pso: RenderPipelineState,
    sampler: SamplerState,
    textures: HashMap<usize, (Texture, i32, i32, usize, usize)>,
    targets: Option<Targets>,
    unified: bool,
    // per-frame recording
    cmds: Vec<Cmd>,
    shapes: Vec<SV>,
    side: usize,
}

fn blend_pipeline(device: &Device, lib: &Library, vs: &str, fs: &str, format: MTLPixelFormat, samples: u64, blend: bool) -> Option<RenderPipelineState> {
    let desc = RenderPipelineDescriptor::new();
    let (vf, ff) = (lib.get_function(vs, None).ok()?, lib.get_function(fs, None).ok()?);
    desc.set_vertex_function(Some(&vf));
    desc.set_fragment_function(Some(&ff));
    desc.set_raster_sample_count(samples);
    let ca = desc.color_attachments().object_at(0)?;
    ca.set_pixel_format(format);
    if blend {
        // premultiplied alpha "over"
        ca.set_blending_enabled(true);
        ca.set_rgb_blend_operation(MTLBlendOperation::Add);
        ca.set_alpha_blend_operation(MTLBlendOperation::Add);
        ca.set_source_rgb_blend_factor(MTLBlendFactor::One);
        ca.set_source_alpha_blend_factor(MTLBlendFactor::One);
        ca.set_destination_rgb_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        ca.set_destination_alpha_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
    }
    device.new_render_pipeline_state(&desc).map_err(|e| eprintln!("dusty gpu: {e}")).ok()
}

impl Gpu {
    /// None when Metal isn't available or the shaders don't compile (the CPU renderer takes over).
    pub fn new() -> Option<Gpu> {
        let device = Device::system_default()?;
        let lib = device.new_library_with_source(SHADERS, &CompileOptions::new()).map_err(|e| eprintln!("dusty gpu: {e}")).ok()?;
        let layer_pso = blend_pipeline(&device, &lib, "layer_vs", "layer_fs", MTLPixelFormat::RGBA8Unorm, 4, true)?;
        let shape_pso = blend_pipeline(&device, &lib, "shape_vs", "shape_fs", MTLPixelFormat::RGBA8Unorm, 4, true)?;
        let final_pso = blend_pipeline(&device, &lib, "final_vs", "final_fs", MTLPixelFormat::BGRA8Unorm, 1, false)?;
        let sd = SamplerDescriptor::new();
        sd.set_min_filter(MTLSamplerMinMagFilter::Linear);
        sd.set_mag_filter(MTLSamplerMinMagFilter::Linear);
        sd.set_address_mode_s(MTLSamplerAddressMode::ClampToZero);
        sd.set_address_mode_t(MTLSamplerAddressMode::ClampToZero);
        let sampler = device.new_sampler(&sd);
        let queue = device.new_command_queue();
        let unified = device.has_unified_memory();
        Some(Gpu { device, queue, layer_pso, shape_pso, final_pso, sampler, textures: HashMap::new(), targets: None, unified, cmds: vec![], shapes: vec![], side: 0 })
    }

    fn texture_for(&mut self, l: &Layer) -> usize {
        let key = l as *const Layer as usize;
        if !self.textures.contains_key(&key) {
            let td = TextureDescriptor::new();
            td.set_pixel_format(MTLPixelFormat::RGBA8Unorm);
            td.set_width(l.w as u64);
            td.set_height(l.h as u64);
            td.set_usage(MTLTextureUsage::ShaderRead);
            td.set_storage_mode(if self.unified { MTLStorageMode::Shared } else { MTLStorageMode::Managed });
            let tex = self.device.new_texture(&td);
            let bytes: Vec<u8> = l.px.iter().flat_map(|p| p.map(|v| (v.clamp(0., 1.) * 255. + 0.5) as u8)).collect();
            tex.replace_region(MTLRegion::new_2d(0, 0, l.w as u64, l.h as u64), 0, bytes.as_ptr() as *const c_void, (l.w * 4) as u64);
            self.textures.insert(key, (tex, l.x0, l.y0, l.w, l.h));
        }
        key
    }

    fn ensure_targets(&mut self, side: usize) {
        if self.targets.as_ref().is_some_and(|t| t.side == side) {
            return;
        }
        let make = |fmt: MTLPixelFormat, samples: u64, usage: MTLTextureUsage, storage: MTLStorageMode| {
            let td = TextureDescriptor::new();
            td.set_pixel_format(fmt);
            td.set_width(side as u64);
            td.set_height(side as u64);
            if samples > 1 {
                td.set_texture_type(MTLTextureType::D2Multisample);
                td.set_sample_count(samples);
            }
            td.set_usage(usage);
            td.set_storage_mode(storage);
            self.device.new_texture(&td)
        };
        let msaa = make(MTLPixelFormat::RGBA8Unorm, 4, MTLTextureUsage::RenderTarget, MTLStorageMode::Private);
        let resolved = make(MTLPixelFormat::RGBA8Unorm, 1, MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead, MTLStorageMode::Private);
        let out = make(MTLPixelFormat::BGRA8Unorm, 1, MTLTextureUsage::RenderTarget, MTLStorageMode::Private);
        let readback = self.device.new_buffer((side * side * 4) as u64, if self.unified { MTLResourceOptions::StorageModeShared } else { MTLResourceOptions::StorageModeManaged });
        self.targets = Some(Targets { side, msaa, resolved, out, readback });
    }

    /// Record a frame with `draw` (Dusty's renderer), run it on the GPU, return straight-alpha BGRA bytes.
    pub fn frame(&mut self, side: usize, draw: impl FnOnce(&mut dyn Painter)) -> Option<Vec<u8>> {
        objc::rc::autoreleasepool(|| {
            self.side = side;
            self.cmds.clear();
            self.shapes.clear();
            draw(self);
            self.ensure_targets(side);
            let t = self.targets.as_ref()?;
            let cb = self.queue.new_command_buffer();

            let pass = RenderPassDescriptor::new();
            let ca = pass.color_attachments().object_at(0)?;
            ca.set_texture(Some(&t.msaa));
            ca.set_resolve_texture(Some(&t.resolved));
            ca.set_load_action(MTLLoadAction::Clear);
            ca.set_clear_color(MTLClearColor::new(0., 0., 0., 0.));
            ca.set_store_action(MTLStoreAction::MultisampleResolve);
            let enc = cb.new_render_command_encoder(pass);
            let canvas = [side as f32, side as f32];
            let shape_buf = (!self.shapes.is_empty()).then(|| {
                self.device.new_buffer_with_data(self.shapes.as_ptr() as *const c_void, (self.shapes.len() * std::mem::size_of::<SV>()) as u64, MTLResourceOptions::StorageModeShared)
            });
            for c in &self.cmds {
                match c {
                    Cmd::Layer { key, verts, uni } => {
                        let Some((tex, ..)) = self.textures.get(key) else { continue };
                        enc.set_render_pipeline_state(&self.layer_pso);
                        enc.set_vertex_bytes(0, std::mem::size_of_val(verts) as u64, verts.as_ptr() as *const c_void);
                        enc.set_vertex_bytes(1, std::mem::size_of::<LU>() as u64, uni as *const LU as *const c_void);
                        enc.set_fragment_bytes(0, std::mem::size_of::<LU>() as u64, uni as *const LU as *const c_void);
                        enc.set_fragment_texture(0, Some(tex));
                        enc.set_fragment_sampler_state(0, Some(&self.sampler));
                        enc.draw_primitives(MTLPrimitiveType::TriangleStrip, 0, 4);
                    }
                    Cmd::Shapes { start, count } => {
                        let Some(buf) = &shape_buf else { continue };
                        enc.set_render_pipeline_state(&self.shape_pso);
                        enc.set_vertex_buffer(0, Some(buf), (*start * std::mem::size_of::<SV>()) as u64);
                        enc.set_vertex_bytes(1, 8, canvas.as_ptr() as *const c_void);
                        enc.draw_primitives(MTLPrimitiveType::Triangle, 0, *count as u64);
                    }
                }
            }
            enc.end_encoding();

            let fin = RenderPassDescriptor::new();
            let fa = fin.color_attachments().object_at(0)?;
            fa.set_texture(Some(&t.out));
            fa.set_load_action(MTLLoadAction::DontCare);
            fa.set_store_action(MTLStoreAction::Store);
            let fe = cb.new_render_command_encoder(fin);
            fe.set_render_pipeline_state(&self.final_pso);
            fe.set_fragment_texture(0, Some(&t.resolved));
            fe.draw_primitives(MTLPrimitiveType::Triangle, 0, 3);
            fe.end_encoding();
            let blit = cb.new_blit_command_encoder();
            let size = MTLSize { width: side as u64, height: side as u64, depth: 1 };
            blit.copy_from_texture_to_buffer(&t.out, 0, 0, MTLOrigin { x: 0, y: 0, z: 0 }, size, &t.readback, 0, (side * 4) as u64, (side * side * 4) as u64, MTLBlitOption::empty());
            if !self.unified {
                blit.synchronize_resource(&t.readback);
            }
            blit.end_encoding();
            cb.commit();
            cb.wait_until_completed();

            let src = unsafe { std::slice::from_raw_parts(t.readback.contents() as *const u8, side * side * 4) };
            Some(src.to_vec())
        })
    }

    fn push_shapes(&mut self, verts: &[SV]) {
        let start = self.shapes.len();
        self.shapes.extend_from_slice(verts);
        // merge with the previous shape draw when they're adjacent (keeps draw calls low)
        if let Some(Cmd::Shapes { start: s, count }) = self.cmds.last_mut() {
            if *s + *count == start {
                *count += verts.len();
                return;
            }
        }
        self.cmds.push(Cmd::Shapes { start, count: verts.len() });
    }

    /// A quad given by its 4 corners (canvas px) and their local coords, as two triangles.
    fn quad(&mut self, corners: [(f32, f32); 4], local: [(f32, f32); 4], color: Px, p0: [f32; 4], p1: [f32; 4], kind: f32) {
        let v = |i: usize| SV { pos: [corners[i].0, corners[i].1], local: [local[i].0, local[i].1], color, p0, p1, kind, pad: [0.; 3] };
        self.push_shapes(&[v(0), v(1), v(2), v(1), v(2), v(3)]);
    }

    fn layer_cmd(&mut self, l: &Layer, m: Affine, alpha: f32, region: [(f32, f32); 4], band: Option<Band>) {
        let key = self.texture_for(l);
        let verts = region.map(|(x, y)| {
            let (cx, cy) = m.apply(x, y);
            LV { pos: [cx, cy], src: [x, y] }
        });
        let s = self.side as f32;
        let uni = LU {
            canvas: [s, s],
            origin: [l.x0 as f32, l.y0 as f32],
            size: [l.w as f32, l.h as f32],
            alpha,
            band_on: if band.is_some() { 1. } else { 0. },
            band: band.map(|b| [b.p.0, b.p.1, b.u.0, b.u.1]).unwrap_or_default(),
            band_u: band.map(|b| [b.u0, b.u1]).unwrap_or_default(),
            pad: [0.; 2],
        };
        self.cmds.push(Cmd::Layer { key, verts, uni });
    }
}

impl Painter for Gpu {
    fn side(&self) -> usize {
        self.side
    }
    fn begin(&mut self) {}
    fn draw_layer(&mut self, l: &Layer, m: Affine, alpha: f32, _clip: Option<&dyn Fn(f32, f32) -> bool>) {
        let (x0, y0) = (l.x0 as f32, l.y0 as f32);
        let (x1, y1) = (x0 + l.w as f32, y0 + l.h as f32);
        self.layer_cmd(l, m, alpha, [(x0, y0), (x1, y0), (x0, y1), (x1, y1)], None);
    }
    fn draw_band(&mut self, l: &Layer, m: Affine, band: Band, region: [(f32, f32); 4]) {
        self.layer_cmd(l, m, 1., region, Some(band));
    }
    fn fill_ellipse(&mut self, m: Affine, c: (f32, f32), r: (f32, f32), color: Px, clip: Option<((f32, f32), (f32, f32))>) {
        let pad = 1.5 / m.scale_factor().max(1e-3);
        let local = [(c.0 - r.0 - pad, c.1 - r.1 - pad), (c.0 + r.0 + pad, c.1 - r.1 - pad), (c.0 - r.0 - pad, c.1 + r.1 + pad), (c.0 + r.0 + pad, c.1 + r.1 + pad)];
        let corners = local.map(|(x, y)| m.apply(x, y));
        let p1 = clip.map(|(cc, rr)| [cc.0, cc.1, rr.0, rr.1]).unwrap_or([0., 0., 0., 0.]);
        self.quad(corners, local, color, [c.0, c.1, r.0, r.1], p1, 0.);
    }
    fn fill_radial(&mut self, m: Affine, c: (f32, f32), r: (f32, f32), color: Px) {
        let local = [(c.0 - r.0, c.1 - r.1), (c.0 + r.0, c.1 - r.1), (c.0 - r.0, c.1 + r.1), (c.0 + r.0, c.1 + r.1)];
        let corners = local.map(|(x, y)| m.apply(x, y));
        self.quad(corners, local, color, [c.0, c.1, r.0, r.1], [0.; 4], 1.);
    }
    fn stroke(&mut self, m: Affine, pts: &[(f32, f32)], width: f32, color: Px) {
        let hw = width * m.scale_factor() / 2.;
        let tp: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| m.apply(x, y)).collect();
        for s in tp.windows(2) {
            let (a, b) = (s[0], s[1]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt().max(1e-4);
            let (ux, uy) = (dx / len, dy / len);
            let e = hw + 1.5;
            let (nx, ny) = (-uy * e, ux * e);
            let corners = [
                (a.0 - ux * e + nx, a.1 - uy * e + ny),
                (b.0 + ux * e + nx, b.1 + uy * e + ny),
                (a.0 - ux * e - nx, a.1 - uy * e - ny),
                (b.0 + ux * e - nx, b.1 + uy * e - ny),
            ];
            self.quad(corners, [(0., 0.); 4], color, [a.0, a.1, b.0, b.1], [hw, 0., 0., 0.], 2.);
        }
    }
    fn fill_poly(&mut self, m: Affine, pts: &[(f32, f32)], color: Px) {
        if pts.len() < 3 {
            return;
        }
        // fan from the centroid (Dusty's polygons are star-shaped); MSAA smooths the edges
        let tp: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| m.apply(x, y)).collect();
        let n = tp.len() as f32;
        let c = (tp.iter().map(|p| p.0).sum::<f32>() / n, tp.iter().map(|p| p.1).sum::<f32>() / n);
        let v = |p: (f32, f32)| SV { pos: [p.0, p.1], color, kind: 3., ..Default::default() };
        let mut tris = Vec::with_capacity(tp.len() * 3);
        for i in 0..tp.len() {
            tris.extend([v(c), v(tp[i]), v(tp[(i + 1) % tp.len()])]);
        }
        self.push_shapes(&tris);
    }
}

const _: () = assert!(std::mem::size_of::<SV>() == 80 && std::mem::size_of::<LU>() == 64 && std::mem::size_of::<LV>() == 16);
