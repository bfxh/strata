//! 最小列主序 Mat4（不引第三方数学 crate：矩阵是承重的渲染路径件，
//! 面小、可逐条断言，自己钉死比引依赖更可控）。
#![allow(clippy::must_use_candidate)]

/// 列主序 4x4（与 wgpu uniform 布局一致）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4([
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]);

    /// 右乘：self * other（列主序）。
    pub fn mul(&self, o: &Mat4) -> Mat4 {
        let mut r = [0.0f32; 16];
        for c in 0..4 {
            for rr in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += self.0[k * 4 + rr] * o.0[c * 4 + k];
                }
                r[c * 4 + rr] = s;
            }
        }
        Mat4(r)
    }

    /// 右手透视，**wgpu 深度约定 z ∈ [0,1]**（Vulkan/Metal 半空间）。
    /// 推导：clip_w=-z（前向 -z 为负 ⇒ w 正）；近平面 z=-n 投到 0，远平面 z=-f 投到 1。
    /// 解出 m10 = f/(n-f)，m14 = f·n/(n-f)（均负），与 GL 的 [-1,1] 版不同——
    /// 用错会让近处几何被深度裁掉（wgpu 下）。
    pub fn perspective_rh(f_y_rad: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        assert!(near > 0.0 && far > near && aspect > 0.0);
        let f = 1.0 / (f_y_rad / 2.0).tan();
        let nf = near - far; // 负
        Mat4([
            f / aspect,
            0.0,
            0.0,
            0.0,
            0.0,
            f,
            0.0,
            0.0,
            0.0,
            0.0,
            far / nf, // m10
            -1.0,     // m11
            0.0,
            0.0,
            far * near / nf, // m14
            0.0,
        ])
    }

    /// 右手 look-at（眼位、目标、up）。
    pub fn look_at_rh(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> Mat4 {
        let z = norm([eye[0] - center[0], eye[1] - center[1], eye[2] - center[2]]);
        let x = norm(cross(up, z));
        let y = cross(z, x);
        Mat4([
            x[0],
            y[0],
            z[0],
            0.0,
            x[1],
            y[1],
            z[1],
            0.0,
            x[2],
            y[2],
            z[2],
            0.0,
            -dot(x, eye),
            -dot(y, eye),
            -dot(z, eye),
            1.0,
        ])
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    [v[0] / l, v[1] / l, v[2] / l]
}

#[must_use]
pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 4] {
    let mut o = [0.0f32; 4];
    for (k, row) in o.iter_mut().enumerate() {
        *row = m.0[k] * p[0] + m.0[4 + k] * p[1] + m.0[8 + k] * p[2] + m.0[12 + k];
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAU_EPS: f32 = 1e-6;

    #[test]
    fn identity_mul_is_noop() {
        let p = Mat4::perspective_rh(1.0, 1.6, 0.1, 100.0);
        let q = Mat4::IDENTITY.mul(&p);
        assert_eq!(q, p);
    }

    #[test]
    fn look_at_maps_eye_to_origin() {
        let eye = [3.0, 4.0, 5.0];
        let v = Mat4::look_at_rh(eye, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let o = transform_point(&v, eye);
        assert!(o[0].abs() < TAU_EPS && o[1].abs() < TAU_EPS && o[2].abs() < TAU_EPS);
        assert!((o[3] - 1.0).abs() < TAU_EPS);
    }

    #[test]
    fn look_at_up_axis_stays_up_at_target() {
        // 沿 -Z 看向原点，眼上方点的视图 y 应 > 0
        let v = Mat4::look_at_rh([0.0, 0.0, 10.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let o = transform_point(&v, [0.0, 2.0, 0.0]);
        assert!(o[1] > 1.99);
    }

    #[test]
    fn perspective_maps_near_far_to_wgpu_depth() {
        let (n, f) = (0.5f32, 10.0f32);
        let p = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, n, f);
        // 点在 (0,0,-n) ⇒ z_ndc 应为 0；(0,0,-f) ⇒ 1（wgpu [0,1] 深度）
        let pn = transform_point(&p, [0.0, 0.0, -n]);
        let pf = transform_point(&p, [0.0, 0.0, -f]);
        assert!((pn[2] / pn[3]).abs() < TAU_EPS, "near→{}/{}", pn[2], pn[3]);
        let d = pf[2] / pf[3];
        assert!((d - 1.0).abs() < TAU_EPS, "far→{d}");
    }

    #[test]
    fn perspective_projects_point_decisively() {
        let p = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, 1.0, 10.0);
        let o = transform_point(&p, [1.0, 0.0, -5.0]);
        // w = -z_view = 5（RH 前向 -Z）
        assert!((o[3] - 5.0).abs() < TAU_EPS, "w={}", o[3]);
        // f = 1/tan(π/8)；x_ndc = f*x/w
        let f = 1.0 / (std::f32::consts::FRAC_PI_4 / 2.0).tan();
        let xd = o[0] / o[3];
        assert!((xd - f / 5.0).abs() < 1e-4, "x_ndc={xd} 期望 {}", f / 5.0);
        // 深度落在 (0,1) 内（在锥体内）
        let zd = o[2] / o[3];
        assert!(zd > 0.0 && zd < 1.0, "z_ndc={zd}");
    }

    #[test]
    fn matrix_mul_matches_known_product() {
        // 用平移矩阵验证乘序：T * P 应把 P 的原点送到 T 的位置
        let t = Mat4([
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 7.0, 8.0, 9.0, 1.0,
        ]);
        let o = transform_point(&t.mul(&Mat4::IDENTITY), [0.0, 0.0, 0.0]);
        assert_eq!(&o[..3], &[7.0, 8.0, 9.0]);
    }
}
