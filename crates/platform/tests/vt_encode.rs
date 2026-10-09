//! VideoToolbox H.264 encoding (macOS): a moving test picture encoded by the hardware encoder and
//! decoded by our software decoder must come back close to its source (PSNR of the luma plane),
//! with a sync sample first, key frames at the set distance, an `avcC` that our decoder accepts —
//! also when the container is opened before the first packet came back — and the factory must
//! decline the settings the hardware path doesn't honour. Skips without a hardware encoder.
#![cfg(target_os = "macos")]

use filmcraft_export::{BitrateMode, EncoderFrame, Encoding, ExportSettings, Format, VideoEncoder};
use filmcraft_frame::PixelData;
use filmcraft_time::FrameRate;

const W: u32 = 640;
const H: u32 = 360;

/// A moving gradient with detail (circles), frame `i`, straight sRGB RGBA8.
fn picture(i: u32) -> Vec<u8> {
    let mut px = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let (fx, fy) = ((x + i * 4) as f32, y as f32);
            let r = (fx / W as f32 * 255.0) as u8;
            let g = (fy / H as f32 * 255.0) as u8;
            let d = ((fx - 320.0).powi(2) + (fy - 180.0).powi(2)).sqrt();
            let b = if (d as u32 / 12).is_multiple_of(2) { 220 } else { 40 };
            let o = ((y * W + x) * 4) as usize;
            px[o..o + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    px
}

fn settings() -> ExportSettings {
    ExportSettings { format: Format::H264, bitrate_kbps: 12_000, keyframe_distance: Some(30), encoding: Encoding::Hardware, ..Default::default() }
}

fn hardware(s: &ExportSettings) -> Option<Box<dyn VideoEncoder>> {
    match filmcraft_platform::vt_encode::factory(Format::H264, W, H, FrameRate::FPS_30, s) {
        Some(Ok(e)) => Some(e),
        Some(Err(e)) => panic!("factory error: {e}"),
        None => {
            eprintln!("SKIPPED: no VideoToolbox H.264 encoder");
            None
        }
    }
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let se: f64 = a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum();
    if se == 0.0 { 99.0 } else { 10.0 * (255.0f64.powi(2) * a.len() as f64 / se).log10() }
}

#[test]
fn hardware_h264_decodes_close_to_its_source() {
    let Some(mut enc) = hardware(&settings()) else { return };
    let n = 60u32;
    let mut packets = Vec::new();
    let mut sources = Vec::new();
    for i in 0..n {
        let rgba = picture(i);
        let (mut y, mut u, mut v) = (Vec::new(), Vec::new(), Vec::new());
        filmcraft_export::rgba_to_yuv420_8(&rgba, W as usize, H as usize, &mut y, &mut u, &mut v);
        sources.push(y);
        packets.extend(enc.encode(&EncoderFrame { width: W, height: H, rgba: &rgba, hdr: None, index: i as u64 }).unwrap());
    }
    packets.extend(enc.flush().unwrap());
    assert_eq!(packets.len(), n as usize, "one packet per picture");
    assert!(packets[0].key, "the first sample is a sync sample");
    assert!(packets[1..=30].iter().any(|p| p.key), "a key frame within the key frame distance");
    assert!(packets.iter().all(|p| p.composition_offset == 0 && p.duration == FrameRate::FPS_30.den as u32), "no reordering");

    let entry = enc.sample_entry();
    let mut dec = filmcraft_codecs::software_video_decoder(&entry).expect("our decoder takes the avcC");
    let mut frames = Vec::new();
    for (i, p) in packets.iter().enumerate() {
        frames.extend(dec.decode(&p.data, i as i64).unwrap());
    }
    frames.extend(dec.flush());
    assert_eq!(frames.len(), n as usize);
    let mut worst = 99.0f64;
    for f in &frames {
        let PixelData::Yuv8 { planes, .. } = &f.frame.data else { panic!("8-bit 4:2:0 expected") };
        worst = worst.min(psnr(&planes[0], &sources[f.pts as usize]));
    }
    eprintln!("hardware H.264 at 12 Mbps: worst luma PSNR {worst:.1} dB");
    assert!(worst >= 38.0, "worst luma PSNR {worst:.1} dB");
    let bytes: usize = packets.iter().map(|p| p.data.len()).sum();
    let mbps = bytes as f64 * 8.0 / (n as f64 / 30.0) / 1e6;
    assert!((2.0..=24.0).contains(&mbps), "bitrate {mbps:.1} Mbps for a 12 Mbps target");
}

#[test]
fn the_sample_entry_is_complete_before_flush() {
    // a short export opens the container before the encoder handed back any packet
    let Some(mut enc) = hardware(&settings()) else { return };
    let mut early = Vec::new();
    for i in 0..3 {
        early.extend(enc.encode(&EncoderFrame { width: W, height: H, rgba: &picture(i), hdr: None, index: i as u64 }).unwrap());
    }
    let entry = enc.sample_entry();
    assert!(filmcraft_codecs::software_video_decoder(&entry).is_ok(), "avcC with parameter sets");
    early.extend(enc.flush().unwrap());
    assert_eq!(early.len(), 3);
}

#[test]
fn the_factory_declines_what_hardware_encoding_does_not_honour() {
    let f = |s: &ExportSettings, w: u32, h: u32| filmcraft_platform::vt_encode::factory(Format::H264, w, h, FrameRate::FPS_30, s).is_some();
    let sw = ExportSettings { encoding: Encoding::Software, ..settings() };
    assert!(!f(&sw, W, H), "Software Encoding");
    let two = ExportSettings { bitrate_mode: BitrateMode::Vbr2Pass, ..settings() };
    assert!(!f(&two, W, H), "VBR 2 pass");
    let mxf = ExportSettings { format: Format::MxfOp1a, ..settings() };
    assert!(!f(&mxf, W, H), "MXF (Annex B)");
    let par = ExportSettings { pixel_aspect: Some((4, 3)), ..settings() };
    assert!(!f(&par, W, H), "non-square pixels");
    assert!(!f(&settings(), W + 1, H), "odd width");
    assert!(filmcraft_platform::vt_encode::factory(Format::ProRes, W, H, FrameRate::FPS_30, &settings()).is_none(), "other formats");
}
