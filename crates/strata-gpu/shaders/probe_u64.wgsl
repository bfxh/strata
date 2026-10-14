// B2 parity 技术探针（非承重）：本机 wgpu24/Naga-24/Vulkan + SHADER_INT64 下实测
// direct / limb / mix_direct / mix_limb 四项 GPU==CPU 全 MATCH ⇒ u64 算术逐位准，
// splitmix64 可直译进 WGSL（真门 B2 走此路，device 需申请 SHADER_INT64）。
// 教训登记：前一轮误报"u64 乘错位"是这里常量写成 0xbf58476d1ce4e5b（少末位 9）的
// 笔误，不是引擎缺陷——parity 探针的期望值和被测值必须锚同一常量。

struct Out {
    direct: u64, // seed * c1（u64 直译乘）
    limb: u64, // 通过 u32 双 limb 手工模拟的 seed * c1
    mix_direct: u64, // splitmix64 直译
    mix_limb: u64, // splitmix64 全走 limb 模拟
};

@group(0) @binding(0) var<storage, read_write> out: Out;

// splitmix64 常量（正确 16 位十六进制 + naga 的 lu 后缀）。
const C1: u64 = 0xbf58476d1ce4e5b9lu;
const C2: u64 = 0x94d049bb133111eblu;

fn mul32(a: u32, b: u32) -> vec2<u32> {
    let a0 = a & 0xffffu;
    let a1 = a >> 16u;
    let b0 = b & 0xffffu;
    let b1 = b >> 16u;
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 16u) + (p01 & 0xffffu) + (p10 & 0xffffu);
    let lo = (p00 & 0xffffu) | (mid << 16u);
    let hi = p11 + (p01 >> 16u) + (p10 >> 16u) + (mid >> 16u);
    return vec2<u32>(lo, hi);
}

fn mul64_limb(a: u64, b: u64) -> u64 {
    let a0 = u32(a & 0xfffffffflu);
    let a1 = u32(a >> 32u);
    let b0 = u32(b & 0xfffffffflu);
    let b1 = u32(b >> 32u);
    let m00 = mul32(a0, b0);
    let m01 = mul32(a0, b1);
    let m10 = mul32(a1, b0);
    let p00 = (u64(m00.y) << 32u) | u64(m00.x);
    let s = u64(m01.x) + u64(m10.x);
    return p00 + (s << 32u);
}

fn mix_direct(x: u64) -> u64 {
    var v = x;
    v = (v ^ (v >> 30u)) * C1;
    v = (v ^ (v >> 27u)) * C2;
    return v ^ (v >> 31u);
}

fn mix_limb(x: u64) -> u64 {
    var v = x;
    v = mul64_limb(v ^ (v >> 30u), C1);
    v = mul64_limb(v ^ (v >> 27u), C2);
    return v ^ (v >> 31u);
}

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let r = u64(gid.x); // 恒 0，阻断常量折叠
    let seed: u64 = 0x123456789abcdef0lu + r;
    out.direct = seed * C1;
    out.limb = mul64_limb(seed, C1);
    out.mix_direct = mix_direct(seed);
    out.mix_limb = mix_limb(seed);
}
