//! Shared-array-buffer camera contract.
//!
//! The camera transform (`scale`, `tx`, `ty`) is written by the main
//! thread on every input event and read by the worker at the start
//! of every render frame. To avoid the `postMessage` round-trip
//! (which serialises through the message queue and costs at least a
//! frame), the value lives in a `SharedArrayBuffer` with a known
//! fixed layout.
//!
//! Layout (little-endian, 32-byte buffer):
//!
//! ```text
//! +-----+-----+-----+-----+
//! | 0   | 4   | 8   | 12  |   scale  tx  ty  unused
//! +-----+-----+-----+-----+
//! | 16  | 20  | 24  | 28  |   generation_lo  generation_hi  unused  unused
//! +-----+-----+-----+-----+
//! ```
//!
//! Reader/writer protocol:
//!
//! 1. Writer (main thread): `Atomics.store` for each f32 field, then
//!    `Atomics.add` the generation counter (read-modify-write, lock-
//!    free) to mark the write complete.
//! 2. Reader (worker): `Atomics.load` the generation counter at the
//!    start of each frame; if it changed since the last frame, read
//!    `scale`, `tx`, `ty` and use the new value.
//!
//! Race window: if the writer is interrupted between updating fields
//! and bumping the generation, the reader can read a half-written
//! value. The spec accepts this as a single-frame visual glitch
//! (canvas §5.1) — the next frame reads consistent state. For values
//! where torn reads would corrupt behaviour (selection, mutation),
//! the typed channel is used instead.

/// Total bytes the camera SAB occupies. The writer (JS) allocates
/// `new SharedArrayBuffer(CAMERA_SAB_BYTES)`; the reader maps the
/// same buffer via `Float32Array(buf, 0, 3)` for the transform and
/// `Uint32Array(buf, 16, 2)` for the 64-bit generation counter.
pub const CAMERA_SAB_BYTES: usize = 32;

pub const OFFSET_SCALE: usize = 0;
pub const OFFSET_TX: usize = 4;
pub const OFFSET_TY: usize = 8;
pub const OFFSET_GEN_LO: usize = 16;
pub const OFFSET_GEN_HI: usize = 20;

/// Canonical camera transform: document space → viewport space.
/// `scale` is pixels-per-pt; `tx`, `ty` are the viewport-space
/// position (in CSS pixels) of the document origin (0, 0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub scale: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Camera {
    pub const IDENTITY: Self = Self {
        scale: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// Transform a document-space point into viewport coordinates.
    pub fn to_viewport(&self, doc: (f32, f32)) -> (f32, f32) {
        (doc.0 * self.scale + self.tx, doc.1 * self.scale + self.ty)
    }

    /// Inverse transform: viewport pixel → document point. The
    /// canvas's hit-test path runs through this on every pointer
    /// event so it must be branch-free + allocation-free.
    pub fn to_document(&self, view: (f32, f32)) -> (f32, f32) {
        let inv = 1.0 / self.scale;
        ((view.0 - self.tx) * inv, (view.1 - self.ty) * inv)
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Headless helper for tests and the non-SAB fallback (when
/// `SharedArrayBuffer` isn't available — e.g. cross-origin isolation
/// not configured). Stores the camera in plain bytes; the wasm side
/// wraps a real SAB but presents the same `read` / `write` shape.
#[derive(Debug, Default)]
pub struct CameraLayout {
    bytes: [u8; CAMERA_SAB_BYTES],
}

impl CameraLayout {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn raw(&self) -> &[u8; CAMERA_SAB_BYTES] {
        &self.bytes
    }

    pub fn raw_mut(&mut self) -> &mut [u8; CAMERA_SAB_BYTES] {
        &mut self.bytes
    }

    /// Snapshot the current camera. Reads the generation counter,
    /// then the fields. Callers that need true atomicity across
    /// threads use `Atomics.load` on the JS side; the Rust-side
    /// helper is for tests + the single-threaded fallback.
    pub fn read(&self) -> Camera {
        Camera {
            scale: f32::from_le_bytes(slice4(&self.bytes, OFFSET_SCALE)),
            tx: f32::from_le_bytes(slice4(&self.bytes, OFFSET_TX)),
            ty: f32::from_le_bytes(slice4(&self.bytes, OFFSET_TY)),
        }
    }

    /// Write the camera and bump the generation counter. The
    /// generation is 64-bit so a tight write loop never wraps within
    /// a session (2^64 frames is geological).
    pub fn write(&mut self, cam: Camera) {
        let cur = self.generation();
        write_f32(&mut self.bytes, OFFSET_SCALE, cam.scale);
        write_f32(&mut self.bytes, OFFSET_TX, cam.tx);
        write_f32(&mut self.bytes, OFFSET_TY, cam.ty);
        // Bump only after the field writes — matches the SAB
        // protocol the worker reads.
        self.set_generation(cur.wrapping_add(1));
    }

    pub fn generation(&self) -> u64 {
        let lo = u32::from_le_bytes(slice4(&self.bytes, OFFSET_GEN_LO));
        let hi = u32::from_le_bytes(slice4(&self.bytes, OFFSET_GEN_HI));
        (u64::from(hi) << 32) | u64::from(lo)
    }

    fn set_generation(&mut self, gen: u64) {
        let lo = (gen & 0xFFFF_FFFF) as u32;
        let hi = (gen >> 32) as u32;
        write_u32(&mut self.bytes, OFFSET_GEN_LO, lo);
        write_u32(&mut self.bytes, OFFSET_GEN_HI, hi);
    }
}

fn slice4(buf: &[u8; CAMERA_SAB_BYTES], offset: usize) -> [u8; 4] {
    let mut out = [0u8; 4];
    out.copy_from_slice(&buf[offset..offset + 4]);
    out
}

fn write_f32(buf: &mut [u8; CAMERA_SAB_BYTES], offset: usize, v: f32) {
    buf[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
}

fn write_u32(buf: &mut [u8; CAMERA_SAB_BYTES], offset: usize, v: u32) {
    buf[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sab_layout_constants_match_spec() {
        // Locked-down field offsets — TS code in apps/canvas/ hardcodes
        // the same values via the camera contract types. Any drift
        // here is a wire-format break.
        assert_eq!(OFFSET_SCALE, 0);
        assert_eq!(OFFSET_TX, 4);
        assert_eq!(OFFSET_TY, 8);
        assert_eq!(OFFSET_GEN_LO, 16);
        assert_eq!(OFFSET_GEN_HI, 20);
        assert_eq!(CAMERA_SAB_BYTES, 32);
    }

    #[test]
    fn write_then_read_preserves_camera() {
        let mut layout = CameraLayout::new();
        let cam = Camera {
            scale: 1.5,
            tx: 100.0,
            ty: -50.25,
        };
        layout.write(cam);
        let back = layout.read();
        assert_eq!(back.scale, 1.5);
        assert_eq!(back.tx, 100.0);
        assert_eq!(back.ty, -50.25);
    }

    #[test]
    fn writing_bumps_generation() {
        let mut layout = CameraLayout::new();
        assert_eq!(layout.generation(), 0);
        layout.write(Camera::IDENTITY);
        assert_eq!(layout.generation(), 1);
        layout.write(Camera::IDENTITY);
        assert_eq!(layout.generation(), 2);
    }

    #[test]
    fn to_viewport_and_back_is_identity() {
        let cam = Camera {
            scale: 2.0,
            tx: 10.0,
            ty: 20.0,
        };
        let doc = (5.0, 7.0);
        let view = cam.to_viewport(doc);
        let back = cam.to_document(view);
        assert!((back.0 - doc.0).abs() < 1e-6);
        assert!((back.1 - doc.1).abs() < 1e-6);
    }

    #[test]
    fn identity_camera_is_no_op() {
        let cam = Camera::IDENTITY;
        assert_eq!(cam.to_viewport((42.0, -3.0)), (42.0, -3.0));
        assert_eq!(cam.to_document((42.0, -3.0)), (42.0, -3.0));
    }
}
