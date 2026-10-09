//! VideoToolbox (macOS) hardware H.264 encoding for exports (Encoding Settings ▸ Performance ▸
//! Hardware Encoding).
//!
//! Same FFI rules as [`crate::videotoolbox`]: every `unsafe` block has a `// SAFETY:` comment, no
//! panic unwinds into VideoToolbox (the output callback runs under `catch_unwind`), and every
//! failure is a `Result`. A session that cannot be created (no hardware encoder, a size it
//! declines) makes [`factory`] answer `None`, and the export uses our software encoder.
//!
//! Pictures are converted to BT.709 limited-range 4:2:0 by the export crate's converter (the same
//! numbers the software encoder gets), written into `420v` (NV12) buffers from the session's pool
//! and encoded asynchronously, a few in flight. Frame reordering is off (no B-frames): packets
//! come out in presentation order, so samples need no composition offsets. The output is
//! length-prefixed (`avcC`) H.264 whose parameter sets come from the first packet's format
//! description.

use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::ptr::NonNull;
use std::sync::{Mutex, PoisonError};

use filmcraft_export::{
    BitrateMode, ColorSignal, EncodedPacket, EncoderFrame, Encoding, ExportError, ExportSettings, Format, H264Pass, H264Profile, VideoEncoder,
};
use filmcraft_isobmff::{AvcConfig, SampleEntry};
use filmcraft_time::FrameRate;
use objc2_core_foundation::{CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags, CMVideoFormatDescriptionGetH264ParameterSetAtIndex, kCMSampleAttachmentKey_NotSync};
use objc2_core_video::{
    CVImageBuffer, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeightOfPlane, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferPool, CVPixelBufferUnlockBaseAddress, kCVImageBufferColorPrimaries_ITU_R_709_2,
    kCVImageBufferTransferFunction_ITU_R_709_2, kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVPixelBufferHeightKey, kCVPixelBufferIOSurfacePropertiesKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
};
use objc2_video_toolbox::{
    VTCompressionSession, VTEncodeInfoFlags, VTSessionSetProperty, kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ColorPrimaries, kVTCompressionPropertyKey_ConstantBitRate, kVTCompressionPropertyKey_DataRateLimits,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_H264EntropyMode, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime, kVTCompressionPropertyKey_TransferFunction,
    kVTCompressionPropertyKey_YCbCrMatrix, kVTH264EntropyMode_CABAC, kVTProfileLevel_H264_Baseline_AutoLevel, kVTProfileLevel_H264_High_AutoLevel,
    kVTProfileLevel_H264_Main_AutoLevel, kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder,
};

/// `CVPixelBuffer` format of the pictures we hand VideoToolbox: biplanar 8-bit 4:2:0, video range.
const NV12_VIDEO: u32 = u32::from_be_bytes(*b"420v");
/// `CMVideoCodecType` of H.264.
const AVC1: u32 = u32::from_be_bytes(*b"avc1");
/// Pictures submitted and not yet emitted before `encode` waits for the oldest ones.
const MAX_IN_FLIGHT: u64 = 8;

/// The export crate's encoder factory for VideoToolbox H.264 (registered by [`crate::register`]).
/// `None` (our encoder takes over) for other formats, Software Encoding, settings the hardware
/// path doesn't honour (VBR 2 pass, HDR, MXF's Annex B stream, non-square pixels, odd sizes) and
/// when no hardware session can be created.
pub fn factory(format: Format, w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Option<filmcraft_export::Result<Box<dyn VideoEncoder>>> {
    if format != Format::H264
        || s.encoding != Encoding::Hardware
        || s.format.is_mxf()
        || s.bitrate_mode == BitrateMode::Vbr2Pass
        || !matches!(s.h264_pass, H264Pass::Single)
        || s.signal.is_hdr()
        || s.pixel_aspect.is_some_and(|(n, d)| n != d)
        || !w.is_multiple_of(2)
        || !h.is_multiple_of(2)
        || w < 64
        || h < 64
    {
        return None;
    }
    match VtEncoder::new(w, h, rate, s) {
        Ok(e) => Some(Ok(Box::new(e))),
        Err(e) => {
            log::warn!("VideoToolbox H.264 encoder unavailable ({e}); using the software encoder");
            None
        }
    }
}

/// What the output callback collected.
#[derive(Default)]
struct Out {
    packets: Vec<EncodedPacket>,
    /// SPS, PPS and NAL length size, from the first packet's format description.
    params: Option<(Vec<u8>, Vec<u8>, u8)>,
    emitted: u64,
    error: Option<String>,
}

/// State shared with the output callback (its refcon). Boxed by the encoder and kept alive until
/// the session is invalidated.
struct Shared {
    duration: u32,
    out: Mutex<Out>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Out> {
        self.out.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The output callback: copies each encoded sample out. Never unwinds.
unsafe extern "C-unwind" fn output_callback(
    refcon: *mut c_void,
    _frame_refcon: *mut c_void,
    status: i32,
    flags: VTEncodeInfoFlags,
    sample: *mut CMSampleBuffer,
) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: `refcon` is the `Shared` the session was created with; it lives in a `Box` owned by
    // the encoder, which invalidates the session (no more callbacks) before freeing it.
    let shared = unsafe { &*(refcon as *const Shared) };
    let r = std::panic::catch_unwind(AssertUnwindSafe(|| -> Result<(EncodedPacket, Option<(Vec<u8>, Vec<u8>, u8)>), String> {
        if status != 0 {
            return Err(format!("VideoToolbox encode error {status}"));
        }
        if flags.contains(VTEncodeInfoFlags::FrameDropped) {
            return Err("VideoToolbox dropped a frame".into());
        }
        let sample = NonNull::new(sample).ok_or("VideoToolbox returned no sample")?;
        // SAFETY: VideoToolbox passes a valid sample buffer for the duration of the callback; we
        // only borrow it.
        let sample = unsafe { sample.as_ref() };
        read_sample(sample, shared.duration)
    }));
    let mut out = shared.lock();
    out.emitted += 1;
    match r {
        Ok(Ok((packet, params))) => {
            if out.params.is_none() {
                out.params = params;
            }
            out.packets.push(packet);
        }
        Ok(Err(e)) => {
            out.error.get_or_insert(e);
        }
        Err(_) => {
            out.error.get_or_insert_with(|| "panic while reading an encoded sample".into());
        }
    }
}

/// One encoded sample: its bytes, whether it is a sync sample, and (first sample) the parameter
/// sets of its format description.
fn read_sample(sample: &CMSampleBuffer, duration: u32) -> Result<(EncodedPacket, Option<(Vec<u8>, Vec<u8>, u8)>), String> {
    // SAFETY: getters on a valid sample buffer; the block buffer is returned retained.
    let block = unsafe { sample.data_buffer() }.ok_or("encoded sample without data")?;
    // SAFETY: a getter on a valid block buffer.
    let len = unsafe { block.data_length() };
    let mut data = vec![0u8; len];
    if len > 0 {
        let dst = NonNull::new(data.as_mut_ptr() as *mut c_void).ok_or("no buffer")?;
        // SAFETY: copies `len` bytes (the block's whole length) into `data`, which holds `len` bytes.
        let st = unsafe { block.copy_data_bytes(0, len, dst) };
        if st != 0 {
            return Err(format!("CMBlockBufferCopyDataBytes failed ({st})"));
        }
    }
    // SAFETY: a getter on a valid sample buffer (no attachments array is created).
    let key = match unsafe { sample.sample_attachments_array(false) } {
        Some(arr) => !not_sync(&arr),
        None => true,
    };
    // SAFETY: a getter on a valid sample buffer; the description is returned retained.
    let params = unsafe { sample.format_description() }.and_then(|fd| {
        let mut sets = Vec::new();
        let mut nal_len: i32 = 4;
        for i in 0..2 {
            let mut ptr: *const u8 = std::ptr::null();
            let mut size: usize = 0;
            // SAFETY: `fd` is a valid H.264 format description held for the call; the returned
            // pointer points into it and is copied out before `fd` is released.
            let st = unsafe { CMVideoFormatDescriptionGetH264ParameterSetAtIndex(&fd, i, &mut ptr, &mut size, std::ptr::null_mut(), &mut nal_len) };
            if st != 0 || ptr.is_null() || size == 0 {
                return None;
            }
            // SAFETY: CoreMedia returned `size` readable bytes at `ptr` (see above).
            sets.push(unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec());
        }
        let pps = sets.pop()?;
        let sps = sets.pop()?;
        Some((sps, pps, nal_len.clamp(1, 4) as u8))
    });
    Ok((EncodedPacket { data, key, duration, composition_offset: 0 }, params))
}

/// Whether the first sample's attachments say "not a sync sample".
fn not_sync(arr: &CFArray) -> bool {
    if arr.count() < 1 {
        return false;
    }
    // SAFETY: index 0 exists (count ≥ 1); the array holds CFDictionary values owned by the sample
    // buffer, which outlives this borrow.
    let dict = unsafe { arr.value_at_index(0) } as *const CFDictionary;
    let Some(dict) = NonNull::new(dict as *mut CFDictionary) else {
        return false;
    };
    // SAFETY: a valid dictionary (see above); reading an immutable framework constant as the key.
    let v = unsafe { dict.as_ref().value(kCMSampleAttachmentKey_NotSync as *const CFString as *const c_void) } as *const CFBoolean;
    // SAFETY: a value stored under NotSync is a CFBoolean owned by the dictionary.
    NonNull::new(v as *mut CFBoolean).is_some_and(|b| unsafe { b.as_ref() }.as_bool())
}

/// A compression session with its callback state. Dropping it invalidates the session before the
/// callback state is freed.
struct Session {
    session: CFRetained<VTCompressionSession>,
    pool: CFRetained<CVPixelBufferPool>,
    /// Pointed to by the session's refcon: must outlive `session`.
    shared: Box<Shared>,
}

// SAFETY: a VTCompressionSession and its pixel buffer pool may be used from any thread (Apple:
// "thread safe"); the encoder owning a `Session` is used by one thread at a time (`&mut self`),
// and the callback state is behind a `Mutex`.
unsafe impl Send for Session {}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the session is valid until released; completing and invalidating it guarantees
        // no callback runs afterwards, so `shared` can be freed when this function returns.
        unsafe {
            self.session.complete_frames(INVALID);
            self.session.invalidate();
        }
    }
}

const INVALID: CMTime = CMTime { value: 0, timescale: 0, flags: CMTimeFlags(0), epoch: 0 };

fn set(session: &VTCompressionSession, key: &CFString, value: &CFType) -> Result<(), String> {
    // SAFETY: a valid session, key and value; VideoToolbox retains what it keeps.
    let st = unsafe { VTSessionSetProperty(session.as_ref(), key, Some(value)) };
    if st != 0 { Err(format!("VTSessionSetProperty({key}) failed ({st})")) } else { Ok(()) }
}

/// VideoToolbox H.264 for one export.
pub struct VtEncoder {
    session: Session,
    w: u32,
    h: u32,
    rate: FrameRate,
    signal: ColorSignal,
    submitted: u64,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl VtEncoder {
    pub fn new(w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Result<VtEncoder, String> {
        // SAFETY: reading immutable framework constants.
        let (req, pf_key, w_key, h_key, io_key) = unsafe {
            (
                kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder,
                kCVPixelBufferPixelFormatTypeKey,
                kCVPixelBufferWidthKey,
                kCVPixelBufferHeightKey,
                kCVPixelBufferIOSurfacePropertiesKey,
            )
        };
        let spec = CFDictionary::<CFString, CFType>::from_slices(&[req], &[CFBoolean::new(true).as_ref()]);
        let (pf, wn, hn) = (CFNumber::new_i32(NV12_VIDEO as i32), CFNumber::new_i32(w as i32), CFNumber::new_i32(h as i32));
        let io = CFDictionary::<CFString, CFType>::empty();
        let attrs = CFDictionary::<CFString, CFType>::from_slices(&[pf_key, w_key, h_key, io_key], &[pf.as_ref(), wn.as_ref(), hn.as_ref(), io.as_ref()]);
        let shared = Box::new(Shared { duration: rate.den as u32, out: Mutex::new(Out::default()) });
        let mut out: *mut VTCompressionSession = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call; the refcon points into `shared`, which the
        // returned `Session` keeps alive until the session is invalidated.
        let status = unsafe {
            VTCompressionSession::create(
                None,
                w as i32,
                h as i32,
                AVC1,
                Some(spec.as_ref()),
                Some(attrs.as_ref()),
                None,
                Some(output_callback),
                &*shared as *const Shared as *mut c_void,
                NonNull::from(&mut out),
            )
        };
        let session = NonNull::new(out).filter(|_| status == 0).ok_or_else(|| format!("VTCompressionSessionCreate failed ({status})"))?;
        // SAFETY: a created session is returned retained (+1); `CFRetained` takes that reference over.
        let session = unsafe { CFRetained::from_raw(session) };
        configure(&session, rate, s).inspect_err(|_| {
            // SAFETY: the session is valid; nothing was submitted.
            unsafe { session.invalidate() };
        })?;
        // SAFETY: the session is valid and configured.
        let st = unsafe { session.prepare_to_encode_frames() };
        // SAFETY: a getter on a valid session; the pool is returned retained.
        let pool = unsafe { session.pixel_buffer_pool() };
        let Some(pool) = pool.filter(|_| st == 0) else {
            // SAFETY: the session is valid; nothing was submitted.
            unsafe { session.invalidate() };
            return Err(format!("VideoToolbox could not prepare the encoder ({st})"));
        };
        Ok(VtEncoder { session: Session { session, pool, shared }, w, h, rate, signal: s.signal, submitted: 0, y: Vec::new(), u: Vec::new(), v: Vec::new() })
    }

    fn take(&self) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        let mut out = self.session.shared.lock();
        if let Some(e) = out.error.take() {
            return Err(ExportError::Encode(e));
        }
        Ok(std::mem::take(&mut out.packets))
    }

    /// A pool buffer holding the current Y / U / V planes as NV12.
    fn picture(&self) -> Result<CFRetained<CVImageBuffer>, String> {
        let mut pb: *mut CVImageBuffer = std::ptr::null_mut();
        // SAFETY: a valid pool; the buffer is returned retained (+1).
        let st = unsafe { CVPixelBufferPool::create_pixel_buffer(None, &self.session.pool, NonNull::from(&mut pb)) };
        let pb = NonNull::new(pb).filter(|_| st == 0).ok_or_else(|| format!("CVPixelBufferPoolCreatePixelBuffer failed ({st})"))?;
        // SAFETY: created retained (+1).
        let pb = unsafe { CFRetained::from_raw(pb) };
        // SAFETY: a valid pixel buffer, locked for writing until the matching unlock below.
        let st = unsafe { CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags(0)) };
        if st != 0 {
            return Err(format!("CVPixelBufferLockBaseAddress failed ({st})"));
        }
        let r = self.fill(&pb);
        // SAFETY: locked above with the same flags.
        unsafe { CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags(0)) };
        r.map(|_| pb)
    }

    fn fill(&self, pb: &CVImageBuffer) -> Result<(), String> {
        let (w, h) = (self.w as usize, self.h as usize);
        let cw = w / 2;
        for (i, rows) in [(0usize, h), (1, h / 2)] {
            let base = CVPixelBufferGetBaseAddressOfPlane(pb, i) as *mut u8;
            let stride = CVPixelBufferGetBytesPerRowOfPlane(pb, i);
            if base.is_null() || stride < w || CVPixelBufferGetHeightOfPlane(pb, i) < rows {
                return Err(format!("pixel buffer plane {i} is not mapped"));
            }
            // SAFETY: the base address is locked by the caller; CoreVideo maps `stride * rows`
            // bytes from each plane's base address (checked above), and nothing else aliases it.
            let plane = unsafe { std::slice::from_raw_parts_mut(base, stride * rows) };
            for (r, dst) in plane.chunks_exact_mut(stride).enumerate() {
                if i == 0 {
                    dst[..w].copy_from_slice(&self.y[r * w..(r + 1) * w]);
                } else {
                    let (u, v) = (&self.u[r * cw..(r + 1) * cw], &self.v[r * cw..(r + 1) * cw]);
                    for (k, px) in dst[..w].as_chunks_mut::<2>().0.iter_mut().enumerate() {
                        *px = [u[k], v[k]];
                    }
                }
            }
        }
        Ok(())
    }

    fn time(&self, frame: i64) -> CMTime {
        CMTime { value: frame * self.rate.den, timescale: self.rate.num as i32, flags: CMTimeFlags::Valid, epoch: 0 }
    }
}

/// Session properties from the export settings.
fn configure(session: &VTCompressionSession, rate: FrameRate, s: &ExportSettings) -> Result<(), String> {
    // SAFETY: reading immutable framework constants.
    let k = unsafe {
        [
            kVTCompressionPropertyKey_RealTime,
            kVTCompressionPropertyKey_ProfileLevel,
            kVTCompressionPropertyKey_AllowFrameReordering,
            kVTCompressionPropertyKey_MaxKeyFrameInterval,
            kVTCompressionPropertyKey_ExpectedFrameRate,
            kVTCompressionPropertyKey_ColorPrimaries,
            kVTCompressionPropertyKey_TransferFunction,
            kVTCompressionPropertyKey_YCbCrMatrix,
        ]
    };
    // SAFETY: reading immutable framework constants.
    let (profile, c709) = unsafe {
        (
            match s.h264_profile {
                H264Profile::Baseline => kVTProfileLevel_H264_Baseline_AutoLevel,
                H264Profile::Main => kVTProfileLevel_H264_Main_AutoLevel,
                H264Profile::High => kVTProfileLevel_H264_High_AutoLevel,
            },
            [kCVImageBufferColorPrimaries_ITU_R_709_2, kCVImageBufferTransferFunction_ITU_R_709_2, kCVImageBufferYCbCrMatrix_ITU_R_709_2],
        )
    };
    let fps = rate.num as f64 / rate.den as f64;
    let keyint = s.keyframe_distance.filter(|k| *k > 0).unwrap_or_else(|| (fps * 2.0).round().max(1.0) as u32);
    set(session, k[0], CFBoolean::new(false).as_ref())?;
    set(session, k[1], profile.as_ref())?;
    set(session, k[2], CFBoolean::new(false).as_ref())?;
    set(session, k[3], CFNumber::new_i32(keyint as i32).as_ref())?;
    set(session, k[4], CFNumber::new_f64(fps).as_ref())?;
    for (key, v) in k[5..].iter().zip(c709) {
        set(session, key, v.as_ref())?;
    }
    if s.h264_profile != H264Profile::Baseline {
        // SAFETY: reading immutable framework constants.
        let (key, cabac) = unsafe { (kVTCompressionPropertyKey_H264EntropyMode, kVTH264EntropyMode_CABAC) };
        set(session, key, cabac.as_ref())?;
    }
    let kbps = s.bitrate_kbps.max(100) as i64;
    let bps = kbps * 1000;
    // SAFETY: reading immutable framework constants.
    let (cbr, avg, limits) =
        unsafe { (kVTCompressionPropertyKey_ConstantBitRate, kVTCompressionPropertyKey_AverageBitRate, kVTCompressionPropertyKey_DataRateLimits) };
    if s.bitrate_mode == BitrateMode::Cbr && set(session, cbr, CFNumber::new_i64(bps).as_ref()).is_ok() {
        return Ok(());
    }
    set(session, avg, CFNumber::new_i64(bps).as_ref())?;
    // the peak: bytes per one-second window (what the software encoder's VBV maximum is)
    let max = s.max_bitrate_kbps.filter(|m| *m as i64 >= kbps).map(|m| m as i64).unwrap_or(kbps * 3 / 2);
    let (bytes, secs) = (CFNumber::new_i64(max * 1000 / 8), CFNumber::new_f64(1.0));
    let arr = CFArray::<CFNumber>::from_retained_objects(&[bytes, secs]);
    // a hardware encoder that rejects the limit still honours the average
    if let Err(e) = set(session, limits, arr.as_ref()) {
        log::debug!("{e}");
    }
    Ok(())
}

impl VideoEncoder for VtEncoder {
    fn sample_entry(&self) -> SampleEntry {
        if self.submitted > 0 && self.session.shared.lock().params.is_none() {
            // the container is opened before the first packet came back (a short export): wait
            // for the pictures in flight, whose packets the next `encode` / `flush` hands over
            // SAFETY: a valid session; an invalid time completes every submitted picture.
            unsafe { self.session.session.complete_frames(INVALID) };
        }
        let (sps, pps, len) = self.session.shared.lock().params.clone().unwrap_or_default();
        let mut e = SampleEntry::avc(AvcConfig::new(vec![sps], vec![pps], len.max(1)), self.w as u16, self.h as u16);
        self.signal.apply_to(&mut e, false);
        e
    }

    fn timescale(&self) -> u32 {
        self.rate.num as u32
    }

    fn encode(&mut self, f: &EncoderFrame) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        if f.hdr.is_some() || f.width != self.w || f.height != self.h {
            return Err(ExportError::Encode("the hardware encoder got a picture it was not set up for".into()));
        }
        filmcraft_export::rgba_to_yuv420_8(f.rgba, f.width as usize, f.height as usize, &mut self.y, &mut self.u, &mut self.v);
        let pb = self.picture().map_err(ExportError::Encode)?;
        let mut info = VTEncodeInfoFlags(0);
        let index = f.index as i64;
        // SAFETY: a valid session and pixel buffer (VideoToolbox retains the buffer while it needs
        // it); no frame properties; the frame refcon is unused.
        let st = unsafe { self.session.session.encode_frame(&pb, self.time(index), self.time(1), None, std::ptr::null_mut(), &mut info) };
        if st != 0 {
            return Err(ExportError::Encode(format!("VTCompressionSessionEncodeFrame failed ({st})")));
        }
        self.submitted += 1;
        let emitted = self.session.shared.lock().emitted;
        if self.submitted - emitted > MAX_IN_FLIGHT {
            // SAFETY: a valid session; blocks until the pictures up to that time are emitted.
            unsafe { self.session.session.complete_frames(self.time(index - MAX_IN_FLIGHT as i64 / 2)) };
        }
        self.take()
    }

    fn flush(&mut self) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        // SAFETY: a valid session; an invalid time completes every submitted picture.
        let st = unsafe { self.session.session.complete_frames(INVALID) };
        if st != 0 {
            return Err(ExportError::Encode(format!("VTCompressionSessionCompleteFrames failed ({st})")));
        }
        let out = self.take()?;
        if self.session.shared.lock().params.is_none() && self.submitted > 0 {
            return Err(ExportError::Encode("VideoToolbox gave no H.264 parameter sets".into()));
        }
        Ok(out)
    }
}
