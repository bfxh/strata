//! 门禁编排入口。`cargo xtask gate` = 本地与 CI 同一把尺（06 文档）。
//!
//! M0 接线 A1(fmt)/A2(clippy)/A3(build+test)；M1d 接 D1/D2（bench 回归 + 峰值内存），
//! parity/golden 等判定门在对应里程碑挂进来。jobs=1 是本机内存约束下的硬默认。

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::process::{exit, Command};

/// core 三 crate 另开 pedantic -D warnings（06 A2）。
const CORE_CRATES: &[&str] = &["strata-geo", "strata-exec", "strata-nodes"];
const BASELINE_PATH: &str = "bench/baseline.env";
/// 回归余量（06 D1）：CPU 项确定性高=10%；GPU 帧率受背景负载抖动=25%（登记口径）。
const CPU_MARGIN: f64 = 1.10;
const FPS_MARGIN: f64 = 0.75;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("gate") | None => gate(),
        Some("gate-quick") => gate_quick(),
        Some("bench") => bench(&args[1..]),
        Some(other) => {
            eprintln!("unknown xtask subcommand: {other}");
            eprintln!("usage: cargo xtask [gate|gate-quick|bench [--check|--update|--selftest]]");
            exit(2);
        }
    };
    match result {
        Ok(()) => exit(0),
        Err(code) => exit(code),
    }
}

fn gate() -> Result<(), i32> {
    // A1 fmt
    run("A1 fmt", &["fmt", "--all", "--check"], &[])?;
    // A2 clippy：全 workspace -D warnings，core 另加 pedantic
    run(
        "A2 clippy(all)",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        &[],
    )?;
    for krate in CORE_CRATES {
        run(
            &format!("A2 clippy(pedantic:{krate})"),
            &[
                "clippy",
                "-p",
                krate,
                "--all-targets",
                "--",
                "-D",
                "warnings",
                "-W",
                "clippy::pedantic",
            ],
            &[],
        )?;
    }
    // A3 build（--workspace --locked：依赖漂移不可能判红，见 06）
    run(
        "A3 build",
        &["build", "--workspace", "--all-targets", "--locked"],
        &[],
    )?;
    // B1 test
    run("B1 test", &["test", "--workspace"], &[])?;
    // D1/D2：判红能力自检（无需 GPU）→ 实测对照基线（有 GPU 才有效判红；无 GPU 如实 SKIP 披露）
    bench_selftest()?;
    bench_check(false)?;
    ok("gate", "全部门禁绿");
    Ok(())
}

/// 提交前快门：只 fmt + core clippy + test，跳过全量 build。
fn gate_quick() -> Result<(), i32> {
    run("A1 fmt", &["fmt", "--all", "--check"], &[])?;
    run(
        "A2 clippy(all)",
        &["clippy", "--workspace", "--", "-D", "warnings"],
        &[],
    )?;
    run("B1 test", &["test", "--workspace"], &[])?;
    ok("gate-quick", "快门绿");
    Ok(())
}

/// 运行一条 cargo 子命令，返回其退出码；非零则短路（用 `?` 语义）。
fn run(label: &str, cargo_args: &[&str], _extra: &[(&str, &str)]) -> Result<(), i32> {
    println!("\n[xtask] == {label} ==");
    let mut cmd = Command::new(cargo_bin());
    // 本机内存约束：编译类命令并发锁 1。-j 必须紧跟子命令插队，
    // 不能追加在 `--`（rustc 参数分隔符）之后。fmt/check 类不接 -j。
    if matches!(
        cargo_args.first().copied(),
        Some("build") | Some("clippy") | Some("test")
    ) {
        cmd.arg(cargo_args[0]);
        cmd.arg("-j").arg("1");
        cmd.args(&cargo_args[1..]);
    } else {
        cmd.args(cargo_args);
    }
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("无法启动 cargo: {e}"));
    match status.code() {
        Some(0) => {
            ok(label, "通过");
            Ok(())
        }
        Some(c) => {
            eprintln!("[xtask] ✗ {label} 失败 (exit {c})");
            Err(c)
        }
        None => {
            eprintln!("[xtask] ✗ {label} 被信号中断");
            Err(101)
        }
    }
}

fn cargo_bin() -> String {
    env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn ok(label: &str, msg: &str) {
    println!("[xtask] ✓ {label}: {msg}");
}

// ---------- D1/D2 bench 门 ----------

fn bench(sub: &[String]) -> Result<(), i32> {
    let mode = sub.first().map(String::as_str).unwrap_or("--check");
    match mode {
        "--selftest" => bench_selftest(),
        "--update" => bench_update(),
        "--check" => bench_check(true),
        other => {
            eprintln!("unknown bench mode: {other}（可用 --check|--update|--selftest）");
            Err(2)
        }
    }
}

/// 解析 bin 输出的 KEY=VALUE 行。
fn parse_kv(text: &str) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.trim().split_once('=') {
            if let Ok(x) = v.parse::<f64>() {
                m.insert(k.to_string(), x);
            }
        }
    }
    m
}

fn run_bench_bin() -> Result<Option<HashMap<String, f64>>, i32> {
    let out = Command::new(cargo_bin())
        .args([
            "run",
            "-q",
            "-j",
            "1",
            "-p",
            "strata-app",
            "--bin",
            "strata",
            "--",
            "--bench-render",
            "90",
        ])
        .output()
        .map_err(|_| 101)?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.code() == Some(2) || text.contains("BENCH SKIP") {
        return Ok(None); // 无 GPU：调用方披露 SKIP
    }
    if !out.status.success() {
        eprintln!("{text}");
        return Err(out.status.code().unwrap_or(101));
    }
    Ok(Some(parse_kv(&text)))
}

fn load_baseline() -> Result<HashMap<String, f64>, i32> {
    if !Path::new(BASELINE_PATH).exists() {
        eprintln!("[xtask] ✗ {BASELINE_PATH} 不存在——先 `cargo xtask bench --update` 登记基线");
        return Err(101);
    }
    let text = fs::read_to_string(BASELINE_PATH).unwrap_or_default();
    Ok(parse_kv(&text))
}

/// 对照基线判红。返回 Err 即门红。`None`（无 GPU）按 strict 决定红或披露。
fn bench_check(strict: bool) -> Result<(), i32> {
    println!("\n[xtask] == D1/D2 bench ==");
    let Some(got) = run_bench_bin()? else {
        if strict {
            eprintln!("[xtask] ✗ D1/D2: 无 GPU 且机器未登记豁免（gate 须在有 GPU 面跑）");
            return Err(101);
        }
        ok("D1/D2", "SKIP（无 adapter，披露）");
        return Ok(());
    };
    let base = load_baseline()?;
    let fails = evaluate_against(&base, &got);
    if fails.is_empty() {
        ok("D1/D2", "bench 指标全部在基线余量内");
        Ok(())
    } else {
        for f in &fails {
            eprintln!("[xtask] ✗ D1/D2: {f}");
        }
        Err(101)
    }
}

/// 基线更新（单值改动走单独提交纪律——全量重录要在提交说明里逐项披露）。
fn bench_update() -> Result<(), i32> {
    let Some(got) = run_bench_bin()? else {
        eprintln!("[xtask] ✗ bench --update 需要 GPU（无 adapter 拒绝录基线）");
        return Err(101);
    };
    let mut out = String::from(
        "# D1/D2 基线（本机 Vulkan/RTX4060Ti, debug, min-of-3；余量 CPU±10% / FPS -25%）\n",
    );
    for k in [
        "NOISE_TILE_MS",
        "CLIPMAP_FULL_REGEN_MS",
        "AVG_FPS",
        "P50_MS",
        "P95_MS",
        "PEAK_RSS_MB",
    ] {
        if let Some(v) = got.get(k) {
            out.push_str(&format!("{k}={v:.3}\n"));
        }
    }
    out.push_str("PEAK_RSS_MB_MAX=512\n");
    fs::create_dir_all("bench").map_err(|_| 101)?;
    fs::write(BASELINE_PATH, out).map_err(|_| 101)?;
    ok("bench", "基线已写入 bench/baseline.env（提交前逐项披露）");
    Ok(())
}

/// 负对照：判红能力本身要会判红——合成劣化指标必须被比较器拒绝。
fn bench_selftest() -> Result<(), i32> {
    println!("\n[xtask] == D1/D2 selftest（负对照）==");
    let mut base = HashMap::new();
    base.insert("NOISE_TILE_MS".to_string(), 10.0);
    base.insert("AVG_FPS".to_string(), 60.0);
    base.insert("PEAK_RSS_MB".to_string(), 300.0);
    base.insert("PEAK_RSS_MB_MAX".to_string(), 512.0);
    let cases: [(&str, HashMap<String, f64>, bool); 4] = [
        (
            "CPU 劣化 2×",
            HashMap::from([("NOISE_TILE_MS".to_string(), 20.0)]),
            true,
        ),
        (
            "FPS 掉到 40%",
            HashMap::from([("AVG_FPS".to_string(), 24.0)]),
            true,
        ),
        (
            "RSS 超预算",
            HashMap::from([("PEAK_RSS_MB".to_string(), 600.0)]),
            true,
        ),
        (
            "轻微抖动在余量内",
            HashMap::from([("NOISE_TILE_MS".to_string(), 10.5)]),
            false,
        ),
    ];
    for (name, got, must_fail) in cases {
        let mut merged = base.clone();
        merged.extend(got);
        let failed = evaluate_against(&base, &merged);
        if failed.is_empty() == must_fail {
            eprintln!(
                "[xtask] ✗ selftest: {name} 判红能力失效 (failed={:?})",
                failed
            );
            return Err(101);
        }
    }
    // 基线自身必须通过
    if !evaluate_against(&base, &base).is_empty() {
        eprintln!("[xtask] ✗ selftest: 基线过不了自己的检查");
        return Err(101);
    }
    ok("D1/D2 selftest", "四类劣化全判红、正常不误红");
    Ok(())
}

/// 纯比较器（bench_check 与 selftest 共用同一把尺）。
fn evaluate_against(base: &HashMap<String, f64>, got: &HashMap<String, f64>) -> Vec<String> {
    let mut fails = Vec::new();
    for (key, bv) in base {
        let Some(gv) = got.get(key) else { continue };
        match key.as_str() {
            "AVG_FPS" => {
                if *gv < bv * FPS_MARGIN {
                    fails.push(format!("{key} {gv:.2} 低于下限"));
                }
            }
            "PEAK_RSS_MB" => {
                if let Some(max) = base.get("PEAK_RSS_MB_MAX") {
                    if gv > max {
                        fails.push(format!("{key} {gv:.1} 超预算"));
                    }
                }
            }
            "PEAK_RSS_MB_MAX" => {}
            _ => {
                if *gv > bv * CPU_MARGIN {
                    fails.push(format!("{key} {gv:.3} 超余量"));
                }
            }
        }
    }
    fails
}
