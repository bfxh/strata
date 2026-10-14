// B2 parity 技术探针（非承重）：确认本机 u32 原语是否逐位准，
// 并复现 u64 乘法错位。u32 准 ⇒ limb 模拟 u64 乘这条路可走（真模拟留在 B2 门里做对）。

struct Out {
    mul_direct: u64, // 预期与 CPU 不一致（u64 乘错位复现）
    mul32: u32,
    add32: u32,
    mulhi_u16: u32,
    shift64: u64,
    add64: u64,
};

@group(0) @binding(0) var<storage, read_write> out: Out;

const C1: u64 = 0xbf58476d1ce4e5blu;

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    // gid.x 恒 0（1×1×1 dispatch），但阻断 Naga 常量折叠 ⇒ u32/u64 回绕走运行期语义。
    let r = gid.x;
    let seed: u64 = 0x123456789abcdef0lu;
    out.mul_direct = seed * (C1 - u64(r));
    out.mul32 = 0x9e3779b9u * (0x85ebca6bu + r);
    out.add32 = 0xfffffff7u + (0x9u + r);
    out.mulhi_u16 = 0xffffu * (0xffffu + r);
    out.shift64 = seed >> (32u - r);
    out.add64 = 0xfffffffffffffffflu + (7lu - u64(r));
}
