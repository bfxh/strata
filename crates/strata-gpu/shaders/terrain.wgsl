// 地形单层渲染：顶点从高度纹理取高，片元用差分法线做 Lambert。
// 与 CPU 侧 mesh.rs 的顶点布局对偶（x,z,drop）；B2 parity 门比较的是
// CPU RegionCache 高度 vs 本 shader textureLoad 的高度（量化 i16 后）。

struct Uniforms {
    view_proj: mat4x4<f32>,
    height_scale: f32,
    tex_dim: f32,
    half_span: f32,
    skirt_scale: f32,
    offset_x: f32,
    offset_z: f32,
    _pad0: f32,
    _pad1: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var height_tex: texture_2d<f32>;

struct VsIn {
    @location(0) xz: vec2<f32>,
    @location(1) drop: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) h: f32,
};

fn texel(tex_dim: i32, uv: vec2<f32>) -> vec2<i32> {
    let t = clamp(uv, vec2(0.0), vec2(0.99999));
    return vec2<i32>(i32(t.x * f32(tex_dim)), i32(t.y * f32(tex_dim)));
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var o: VsOut;
    let uv = in.xz * 0.5 + vec2(0.5);
    let dim = i32(u.tex_dim);
    let h = textureLoad(height_tex, texel(dim, uv), 0).r;
    // 世界局部坐标（相对 eye，米）：网格 [-1,1] × half_span + 层偏移
    let wx = in.xz.x * u.half_span + u.offset_x;
    let wz = in.xz.y * u.half_span + u.offset_z;
    let y = h * u.height_scale - in.drop * u.skirt_scale;
    o.clip = u.view_proj * vec4<f32>(wx, y, wz, 1.0);
    o.uv = uv;
    o.h = h;
    return o;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dim = i32(u.tex_dim);
    let step = 1.0 / f32(dim);
    let hl = textureLoad(height_tex, texel(dim, in.uv - vec2(step, 0.0)), 0).r;
    let hr = textureLoad(height_tex, texel(dim, in.uv + vec2(step, 0.0)), 0).r;
    let hd = textureLoad(height_tex, texel(dim, in.uv - vec2(0.0, step)), 0).r;
    let hu = textureLoad(height_tex, texel(dim, in.uv + vec2(0.0, step)), 0).r;
    let n = normalize(vec3<f32>(hl - hr, 2.0 * step * 8.0 / max(u.height_scale, 0.0001), hd - hu));
    let light = normalize(vec3(0.4, 0.8, 0.35));
    let lam = max(dot(n, light), 0.0) * 0.85 + 0.15;
    // 高度着色：低处青绿 → 高处岩灰
    let lo = vec3<f32>(0.21, 0.44, 0.30);
    let hi = vec3<f32>(0.62, 0.60, 0.58);
    let k = clamp(in.h * 0.5 + 0.5, 0.0, 1.0);
    let base = mix(lo, hi, k * k);
    return vec4<f32>(base * lam, 1.0);
}

// ---- sprite 通道：把离屏地形纹理贴进 egui 主 pass（viewport 限定矩形） ----

struct SpriteOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var disp_tex: texture_2d<f32>;
@group(0) @binding(1) var disp_smp: sampler;

@vertex
fn vs_sprite(@builtin(vertex_index) vi: u32) -> SpriteOut {
    // 覆盖视口的超大三角形：(-1,-1),(3,-1),(-1,3)。
    // wgpu 帧缓冲 y 向下 ⇒ uv.y = (1 - ndc.y)/2，uv=(0,0) 对应纹理左上（行主序高度场）。
    var x = -1.0;
    var y = -1.0;
    if (vi == 1u) { x = 3.0; }
    if (vi == 2u) { y = 3.0; }
    var o: SpriteOut;
    o.pos = vec4<f32>(x, y, 0.0, 1.0);
    o.uv = vec2<f32>(x, -y) * 0.5 + 0.5;
    return o;
}

@fragment
fn fs_sprite(in: SpriteOut) -> @location(0) vec4<f32> {
    return textureSample(disp_tex, disp_smp, in.uv);
}
