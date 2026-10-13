//! 地形网格拓扑：正则网格 + skirt 封缝 + 每层独立 near/far。
//! 这是 P5（LOD 裂缝 / z-fighting）的数据侧地基；WGSL 顶点程序将 1:1 复刻
//! 本模块的索引规律（GPU Gems 2 Ch.2 的 strip 化思想），故先用纯 Rust 钉死。
//!
//! 坐标全部是**相对 eye 的局部量**（[-1,1] 归一化网格），呼应 strata-geo 的
//! RTE 契约——世界大数永不进顶点缓冲。

/// 网格 + skirt 的静态拓扑（与高度数据解耦：高度在绘制期由 tile 采样填充 y）。
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainMesh {
    /// 边长顶点数（n×n 网格）
    pub n: u32,
    /// skirt 下坠深度（归一化单位）
    pub skirt_drop: f32,
}

impl TerrainMesh {
    pub fn new(n: u32, skirt_drop: f32) -> Self {
        assert!(n >= 2, "网格至少 2x2");
        assert!(skirt_drop > 0.0, "skirt 必须下坠");
        Self { n, skirt_drop }
    }

    /// 主体三角数 = 2*(n-1)^2。
    pub fn base_triangles(&self) -> u32 {
        2 * (self.n - 1) * (self.n - 1)
    }

    /// 索引总量：主体(6·e²) + 四边 skirt(每边 e 个四边形 = 6e 索引) ⇒ 6e² + 24e。
    pub fn index_count(&self) -> usize {
        let e = (self.n - 1) as usize;
        6 * e * e + 24 * e
    }

    /// 生成索引缓冲。顶点布局：前 n*n 为主体网格（行主序，`v = row*n + col`），
    /// 随后四段各 n 个 skirt 顶点（下坠副本）：`[south, north, west, east]`。
    /// skirt 四边形连接"边界顶点 i — 边界顶点 i+1 — 其下坠副本 i — 副本 i+1"，
    /// 层间高度差被裙边立面遮蔽 ⇒ 无 T-junction 裂缝（P5 封缝）。
    pub fn indices(&self) -> Vec<u32> {
        let n = self.n as usize;
        let e = n - 1;
        let mut idx = Vec::with_capacity(self.index_count());

        // 主体：逐格两个三角形
        for row in 0..e {
            for col in 0..e {
                let v00 = (row * n + col) as u32;
                let v10 = v00 + 1;
                let v01 = v00 + n as u32;
                let v11 = v01 + 1;
                idx.extend_from_slice(&[v00, v01, v11, v00, v11, v10]);
            }
        }

        let skirt0 = (n * n) as u32;
        let south = skirt0;
        let north = skirt0 + n as u32;
        let west = skirt0 + 2 * n as u32;
        let east = skirt0 + 3 * n as u32;

        // 一个裙边四边形（边界 b0,b1 + 下坠副本 s0,s1）拆两三角形
        let quad = |b0: u32, b1: u32, s0: u32, s1: u32, idx: &mut Vec<u32>| {
            idx.extend_from_slice(&[b0, s0, b1, b1, s0, s1]);
        };

        for i in 0..e {
            let ii = i as u32;
            // south：边界顶行 v=col
            quad(ii, ii + 1, south + ii, south + ii + 1, &mut idx);
            // north：边界末行 v=(n-1)*n + col
            let r = (n - 1) as u32 * n as u32;
            quad(r + ii, r + ii + 1, north + ii, north + ii + 1, &mut idx);
            // west：边界首列 v=row*n
            let c0 = ii * n as u32;
            let c0n = (ii + 1) * n as u32;
            quad(c0, c0n, west + ii, west + ii + 1, &mut idx);
            // east：边界末列 v=row*n + (n-1)
            let c1 = ii * n as u32 + (n as u32 - 1);
            let c1n = (ii + 1) * n as u32 + (n as u32 - 1);
            quad(c1, c1n, east + ii, east + ii + 1, &mut idx);
        }
        idx
    }

    /// 顶点总数：主体 n² + 4 条边各 n 个 skirt 顶点。
    pub fn vertex_count(&self) -> u32 {
        self.n * self.n + 4 * self.n
    }
}

/// 每层独立 near/far（消 z-fighting 的构造法，GPU Gems 2 Ch.2）：
/// 输入各层可见半径（近→远递增，单位 mm）。第 i 层画**环带**
/// `[r_{i-1}, r_i]`（i=0 时近端取 `max(1, r0/64)` 作正近裁剪面），
/// 投影把该环压缩到深度窗口 `[clear_i, clear_{i+1})`：绘制前 clear depth
/// 到 `clear_i = i/(n+1)`，层间深度互不重叠 ⇒ float 精度全部花在环带厚度上。
/// 输出 `(near_mm, far_mm, clear_depth)` per layer。
pub fn per_layer_depth_ranges(layer_radii_mm: &[i64]) -> Vec<(f32, f32, f32)> {
    let n = layer_radii_mm.len();
    let mut out = Vec::with_capacity(n);
    for (i, &r_i) in layer_radii_mm.iter().enumerate() {
        let far = r_i.max(1) as f64;
        let near = if i == 0 {
            (far / 64.0).max(1.0)
        } else {
            (layer_radii_mm[i - 1].max(1) as f64).min(far)
        };
        debug_assert!(near <= far, "半径序列须递增");
        out.push((near as f32, far as f32, i as f32 / (n as f32 + 1.0)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_counts_are_consistent() {
        let m = TerrainMesh::new(8, 0.02);
        assert_eq!(m.vertex_count(), 64 + 32);
        assert_eq!(m.index_count(), m.indices().len());
        assert_eq!(m.base_triangles(), 98);
        // 总三角 = 98 主体 + 4 边×7 四边形×2 = 98+56 = 154
        assert_eq!(m.indices().len() / 3, 154);
    }

    #[test]
    fn all_indices_in_range() {
        let m = TerrainMesh::new(16, 0.02);
        let v = m.vertex_count();
        for &i in &m.indices() {
            assert!(i < v, "index {i} 越出顶点数 {v}");
        }
    }

    #[test]
    fn skirt_edges_covered_every_layer_boundary() {
        // 每个 skirt 顶点都被裙边三角形引用 ⇒ 层边界全程有立面遮蔽（P5 封缝主张）
        let m = TerrainMesh::new(5, 0.01);
        let idx = m.indices();
        let skirt0 = (5 * 5) as u32;
        let seg = 5usize;
        let mut used = vec![false; 4 * seg];
        for &v in &idx {
            if let Some(k) = v.checked_sub(skirt0) {
                if (k as usize) < 4 * seg {
                    used[k as usize] = true;
                }
            }
        }
        assert!(used.iter().all(|&u| u), "存在未封缝的 skirt 顶点");
    }

    #[test]
    fn indices_deterministic_and_closed_mesh() {
        let a = TerrainMesh::new(4, 0.02).indices();
        let b = TerrainMesh::new(4, 0.02).indices();
        assert_eq!(a, b);
        // 闭合性：每个顶点度数 ≥ 1（无孤儿顶点）
        let m = TerrainMesh::new(4, 0.02);
        let mut deg = vec![0u32; m.vertex_count() as usize];
        for &i in &a {
            deg[i as usize] += 1;
        }
        assert!(deg.iter().all(|&d| d > 0));
    }

    #[test]
    fn depth_ranges_are_touching_rings_with_monotonic_clear() {
        let radii = [512i64, 1024, 2048];
        let ds = per_layer_depth_ranges(&radii);
        assert_eq!(ds.len(), 3);
        // 环带：far_i == r_i；near_i == r_{i-1}（相接不重叠）；i=0 近端为正小数
        assert_eq!(ds[0].1, 512.0);
        assert!(ds[0].0 > 0.0 && ds[0].0 < 512.0);
        assert_eq!(ds[1].0, 512.0);
        assert_eq!(ds[1].1, 1024.0);
        assert_eq!(ds[2].0, 1024.0);
        assert_eq!(ds[2].1, 2048.0);
        // clear_depth 单调递增且全部 <1（绘制序近→远，每层画前清到各自分隔值）
        let mut prev = -1.0f32;
        for &(_, _, clear) in &ds {
            assert!(clear > prev, "clear_depth 非单调");
            assert!(clear < 1.0);
            prev = clear;
        }
        // 空输入不炸
        assert!(per_layer_depth_ranges(&[]).is_empty());
    }

    #[test]
    fn depth_ranges_tolerate_nonincreasing_radii_without_inversion() {
        // 防御面：递减输入不应产生 near>far 的区间（min 钳制）
        let ds = per_layer_depth_ranges(&[2048, 1024, 512]);
        for &(near, far, _) in &ds {
            assert!(near <= far, "ring inverted: {near} > {far}");
        }
    }
}
