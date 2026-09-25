//! Compile + run gate for **field types inferred from initialisers** and **integer
//! constants folded**. Every field below is unannotated, as Haxe allows; they used to
//! come out as `void*`. Constants defined from other constants fold to literals with
//! Haxe's 32-bit semantics and become C++ integral constants the compiler can fold;
//! a mutable static initialised from a constant is a plain static, not an accessor.
//!
//! Skipped (passes vacuously) when no C++ compiler is available.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn find_gxx() -> Option<String> {
    let candidates = [
        std::env::var("HATCHET_GXX").ok(),
        Some("g++".to_string()),
        Some(r"C:\msys64\mingw32\bin\g++.exe".to_string()),
    ];
    candidates.into_iter().flatten().find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

const SRC: &str = r#"package lib;

class Consts {
	public static inline var SIZE = 0x100;
	public static inline var MASK = SIZE - 1;
	public static inline var SIGN = 0x80000000;
	public static final TOP = SIGN >>> 28;
	public static var counter = MASK;
	public static final TABLE = [1, 2, 3];
	public var inst = 7;

	public function new() {}

	public function sum():Int {
		var t = 0;
		for (v in TABLE) t += v;
		return t;
	}

	public function run():Int {
		counter += 1;
		var m = MASK;
		return m + TOP + counter + inst + sum();
	}
}
"#;

const MAIN_CPP: &str = r#"#include <stdio.h>
#include "lib/Consts.h"
using namespace lib;
int main() {
	Consts c;
	printf("run=%d sign=%d\n", c.run(), Consts::SIGN);
	return 0;
}
"#;

fn cpp_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(cpp_files(&p));
            } else if p.extension().and_then(|s| s.to_str()) == Some("cpp") {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn unannotated_fields_are_typed_and_constants_folded() {
    let header = common::gen_header(SRC, "Consts");
    for want in [
        "static const int SIZE = 0x100;",
        "static const int MASK = 255;",
        "static const int SIGN = ((int)0x80000000);",
        "static const int TOP = 8;",
        "static int counter;",
        "int inst;",
    ] {
        assert!(
            header.contains(want),
            "expected `{want}` in the header:\n{header}"
        );
    }
    assert!(
        !header.contains("void*"),
        "no field may fall back to void*:\n{header}"
    );
    assert!(
        header.contains("std::vector<int>"),
        "an Int array literal types the field as Array<Int>:\n{header}"
    );
}

#[test]
fn unannotated_fields_compile_and_run() {
    let Some(gxx) = find_gxx() else {
        eprintln!("skipping: no C++ compiler");
        return;
    };
    let root = std::env::temp_dir().join(format!("hatchet_fieldinfer_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("Consts.hx"), SRC).unwrap();
    let main_cpp = root.join("main.cpp");
    std::fs::write(&main_cpp, MAIN_CPP).unwrap();
    let out = root.join("out");
    let gen = Command::new(env!("CARGO_BIN_EXE_hatchet"))
        .arg("--src")
        .arg(&lib)
        .arg("--out")
        .arg(&out)
        .arg("--force")
        .output()
        .expect("run hatchet");
    assert!(
        gen.status.success(),
        "transpiling failed:\n{}",
        String::from_utf8_lossy(&gen.stderr)
    );
    let exe = out.join(if cfg!(windows) {
        "fieldinfer.exe"
    } else {
        "fieldinfer"
    });
    let mut cmd = Command::new(&gxx);
    cmd.args(["-std=c++98", "-pedantic", "-Wall"])
        .arg("-I")
        .arg(&out)
        .arg(&main_cpp);
    for f in cpp_files(&out) {
        cmd.arg(f);
    }
    cmd.arg("-o").arg(&exe);
    let compile = cmd.output().expect("run g++");
    assert!(
        compile.status.success(),
        "did not compile under g++ -std=c++98:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&exe).output().expect("run the demo");
    let stdout = String::from_utf8_lossy(&run.stdout);
    // 255 + 8 + 256 + 7 + 6
    assert_eq!(stdout.trim(), "run=532 sign=-2147483648", "got: {stdout}");
}
