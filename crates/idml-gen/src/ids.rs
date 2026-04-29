//! Deterministic `Self`-id generation.
//!
//! Real InDesign exports use IDs of the shape `u<hex>` where `<hex>` is
//! an internal counter. We mimic the shape (so generated samples sit
//! comfortably alongside hand-curated ones) but derive the value from a
//! BLAKE3 hash of `(sample, path, seq)` — same inputs always yield the
//! same id, and the `path` segment lets two different element kinds at
//! the same `seq` collide-free.
//!
//! 6 hex characters = 24 bits ≈ 16 M keyspace per (sample, path)
//! combo. A document with a few thousand items is comfortably below
//! the birthday-collision threshold; if a real generator ever pushes
//! past that, lift the slice length.

/// Compute a stable id of the form `u<hex6>` for the given coordinates.
///
/// `sample` — the mega-file name (`"geometry"`, `"text"`, ...).
/// `path` — the element class (`"Spread"`, `"Story"`, `"TextFrame"`, ...).
/// `seq` — a per-class sequence counter the caller manages.
pub fn self_id(sample: &str, path: &str, seq: u32) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(sample.as_bytes());
    hasher.update(b"::");
    hasher.update(path.as_bytes());
    hasher.update(b"::");
    hasher.update(&seq.to_le_bytes());
    let digest = hasher.finalize();
    let bytes = digest.as_bytes();
    // Hex-encode the first 3 bytes → 6 hex characters.
    let mut out = String::with_capacity(7);
    out.push('u');
    for b in &bytes[..3] {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_across_calls() {
        let a = self_id("geometry", "Spread", 0);
        let b = self_id("geometry", "Spread", 0);
        assert_eq!(a, b);
    }

    #[test]
    fn different_inputs_produce_different_ids() {
        assert_ne!(
            self_id("geometry", "Spread", 0),
            self_id("geometry", "Spread", 1)
        );
        assert_ne!(
            self_id("geometry", "Spread", 0),
            self_id("geometry", "Page", 0)
        );
        assert_ne!(
            self_id("geometry", "Spread", 0),
            self_id("text", "Spread", 0)
        );
    }

    #[test]
    fn shape_matches_indesign_convention() {
        let id = self_id("geometry", "Spread", 0);
        assert_eq!(id.len(), 7);
        assert!(id.starts_with('u'));
        assert!(id[1..].chars().all(|c| c.is_ascii_hexdigit()));
    }
}
