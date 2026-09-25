//! Compile + run gate for **hex `Int` literals in the negative range**.
//!
//! Haxe's `Int` is 32 bits, so `0x80000000` is `-2147483648` and `0xFFFFFFFF` is `-1`.
//! In C++ those spellings are `unsigned int` literals, and the unsignedness spreads into
//! every expression around them: comparisons become unsigned, `>>` shifts in zeros. The
//! values printed here are what Haxe (and hxcpp, and the JS target) compute; before the
//! fix the unsigned compare and the shift both differed, silently.
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

class Bits {
	public static inline var SIGN:Int = 0x80000000;
	public function new() {}

	// The unsigned-compare idiom: flip the sign bit, then compare signed.
	public function unsignedLess(a:Int, b:Int):Int {
		return (a ^ 0x80000000) < (b ^ 0x80000000) ? 1 : 0;
	}

	public function shifted():Int {
		return 0xFFFFFFFF >> 4;
	}

	public function negative():Int {
		var x:Int = 0xFFFF0000;
		return x < 0 ? 1 : 0;
	}

	public function which(v:Int):Int {
		switch (v) {
			case 0x80000000:
				return 1;
			default:
				return 0;
		}
	}

	public function viaConstant(a:Int):Int {
		return (a & SIGN) != 0 ? 1 : 0;
	}
}
"#;

const MAIN_CPP: &str = r#"#include <stdio.h>
#include "lib/Bits.h"
using namespace lib;
int main() {
	Bits b;
	printf("ult=%d,%d shr=%d neg=%d case=%d const=%d\n",
		b.unsignedLess(0, 1), b.unsignedLess(1, (int)0x80000000),
		b.shifted(), b.negative(), b.which((int)0x80000000), b.viaConstant(-5));
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
fn negative_hex_literals_are_emitted_as_int() {
    let out = common::gen_one(SRC, "Bits");
    assert!(
        out.contains("((int)0x80000000)"),
        "a hex literal in the negative Int range must be cast to int:\n{out}"
    );
    assert!(
        out.contains("((int)0xFFFFFFFF)"),
        "0xFFFFFFFF is the Int -1, not an unsigned literal:\n{out}"
    );
}

#[test]
fn negative_hex_literals_keep_haxe_semantics_at_run_time() {
    let Some(gxx) = find_gxx() else {
        eprintln!("skipping: no C++ compiler");
        return;
    };
    let root = std::env::temp_dir().join(format!("hatchet_signedhex_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("Bits.hx"), SRC).unwrap();
    let main_cpp = root.join("main.cpp");
    std::fs::write(&main_cpp, MAIN_CPP).unwrap();
    let out = root.join("out");
    let gen_ok = Command::new(env!("CARGO_BIN_EXE_hatchet"))
        .arg("--src")
        .arg(&lib)
        .arg("--out")
        .arg(&out)
        .arg("--force")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(gen_ok, "transpiling the signed-hex demo failed");
    let exe = out.join(if cfg!(windows) {
        "signedhex.exe"
    } else {
        "signedhex"
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
        "the signed-hex demo did not compile under g++ -std=c++98:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&exe).output().expect("run the demo");
    let stdout = String::from_utf8_lossy(&run.stdout);
    // Haxe: (0^S) < (1^S) is S < S+1 -> true; (1^S) < (S^S) is S+1 < 0 -> true;
    // -1 >> 4 is -1; 0xFFFF0000 is negative; the case matches; -5 has the sign bit.
    assert_eq!(
        stdout.trim(),
        "ult=1,1 shr=-1 neg=1 case=1 const=1",
        "got: {stdout}"
    );
}
