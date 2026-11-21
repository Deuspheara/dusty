//! Dusty: free disk space on macOS without throwing away things you'd have to re-download.
//! Native GPUI app; the scanning/cleaning engine is a Rust port of the `dusty` CLI.

mod dusty;
mod engine;
mod platform;
mod ui;

use std::borrow::Cow;

use gpui::*;

actions!(dusty, [Quit, IconPurple, IconMint, IconCream]);

/// The Dusty menu, with the current icon color ticked.
fn menus(current: platform::IconVariant) -> Vec<Menu> {
    let item = |v: platform::IconVariant| {
        let name = if v == current { format!("✓ {}", v.label()) } else { format!("    {}", v.label()) };
        match v {
            platform::IconVariant::Purple => MenuItem::action(name, IconPurple),
            platform::IconVariant::Mint => MenuItem::action(name, IconMint),
            platform::IconVariant::Cream => MenuItem::action(name, IconCream),
        }
    };
    vec![Menu {
        name: "Dusty".into(),
        items: vec![
            MenuItem::submenu(Menu { name: "App Icon".into(), items: platform::IconVariant::ALL.into_iter().map(item).collect() }),
            MenuItem::separator(),
            MenuItem::action("Quit Dusty", Quit),
        ],
    }]
}

fn set_icon(v: platform::IconVariant, cx: &mut App) {
    platform::apply_icon(v, true);
    cx.set_menus(menus(v));
}

pub static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
    START.get_or_init(std::time::Instant::now);
    engine::fix_path();
    // headless mode for checking the engine against the CLI: `Dusty --scan`
    if std::env::args().any(|a| a == "--scan") {
        let progress = std::sync::Arc::new(std::sync::Mutex::new(engine::ScanProgress::default()));
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let opts = engine::ScanOptions { roots: engine::default_roots(), keep_recent_days: 0, min_size_mb: 1 };
        let t0 = std::time::Instant::now();
        let targets = engine::scan_all(&opts, progress, cancel);
        for t in &targets {
            println!("{:?}\t{}\t{}\t{}", t.group, t.default_on, t.bytes, t.label);
        }
        eprintln!("{} items in {:.1}s", targets.len(), t0.elapsed().as_secs_f32());
        return;
    }
    // headless benchmark: `Dusty --bench` (rig build + per-frame compose cost)
    if std::env::args().any(|a| a == "--bench") {
        let t0 = std::time::Instant::now();
        let rig = dusty::rig().expect("rig");
        let build = t0.elapsed();
        let mut st = dusty::State::new();
        for (label, side) in [("250pt @2x (home)", 500usize), ("118pt @2x (results)", 236), ("84pt @2x (pick)", 168)] {
            for mood in [dusty::Mood::Idle, dusty::Mood::Think, dusty::Mood::Sad] {
                st.set(mood, None);
                st.dirt = if mood == dusty::Mood::Think { 0.8 } else { 0. };
                let mut cv = dusty::raster::Canvas::new(side, side);
                let n = 300;
                let t = std::time::Instant::now();
                for _ in 0..n {
                    st.advance(1. / 60.);
                    dusty::render(&rig, &st, &mut cv);
                    std::hint::black_box(cv.to_bgra());
                }
                let per = t.elapsed().as_secs_f64() * 1000. / n as f64;
                println!("{label:<22} {:<5} {per:6.2} ms/frame  ({:.0} fps max on one core)", format!("{mood:?}"), 1000. / per);
            }
        }
        #[cfg(target_os = "macos")]
        if let Some(mut g) = dusty::gpu::Gpu::new() {
            for (label, side) in [("GPU 250pt @2x", 500usize), ("GPU 118pt @2x", 236)] {
                for mood in [dusty::Mood::Idle, dusty::Mood::Think, dusty::Mood::Sad] {
                    st.set(mood, None);
                    st.dirt = if mood == dusty::Mood::Think { 0.8 } else { 0. };
                    let n = 300;
                    let cpu0 = cpu_time();
                    let t = std::time::Instant::now();
                    for _ in 0..n {
                        st.advance(1. / 60.);
                        std::hint::black_box(g.frame(side, |p| dusty::render(&rig, &st, p)));
                    }
                    let wall = t.elapsed().as_secs_f64() * 1000. / n as f64;
                    let cpu = (cpu_time() - cpu0) * 1000. / n as f64;
                    println!("{label:<22} {:<5} {wall:6.2} ms/frame wall, {cpu:5.2} ms CPU", format!("{mood:?}"));
                }
            }
        }
        // GPU vs CPU: same pose, same frame → how many pixels differ noticeably
        #[cfg(target_os = "macos")]
        if let (Some(mut g), Ok(dir)) = (dusty::gpu::Gpu::new(), std::env::var("DUSTY_BENCH_OUT")) {
            for mood in [dusty::Mood::Idle, dusty::Mood::Sad, dusty::Mood::Happy] {
                let mut st = dusty::State::new();
                st.set(mood, None);
                st.advance(0.4);
                let side = 500;
                let mut cv = dusty::raster::Canvas::new(side, side);
                dusty::render(&rig, &st, &mut cv);
                let cpu = cv.to_bgra();
                let gpu = g.frame(side, |p| dusty::render(&rig, &st, p)).unwrap();
                let (mut diff, mut seen) = (0, 0);
                for (a, b) in cpu.chunks(4).zip(gpu.chunks(4)) {
                    if a[3] < 8 && b[3] < 8 {
                        continue;
                    }
                    seen += 1;
                    // compare premultiplied colour, so faint anti-aliased edges don't dominate
                    let pm = |p: &[u8], i: usize| p[i] as i32 * p[3] as i32 / 255;
                    let d = (0..3).map(|i| (pm(a, i) - pm(b, i)).abs()).max().unwrap().max((a[3] as i32 - b[3] as i32).abs());
                    if d > 24 {
                        diff += 1;
                    }
                }
                println!("GPU vs CPU {:<6} {:.2}% of pixels differ", format!("{mood:?}"), diff as f64 * 100. / seen.max(1) as f64);
                for (name, bytes) in [("cpu", &cpu), ("gpu", &gpu)] {
                    let rgba: Vec<u8> = bytes.chunks(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
                    let _ = image::RgbaImage::from_raw(side as u32, side as u32, rgba).unwrap().save(format!("{dir}/{name}-{mood:?}.png"));
                }
            }
        }
        println!("rig build (cut + inpaint + masks): {:.0} ms", build.as_secs_f64() * 1000.);
        return;
    }
    // warm the mascot rig off the main thread so the first frame is instant
    std::thread::spawn(|| {
        let _ = dusty::rig();
    });

    Application::new().run(|cx: &mut App| {
        if let Ok(font) = std::fs::read("/System/Library/Fonts/SFNSRounded.ttf") {
            let _ = cx.text_system().add_fonts(vec![Cow::Owned(font)]);
        }
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None), KeyBinding::new("cmd-w", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &IconPurple, cx| set_icon(platform::IconVariant::Purple, cx));
        cx.on_action(|_: &IconMint, cx| set_icon(platform::IconVariant::Mint, cx));
        cx.on_action(|_: &IconCream, cx| set_icon(platform::IconVariant::Cream, cx));
        let icon = platform::saved_icon();
        if icon != platform::IconVariant::Purple {
            platform::apply_icon(icon, false);
        }
        cx.set_menus(menus(icon));
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(440.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(400.), px(660.))),
                titlebar: Some(TitlebarOptions { title: Some("Dusty".into()), appears_transparent: true, traffic_light_position: Some(point(px(18.), px(20.))) }),
                app_id: Some("io.github.deuspheara.dusty".into()),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| ui::DustyApp::new(window, cx)),
        )
        .expect("open window");
        cx.activate(true);
    });
}

/// CPU time used by this process so far (seconds): user + system.
#[allow(dead_code)]
fn cpu_time() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    tv(ru.ru_utime) + tv(ru.ru_stime)
}
