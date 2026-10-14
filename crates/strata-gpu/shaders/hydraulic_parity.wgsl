// B2 侵蚀 parity：CPU strata-nodes::erosion::Hydraulic 的 WGSL 直译（双 pass）。
// 一 dispatch 一步：pass1 compute_flow 写每格 4 向 flow_w/flow_s；
// pass2 integrate 每格按固定 [E,W,N,S] 顺序求 out/in 总和 ⇒ 新态。
// 与 CPU 逐位对齐要求：只用 f32 的 + - * / max min，无超越函数。

struct Params {
    n: u32,
    _pad0: u32,
    k_capacity: f32,
    k_dissolve: f32,
    k_deposit: f32,
    rain: f32,
    evaporation: f32,
    min_water: f32,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> state_in: array<f32>; // [h(L) | water(L) | sed(L)]
@group(0) @binding(2) var<storage, read_write> flow: array<f32>; // [flow_w(4L) | flow_s(4L)]
@group(0) @binding(3) var<storage, read_write> state_out: array<f32>;

const DX = array<i32, 4>(1, -1, 0, 0);
const DY = array<i32, 4>(0, 0, 1, -1);
// 邻居 nb 在方向 k，nb 朝本格流出对应 nb 的 FLIP[k] 方向。
const FLIP = array<u32, 4>(1u, 0u, 3u, 2u);

fn L() -> u32 {
    return p.n * p.n;
}

// 计算一格四向流出，直接写入 flow 缓冲（避免按值返回 array ⇒ Naga/SPIR-V 不稳）。
fn write_flow(cx: u32, cy: u32, c: u32) {
    let n = p.n;
    let len = L();
    let h_c = state_in[c];
    let w_c = state_in[len + c];
    let s_c = state_in[2u * len + c];
    var drops = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    var total_drop = 0.0;
    if (w_c > p.min_water) {
        let head_c = h_c + w_c;
        for (var k: u32 = 0u; k < 4u; k = k + 1u) {
            let nx = i32(cx) + DX[k];
            let ny = i32(cy) + DY[k];
            if (nx >= 0 && ny >= 0 && u32(nx) < n && u32(ny) < n) {
                let nb = u32(ny) * n + u32(nx);
                let d = max(0.0, head_c - (state_in[nb] + state_in[len + nb]));
                drops[k] = d;
                total_drop = total_drop + d;
            }
        }
    }
    for (var k: u32 = 0u; k < 4u; k = k + 1u) {
        var fw = 0.0;
        var fs = 0.0;
        if (total_drop > 0.0) {
            let frac = drops[k] / total_drop;
            fw = w_c * frac;
            fs = s_c * frac;
        }
        flow[c * 4u + k] = fw;
        flow[4u * len + c * 4u + k] = fs;
    }
}

// Pass 1：把每格 4 向 flow_w/flow_s 写入 flow（供 pass2 读，避免二次算）。
@compute @workgroup_size(8, 8, 1)
fn pass1_flow(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= p.n || y >= p.n) {
        return;
    }
    let c = y * p.n + x;
    write_flow(x, y, c);
}

// Pass 2：每格按固定 [E,W,N,S] 顺序求 out/in 总和 ⇒ 新态。
@compute @workgroup_size(8, 8, 1)
fn pass2_integrate(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= p.n || y >= p.n) {
        return;
    }
    let n = p.n;
    let len = L();
    let c = y * n + x;

    var w_out_sum = 0.0;
    var s_out_sum = 0.0;
    for (var k: u32 = 0u; k < 4u; k = k + 1u) {
        w_out_sum = w_out_sum + flow[c * 4u + k];
        s_out_sum = s_out_sum + flow[4u * len + c * 4u + k];
    }
    var w_in_sum = 0.0;
    var s_in_sum = 0.0;
    for (var k: u32 = 0u; k < 4u; k = k + 1u) {
        let nx = i32(x) + DX[k];
        let ny = i32(y) + DY[k];
        if (nx < 0 || ny < 0 || u32(nx) >= n || u32(ny) >= n) {
            continue;
        }
        let nb = u32(ny) * n + u32(nx);
        let fk = FLIP[k];
        w_in_sum = w_in_sum + flow[nb * 4u + fk];
        s_in_sum = s_in_sum + flow[4u * len + nb * 4u + fk];
    }

    var h_new = state_in[c];
    var w_new = state_in[len + c] - w_out_sum + w_in_sum;
    var s_new = state_in[2u * len + c] - s_out_sum + s_in_sum;

    // 溶蚀/沉积（h ↔ sed 内搬运，Σ(h+sed) 守恒）。
    let capacity = p.k_capacity * w_out_sum;
    if (capacity > s_new) {
        let avail = (capacity - s_new) * p.k_dissolve;
        let eroded = min(avail, max(h_new, 0.0));
        h_new = h_new - eroded;
        s_new = s_new + eroded;
    } else {
        let excess = (s_new - capacity) * p.k_deposit;
        let dep = min(excess, max(s_new, 0.0));
        s_new = s_new - dep;
        h_new = h_new + dep;
    }

    // 降雨/蒸发（唯一水量进出）。
    let v = w_new + p.rain;
    w_new = v * (1.0 - p.evaporation);

    state_out[c] = h_new;
    state_out[len + c] = w_new;
    state_out[2u * len + c] = s_new;
}
