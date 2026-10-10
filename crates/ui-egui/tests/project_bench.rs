//! Frame cost of a real project (ignored; run on demand):
//!
//! ```text
//! FILMCRAFT_BENCH_PROJECT=/path/project.fcproj FILMCRAFT_BENCH_AT=50 \
//!   cargo test --release -p filmcraft-ui-egui --test project_bench -- --ignored --nocapture
//! ```
//!
//! For `FILMCRAFT_BENCH_FRAMES` frames (default 60) of the active sequence from `FILMCRAFT_BENCH_AT`
//! seconds (default 10) it reports, per frame at `FILMCRAFT_BENCH_SCALE` (default ½ resolution): the CPU render (what the preview did when a frame fell back) and the GPU
//! path (`plan_frame` + the compositor, read back), and how many frames planned as GPU layers.

use std::time::Instant;

use filmcraft_engine::Session;
use filmcraft_gpu::GpuCompositor;
use filmcraft_render::RenderOptions;
use filmcraft_render::plan::{FramePlan, plan_frame};
use filmcraft_time::Tick;
use serde_json::json;

fn percentiles(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[v.len() / 2], v[(v.len() * 95 / 100).min(v.len() - 1)])
}

#[test]
#[ignore = "benchmark of a user project; set FILMCRAFT_BENCH_PROJECT"]
fn frame_cost_of_a_project() {
    let Ok(path) = std::env::var("FILMCRAFT_BENCH_PROJECT") else {
        eprintln!("FILMCRAFT_BENCH_PROJECT not set");
        return;
    };
    let at: f64 = std::env::var("FILMCRAFT_BENCH_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(10.0);
    filmcraft_platform::register();
    let mut s = Session::default();
    s.execute("file.open", json!({"path": path})).expect("open");
    let seq = s.state.active_sequence.expect("active sequence");
    let rate = s.project.sequence(seq).expect("sequence").settings.frame_rate;
    let scale: f32 = std::env::var("FILMCRAFT_BENCH_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5);
    let count: i64 = std::env::var("FILMCRAFT_BENCH_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let opts = RenderOptions { scale, ..Default::default() };
    // full resolution renders the original media, as an export does; below it the proxies
    let provider =
        if scale >= 1.0 { s.media.full_res_provider(s.project.clone(), s.services.clone()) } else { s.media.provider(s.project.clone(), s.services.clone()) };
    let Some((dev, q)) = (|| {
        let instance = eframe::wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&eframe::wgpu::RequestAdapterOptions::default())).ok()?;
        pollster::block_on(adapter.request_device(&eframe::wgpu::DeviceDescriptor::default())).ok()
    })() else {
        eprintln!("no GPU adapter");
        return;
    };
    let mut gpu = GpuCompositor::new(&dev, &q);
    let first = rate.frame_at(Tick::from_seconds_f64(at));
    let frames: Vec<Tick> = (0..count).map(|k| rate.tick_of(first + k)).collect();
    // warm the decoders
    for t in frames.iter().take(3) {
        let _ = filmcraft_render::render_sequence(&s.project, seq, *t, opts, &provider);
    }
    let (mut cpu_ms, mut plan_ms, mut gpu_ms) = (Vec::new(), Vec::new(), Vec::new());
    let mut layered = 0;
    for t in &frames {
        let t0 = Instant::now();
        let _ = filmcraft_render::render_sequence(&s.project, seq, *t, opts, &provider);
        cpu_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        let t1 = Instant::now();
        let plan = plan_frame(&s.project, seq, *t, opts, &provider);
        plan_ms.push(t1.elapsed().as_secs_f64() * 1000.0);
        if matches!(plan, FramePlan::Layers { .. }) {
            layered += 1;
        }
        gpu.composite(&plan);
        let _ = gpu.read_output();
        gpu_ms.push(t1.elapsed().as_secs_f64() * 1000.0);
    }
    let (c50, c95) = percentiles(cpu_ms);
    let (p50, p95) = percentiles(plan_ms);
    let (g50, g95) = percentiles(gpu_ms);
    eprintln!("{} frames from {at} s at scale {scale}:", frames.len());
    eprintln!("  CPU render           p50 {c50:6.1} ms  p95 {c95:6.1} ms");
    eprintln!("  GPU path (plan+draw) p50 {g50:6.1} ms  p95 {g95:6.1} ms   (plan alone p50 {p50:.1} ms, p95 {p95:.1} ms)");
    eprintln!("  frames planned as GPU layers: {layered} of {}", frames.len());
}

/// The GPU path against the CPU render at `FILMCRAFT_BENCH_AT` (full resolution), both written as
/// PPM to `FILMCRAFT_BENCH_OUT` (default the temp dir) with their PSNR.
#[test]
#[ignore = "compares a user project's frame; set FILMCRAFT_BENCH_PROJECT"]
fn gpu_frame_of_a_project_matches_the_cpu() {
    let Ok(path) = std::env::var("FILMCRAFT_BENCH_PROJECT") else {
        eprintln!("FILMCRAFT_BENCH_PROJECT not set");
        return;
    };
    let at: f64 = std::env::var("FILMCRAFT_BENCH_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(10.0);
    let scale: f32 = std::env::var("FILMCRAFT_BENCH_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.25);
    let out = std::env::var("FILMCRAFT_BENCH_OUT").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    filmcraft_platform::register();
    let mut s = Session::default();
    s.execute("file.open", json!({"path": path})).expect("open");
    let seq = s.state.active_sequence.expect("active sequence");
    let rate = s.project.sequence(seq).expect("sequence").settings.frame_rate;
    // FILMCRAFT_BENCH_PROXIES=1: the media the preview plays (proxies when attached and enabled)
    let provider = if std::env::var_os("FILMCRAFT_BENCH_PROXIES").is_some() {
        s.media.provider(s.project.clone(), s.services.clone())
    } else {
        s.media.full_res_provider(s.project.clone(), s.services.clone())
    };
    let opts = RenderOptions { scale, ..Default::default() };
    let Some((dev, q)) = (|| {
        let instance = eframe::wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&eframe::wgpu::RequestAdapterOptions::default())).ok()?;
        pollster::block_on(adapter.request_device(&eframe::wgpu::DeviceDescriptor::default())).ok()
    })() else {
        eprintln!("no GPU adapter");
        return;
    };
    let gpu = filmcraft_gpu::GpuFrameRenderer::new(&dev, &q).expect("effect stage");
    let t = rate.tick_of(rate.frame_at(Tick::from_seconds_f64(at)));
    let cpu = filmcraft_render::render_sequence(&s.project, seq, t, opts, &provider);
    let g = filmcraft_render::FrameRenderer::render(&gpu, &s.project, seq, t, opts, &provider);
    let layered = matches!(plan_frame(&s.project, seq, t, opts, &provider), FramePlan::Layers { .. });
    let (a, b) = (cpu.to_rgba8(), g.to_rgba8());
    let se: f64 = a.iter().zip(&b).enumerate().filter(|(i, _)| i % 4 != 3).map(|(_, (x, y))| (*x as f64 - *y as f64).powi(2)).sum();
    let psnr = 10.0 * (255.0f64.powi(2) / (se / (a.len() as f64 * 0.75)).max(1e-9)).log10();
    for (name, px) in [("cpu", &a), ("gpu", &b)] {
        let mut ppm = format!("P6\n{} {}\n255\n", cpu.w, cpu.h).into_bytes();
        ppm.extend(px.chunks(4).flat_map(|p| [p[0], p[1], p[2]]));
        let tag = if std::env::var_os("FILMCRAFT_BENCH_PROXIES").is_some() { "_proxy" } else { "" };
        std::fs::write(out.join(format!("frame_{at}{tag}_{name}.ppm")), ppm).expect("write");
    }
    eprintln!("{at} s ({}x{}, layered {layered}): GPU vs CPU {psnr:.1} dB", cpu.w, cpu.h);
}

/// Where a project plays slowly (ignored; run on demand): `FILMCRAFT_BENCH_RUN` consecutive frames
/// (default 6) every `FILMCRAFT_BENCH_STEP` seconds (default 2) through the whole sequence, on the
/// proxies at `FILMCRAFT_BENCH_SCALE` (default ½) as the preview plays them. Per sample: the
/// steady-state cost (median of the frames after the first two: decoders warm, as in playback)
/// of planning plus preparing the layers (the frame workers' part) and of drawing them (the
/// GPU's), whether the frame fell back to the CPU, and what is on the video
/// tracks (clip id, standard effects, speed / interpolation). Samples over 33 ms are marked.
#[test]
#[ignore = "benchmark of a user project; set FILMCRAFT_BENCH_PROJECT"]
fn timeline_scan() {
    let Ok(path) = std::env::var("FILMCRAFT_BENCH_PROJECT") else {
        eprintln!("FILMCRAFT_BENCH_PROJECT not set");
        return;
    };
    let get = |k: &str, d: f64| std::env::var(k).ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(d);
    let (step, run, scale) = (get("FILMCRAFT_BENCH_STEP", 2.0), get("FILMCRAFT_BENCH_RUN", 6.0) as i64, get("FILMCRAFT_BENCH_SCALE", 0.5) as f32);
    filmcraft_platform::register();
    let mut s = Session::default();
    s.execute("file.open", json!({"path": path})).expect("open");
    let seq = s.state.active_sequence.expect("active sequence");
    let q = s.project.sequence(seq).expect("sequence");
    let (rate, dur) = (q.settings.frame_rate, q.duration());
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let opts = RenderOptions { scale, ..Default::default() };
    let Some((dev, queue)) = (|| {
        let instance = eframe::wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&eframe::wgpu::RequestAdapterOptions::default())).ok()?;
        pollster::block_on(adapter.request_device(&eframe::wgpu::DeviceDescriptor::default())).ok()
    })() else {
        eprintln!("no GPU adapter");
        return;
    };
    let mut gpu = GpuCompositor::new(&dev, &queue);
    // (second, plan ms, GPU ms, fell back to a CPU image, layers, what's on the tracks)
    let mut rows: Vec<(f64, f64, f64, bool, usize, String)> = Vec::new();
    let mut at = 0.0;
    while at < dur.seconds() - 0.2 {
        let first = rate.frame_at(Tick::from_seconds_f64(at));
        let (mut plan_ms, mut gpu_ms, mut image) = (Vec::new(), Vec::new(), false);
        let mut layers = 0;
        for k in 0..run {
            let t = rate.tick_of(first + k);
            let t0 = Instant::now();
            let plan = plan_frame(&s.project, seq, t, opts, &provider);
            let planned = t0.elapsed().as_secs_f64() * 1000.0;
            let t1 = Instant::now();
            let mut prepared = 0.0;
            match &plan {
                FramePlan::Layers { layers: l, .. } => {
                    layers = l.len();
                    // the frame workers convert and upload-prepare the layers; the UI thread draws
                    let prep = filmcraft_gpu::prepare(&plan);
                    prepared = t1.elapsed().as_secs_f64() * 1000.0;
                    gpu.composite_prepared(&plan, Some(&prep));
                    let _ = dev.poll(eframe::wgpu::PollType::wait_indefinitely());
                }
                FramePlan::Image(_) => image = true,
            }
            if k >= 2 {
                // (plan + prepare run on the workers; the draw is the GPU's own cost)
                plan_ms.push(planned + prepared);
                gpu_ms.push(t1.elapsed().as_secs_f64() * 1000.0 - prepared);
            }
        }
        plan_ms.sort_by(f64::total_cmp);
        gpu_ms.sort_by(f64::total_cmp);
        let t = Tick::from_seconds_f64(at + 0.1);
        let q = s.project.sequence(seq).expect("sequence");
        let mut what = String::new();
        for (v, tr) in q.video_tracks.iter().enumerate() {
            if let Some(it) = tr.item_at(t) {
                let fx: Vec<&str> = it.effects.iter().filter(|e| e.enabled && e.def().is_some_and(|d| !d.intrinsic)).map(|e| e.effect.as_str()).collect();
                let masks: usize = it.effects.iter().map(|e| e.masks.len()).sum();
                let speed = if (it.speed - 1.0).abs() > 1e-6 { format!(" {:.0}%", it.speed * 100.0) } else { String::new() };
                let fx = if fx.is_empty() {
                    String::new()
                } else {
                    format!(" [{}{}]", fx.join(","), if masks > 0 { format!(" masks {masks}") } else { String::new() })
                };
                what += &format!("V{}:{}{speed}{fx} ", v + 1, it.id.0);
            }
        }
        rows.push((at, plan_ms[plan_ms.len() / 2], gpu_ms[gpu_ms.len() / 2], image, layers, what));
        at += step;
    }
    for (at, pl, gp, image, layers, what) in &rows {
        let flag = if *pl > 30.0 || *gp > 25.0 || *image { "SLOW" } else { "    " };
        eprintln!("{flag} {at:7.1} s  plan {pl:6.1} ms  gpu {gp:6.1} ms  {}  layers {layers:2}  {what}", if *image { "CPU image" } else { "gpu      " });
    }
    let slow = rows.iter().filter(|r| r.1 > 30.0 || r.2 > 25.0 || r.3).count();
    let mut plans: Vec<f64> = rows.iter().map(|r| r.1).collect();
    let mut gpus: Vec<f64> = rows.iter().map(|r| r.2).collect();
    plans.sort_by(f64::total_cmp);
    gpus.sort_by(f64::total_cmp);
    eprintln!(
        "{} samples, {slow} slow (plan > 30 ms or gpu > 25 ms); plan p50 {:.1} p90 {:.1} max {:.1} ms; gpu p50 {:.1} p90 {:.1} max {:.1} ms",
        rows.len(),
        plans[plans.len() / 2],
        plans[plans.len() * 9 / 10],
        plans[plans.len() - 1],
        gpus[gpus.len() / 2],
        gpus[gpus.len() * 9 / 10],
        gpus[gpus.len() - 1]
    );
}
