// B2 parity 门被测核：strata-nodes::noise 的 WGSL 直译（hash2/perlin/fbm/warped_fbm）。
// 纪律同 CPU：只用 u64 整数乘/加/移/异或 + f32 的 + - * / floor，无超越函数。
// u64 算术在本机 wgpu24/Naga-24+SHADER_INT64 已实测逐位准（见 probe_u64 探针）。
// 输出量化 i16（存进 i32），round 用「半程远离零」对齐 Rust f32::round()。

struct Params {
    seed_lo: u32,
    seed_hi: u32,
    step_mm: i32, // 每像素世界步长（毫米，i32 域内，< 2^24 保证 f32 精确）
    wavelength_mm: i32,
    octaves: u32,
    has_warp: u32,
    warp_amp_mm: i32,
    warp_wl_mm: i32,
    grid_n: u32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> out: array<i32>;

fn seed() -> u64 {
    return (u64(p.seed_hi) << 32u) | u64(p.seed_lo);
}

fn mix64(xin: u64) -> u64 {
    var x = xin;
    x = (x ^ (x >> 30u)) * 0xbf58476d1ce4e5b9lu;
    x = (x ^ (x >> 27u)) * 0x94d049bb133111eblu;
    return x ^ (x >> 31u);
}

// 与 CPU hash2 一致：ix,iy 的补码低 32 位当 u64 用。
fn hash2(ix: i32, iy: i32) -> u64 {
    let x = u64(u32(ix)) * 0x9e3779b97f4a7c15lu;
    let y = u64(u32(iy)) * 0xbf58476d1ce4e5b9lu;
    return mix64(seed() ^ x ^ y);
}

const GRAD_X = array<f32, 8>(1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 0.0, 0.0);
const GRAD_Y = array<f32, 8>(1.0, 1.0, -1.0, -1.0, 0.0, 0.0, 1.0, -1.0);

fn grad2(h: u64, dx: f32, dy: f32) -> f32 {
    let ix = u32(h & 7lu);
    return GRAD_X[ix] * dx + GRAD_Y[ix] * dy;
}

fn fade(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    return a + t * (b - a);
}

// 经典 Perlin，世界坐标 i32 毫米 + 波长，输出约 [-1,1]。
fn perlin(wx: i32, wy: i32, wl: i32) -> f32 {
    let gx = f32(wx) / f32(wl);
    let gy = f32(wy) / f32(wl);
    let x0 = floor(gx);
    let y0 = floor(gy);
    let fx = gx - x0;
    let fy = gy - y0;
    let ix0 = i32(x0);
    let iy0 = i32(y0);
    let u = fade(fx);
    let v = fade(fy);
    let n00 = grad2(hash2(ix0, iy0), fx, fy);
    let n10 = grad2(hash2(ix0 + 1, iy0), fx - 1.0, fy);
    let n01 = grad2(hash2(ix0, iy0 + 1), fx, fy - 1.0);
    let n11 = grad2(hash2(ix0 + 1, iy0 + 1), fx - 1.0, fy - 1.0);
    let a = lerp(n00, n10, u);
    let b = lerp(n01, n11, u);
    return lerp(a, b, v);
}

// gain=0.5 逐层减半波长；与 CPU fbm 的 saturating_div(2).max(1) 对齐（wl 恒 ≥1）。
fn fbm(wx: i32, wy: i32, base_wl: i32, oct: u32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var norm = 0.0;
    var wl = base_wl;
    for (var i: u32 = 0u; i < oct; i = i + 1u) {
        sum = sum + amp * perlin(wx, wy, wl);
        norm = norm + amp;
        amp = amp * 0.5;
        wl = max(1, wl / 2);
    }
    if (norm > 0.0) {
        return sum / norm;
    }
    return 0.0;
}

fn fbm_seed(wx: i32, wy: i32, base_wl: i32, oct: u32, sd: u64) -> f32 {
    // 扭曲用的两路低频 fbm，走独立 seed（不进 p.seed，避免污染 hash2）。
    var sum = 0.0;
    var amp = 1.0;
    var norm = 0.0;
    var wl = base_wl;
    for (var i: u32 = 0u; i < oct; i = i + 1u) {
        sum = sum + amp * perlin_seed(wx, wy, wl, sd);
        norm = norm + amp;
        amp = amp * 0.5;
        wl = max(1, wl / 2);
    }
    if (norm > 0.0) {
        return sum / norm;
    }
    return 0.0;
}

fn hash2_seed(ix: i32, iy: i32, sd: u64) -> u64 {
    let x = u64(u32(ix)) * 0x9e3779b97f4a7c15lu;
    let y = u64(u32(iy)) * 0xbf58476d1ce4e5b9lu;
    return mix64(sd ^ x ^ y);
}

fn perlin_seed(wx: i32, wy: i32, wl: i32, sd: u64) -> f32 {
    let gx = f32(wx) / f32(wl);
    let gy = f32(wy) / f32(wl);
    let x0 = floor(gx);
    let y0 = floor(gy);
    let fx = gx - x0;
    let fy = gy - y0;
    let ix0 = i32(x0);
    let iy0 = i32(y0);
    let u = fade(fx);
    let v = fade(fy);
    let n00 = grad2(hash2_seed(ix0, iy0, sd), fx, fy);
    let n10 = grad2(hash2_seed(ix0 + 1, iy0, sd), fx - 1.0, fy);
    let n01 = grad2(hash2_seed(ix0, iy0 + 1, sd), fx, fy - 1.0);
    let n11 = grad2(hash2_seed(ix0 + 1, iy0 + 1, sd), fx - 1.0, fy - 1.0);
    let a = lerp(n00, n10, u);
    let b = lerp(n01, n11, u);
    return lerp(a, b, v);
}

fn round_away(x: f32) -> f32 {
    // 半程远离零，等价 Rust f32::round()（WGSL 内建 round 是半程取偶）。
    return sign(x) * floor(abs(x) + 0.5);
}

fn warped_fbm(wx: i32, wy: i32) -> f32 {
    if (p.has_warp == 0u) {
        return fbm(wx, wy, p.wavelength_mm, p.octaves);
    }
    let sd = seed();
    let ox = fbm_seed(wx, wy, p.warp_wl_mm, 2u, sd ^ 0x0a1b2c3dlu);
    let oy = fbm_seed(wx, wy, p.warp_wl_mm, 2u, sd ^ 0x4e5f6a7blu);
    let dx = i32(round_away(ox * f32(p.warp_amp_mm)));
    let dy = i32(round_away(oy * f32(p.warp_amp_mm)));
    return fbm(wx + dx, wy + dy, p.wavelength_mm, p.octaves);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= p.grid_n || y >= p.grid_n) {
        return;
    }
    let wx = i32(x * u32(p.step_mm));
    let wy = i32(y * u32(p.step_mm));
    let v = warped_fbm(wx, wy);
    let q = round_away(v * 32767.0);
    out[i32(y * p.grid_n + x)] = i32(clamp(q, -32767.0, 32767.0));
}
