/// 2D Transformation Matrix (3x3).
/// Represented as a flat array of 9 elements in row-major order:
/// [ a, c, e ]
/// [ b, d, f ]
/// [ 0, 0, 1 ]
///
/// In CSS context: `matrix(a, b, c, d, e, f)`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix3x3(pub [f32; 9]);

impl Matrix3x3 {
    pub fn identity() -> Self {
        Self([
            1.0, 0.0, 0.0,
            0.0, 1.0, 0.0,
            0.0, 0.0, 1.0,
        ])
    }

    pub fn translate(tx: f32, ty: f32) -> Self {
        Self([
            1.0, 0.0, tx,
            0.0, 1.0, ty,
            0.0, 0.0, 1.0,
        ])
    }

    pub fn scale(sx: f32, sy: f32) -> Self {
        Self([
            sx,  0.0, 0.0,
            0.0, sy,  0.0,
            0.0, 0.0, 1.0,
        ])
    }

    pub fn rotate(radians: f32) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self([
            cos, -sin, 0.0,
            sin,  cos, 0.0,
            0.0,  0.0, 1.0,
        ])
    }

    pub fn multiply(&self, other: &Self) -> Self {
        let mut result = [0.0; 9];
        for i in 0..3 {
            for j in 0..3 {
                for k in 0..3 {
                    result[i * 3 + j] += self.0[i * 3 + k] * other.0[k * 3 + j];
                }
            }
        }
        Self(result)
    }
}

/// 3D Transformation Matrix (4x4).
/// Row-major order (standard for modern graphics APIs and CSS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix4x4(pub [f32; 16]);

impl Matrix4x4 {
    pub fn identity() -> Self {
        Self([
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn from_2d(m: Matrix3x3) -> Self {
        Self([
            m.0[0], m.0[1], 0.0, m.0[2],
            m.0[3], m.0[4], 0.0, m.0[5],
            0.0,    0.0,    1.0, 0.0,
            m.0[6], m.0[7], 0.0, m.0[8],
        ])
    }

    pub fn multiply(&self, other: &Self) -> Self {
        let mut result = [0.0; 16];
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    result[i * 4 + j] += self.0[i * 4 + k] * other.0[k * 4 + j];
                }
            }
        }
        Self(result)
    }

    pub fn translate(tx: f32, ty: f32, tz: f32) -> Self {
        Self([
            1.0, 0.0, 0.0, tx,
            0.0, 1.0, 0.0, ty,
            0.0, 0.0, 1.0, tz,
            0.0, 0.0, 0.0, 1.0,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matrix_identity() {
        let m = Matrix3x3::identity();
        assert_eq!(m.0[0], 1.0);
        assert_eq!(m.0[4], 1.0);
        assert_eq!(m.0[8], 1.0);
    }

    #[test]
    fn test_matrix_translate() {
        let m = Matrix3x3::translate(10.0, 20.0);
        assert_eq!(m.0[2], 10.0);
        assert_eq!(m.0[5], 20.0);
    }

    #[test]
    fn test_matrix_multiply() {
        let m1 = Matrix3x3::translate(10.0, 0.0);
        let m2 = Matrix3x3::translate(0.0, 20.0);
        let result = m1.multiply(&m2);
        // [1 0 10] * [1 0 0 ] = [1 0 10]
        // [0 1 0 ]   [0 1 20]   [0 1 20]
        // [0 0 1 ]   [0 0 1 ]   [0 0 1 ]
        assert_eq!(result.0[2], 10.0);
        assert_eq!(result.0[5], 20.0);
    }

    #[test]
    fn test_matrix_4x4_from_2d() {
        let m2d = Matrix3x3::translate(10.0, 20.0);
        let m4d = Matrix4x4::from_2d(m2d);
        assert_eq!(m4d.0[3], 10.0);
        assert_eq!(m4d.0[7], 20.0);
        assert_eq!(m4d.0[10], 1.0); // Z-axis scale
        assert_eq!(m4d.0[15], 1.0); // W-axis
    }
}
