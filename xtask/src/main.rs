//! 门禁编排入口。`cargo xtask gate` = 本地与 CI 同一把尺（06 文档）。
//!
//! M0 接线 A1(fmt)/A2(clippy)/A3(build+test 基线)；parity/golden/mutants 等
//! 判定门在对应里程碑（M1–M2）建立时挂进来。jobs=1 是本机内存约束下的硬默认。

use std::env;
use std::process::{exit, Command};

/// core 三 crate 另开 pedantic -D warnings（06 A2）。
const CORE_CRATES: &[&str] = &["strata-geo", "strata-exec", "strata-nodes"];

fn main() {
    let arg = env::args().nth(1);
    let result = match arg.as_deref() {
        Some("gate") | None => gate(),
        Some("gate-quick") => gate_quick(),
        Some(other) => {
            eprintln!("unknown xtask subcommand: {other}");
            eprintln!("usage: cargo xtask [gate|gate-quick]");
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
