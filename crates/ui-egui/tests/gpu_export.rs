//! Exports render on the GPU ([`filmcraft_gpu::GpuFrameRenderer`], what the app hands the engine as
//! `Session::frame_renderer`): its frames against the CPU render the export used before, over the
//! demo project, and an export through it. Skipped without a GPU adapter.

use std::sync::Arc;

use filmcraft_engine::Session;
use filmcraft_render::{FrameRenderer, RenderOptions};
use filmcraft_time::Tick;
use serde_json::json;

fn device() -> Option<(eframe::wgpu::Device, eframe::wgpu::Queue)> {
    let instance = eframe::wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&eframe::wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&eframe::wgpu::DeviceDescriptor::default())).ok()
}

/// PSNR (dB) of two RGBA8 pictures, colour channels only.
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let (mut se, mut n) = (0f64, 0f64);
    for (p, q) in a.chunks(4).zip(b.chunks(4)) {
        for k in 0..3 {
            se += (p[k] as f64 - q[k] as f64).powi(2);
            n += 1.0;
        }
    }
    if se == 0.0 { 99.0 } else { 10.0 * (255.0f64.powi(2) * n / se).log10() }
}

#[test]
fn gpu_frames_match_the_cpu_render() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let gpu = filmcraft_gpu::GpuFrameRenderer::new(&dev, &q).expect("the effect stage builds");
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let seq = s.state.active_sequence.unwrap();
    let q = s.project.sequence(seq).unwrap();
    let (dur, rate) = (q.duration(), q.settings.frame_rate);
    let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
    let opts = RenderOptions { scale: 0.5, ..Default::default() };
    for k in 0..8 {
        let t = rate.snap_nearest(Tick(dur.0 * (2 * k + 1) / 16));
        let cpu = filmcraft_render::render_sequence(&s.project, seq, t, opts, &provider).to_rgba8();
        let g = gpu.render(&s.project, seq, t, opts, &provider).to_rgba8();
        assert_eq!(cpu.len(), g.len());
        let db = psnr(&cpu, &g);
        eprintln!("{:.2} s: {db:.1} dB", t.seconds());
        assert!(db >= 38.0, "{:.2} s: GPU frame {db:.1} dB from the CPU render", t.seconds());
    }
}

#[test]
fn exports_go_through_the_session_renderer_unless_software_only() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    assert!(filmcraft_engine::export_tools::export_renderer(&s).is_none(), "no renderer attached: CPU");
    s.frame_renderer = filmcraft_gpu::GpuFrameRenderer::new(&dev, &q).map(|r| Arc::new(r) as Arc<dyn FrameRenderer>);
    assert!(filmcraft_engine::export_tools::export_renderer(&s).is_some());
    s.execute("file.projectSettings.general", json!({"renderer": "software"})).unwrap();
    assert!(filmcraft_engine::export_tools::export_renderer(&s).is_none(), "Software Only keeps exports on the CPU");
}
