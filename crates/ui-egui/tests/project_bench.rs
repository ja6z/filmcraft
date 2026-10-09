//! Frame cost of a real project (ignored; run on demand):
//!
//! ```text
//! FILMCRAFT_BENCH_PROJECT=/path/project.fcproj FILMCRAFT_BENCH_AT=50 \
//!   cargo test --release -p filmcraft-ui-egui --test project_bench -- --ignored --nocapture
//! ```
//!
//! For 2 s of the active sequence from `FILMCRAFT_BENCH_AT` seconds (default 10) it reports, per
//! frame at ½ resolution: the CPU render (what the preview did when a frame fell back) and the GPU
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
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let opts = RenderOptions { scale: 0.5, ..Default::default() };
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
    let frames: Vec<Tick> = (0..60).map(|k| rate.tick_of(first + k)).collect();
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
    eprintln!("{} frames from {at} s at ½ resolution:", frames.len());
    eprintln!("  CPU render           p50 {c50:6.1} ms  p95 {c95:6.1} ms");
    eprintln!("  GPU path (plan+draw) p50 {g50:6.1} ms  p95 {g95:6.1} ms   (plan alone p50 {p50:.1} ms, p95 {p95:.1} ms)");
    eprintln!("  frames planned as GPU layers: {layered} of {}", frames.len());
}
