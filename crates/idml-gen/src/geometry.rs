//! Affine-matrix helpers in IDML's row-major `[a, b, c, d, tx, ty]`
//! convention.
//!
//! IDML stores `ItemTransform` as a six-number string in this exact
//! order (spec §10.3.2 "Geometry Example"):
//!
//! ```text
//! [ a  c  tx ]
//! [ b  d  ty ]
//! [ 0  0  1  ]
//! ```
//!
//! That is, `(x, y)` becomes `(a*x + c*y + tx, b*x + d*y + ty)`. The
//! helpers below all return matrices in that layout so the caller can
//! pass them to `format_matrix` unchanged.

pub type Matrix = [f32; 6];

pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

pub fn translate(dx: f32, dy: f32) -> Matrix {
    [1.0, 0.0, 0.0, 1.0, dx, dy]
}

pub fn scale(sx: f32, sy: f32) -> Matrix {
    [sx, 0.0, 0.0, sy, 0.0, 0.0]
}

pub fn rotate_deg(deg: f32) -> Matrix {
    let r = deg.to_radians();
    let (s, c) = (r.sin(), r.cos());
    [c, s, -s, c, 0.0, 0.0]
}

/// Apply `b` after `a` (i.e. `compose(translate, rotate)` rotates
/// first, then translates the rotated coordinate frame).
pub fn compose(a: Matrix, b: Matrix) -> Matrix {
    let [a0, a1, a2, a3, a4, a5] = a;
    let [b0, b1, b2, b3, b4, b5] = b;
    [
        b0 * a0 + b2 * a1,
        b1 * a0 + b3 * a1,
        b0 * a2 + b2 * a3,
        b1 * a2 + b3 * a3,
        b0 * a4 + b2 * a5 + b4,
        b1 * a4 + b3 * a5 + b5,
    ]
}

/// Format a matrix as the space-separated 4-decimal string IDML uses
/// inside `ItemTransform="..."`.
pub fn format_matrix(m: &Matrix) -> String {
    format!(
        "{} {} {} {} {} {}",
        crate::xml::format_f32(m[0]),
        crate::xml::format_f32(m[1]),
        crate::xml::format_f32(m[2]),
        crate::xml::format_f32(m[3]),
        crate::xml::format_f32(m[4]),
        crate::xml::format_f32(m[5]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(m: &Matrix, expected: &Matrix) {
        for (i, (a, b)) in m.iter().zip(expected.iter()).enumerate() {
            assert!((a - b).abs() < 1e-4, "slot {i}: {a} vs {b}");
        }
    }

    #[test]
    fn identity_is_identity() {
        approx(&IDENTITY, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn rotate_90_swaps_axes() {
        let m = rotate_deg(90.0);
        approx(&m, &[0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn translate_then_identity_is_translate() {
        approx(
            &compose(translate(10.0, 20.0), IDENTITY),
            &translate(10.0, 20.0),
        );
    }

    #[test]
    fn rotate_then_translate_orders_correctly() {
        // rotate 90° then translate (10, 0): (1,0) → (0,1) → (10,1).
        let m = compose(rotate_deg(90.0), translate(10.0, 0.0));
        let (x, y) = (
            m[0] * 1.0 + m[2] * 0.0 + m[4],
            m[1] * 1.0 + m[3] * 0.0 + m[5],
        );
        assert!((x - 10.0).abs() < 1e-4);
        assert!((y - 1.0).abs() < 1e-4);
    }
}
