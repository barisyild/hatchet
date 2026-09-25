//! Compile + run gate for the forms a real program (recompsx's PlayStation runtime) needed
//! beyond the language core, each of which used to fail to compile or compile wrongly:
//! - a Haxe name that is a C++ keyword (`register`, `char`) is renamed everywhere;
//! - `@:headerCode("#include \"x.h\"")` carries the Haxe string's value, not its escapes;
//! - `new Map()` with its type parameters inferred takes them from the field it initialises;
//! - a `static final` container stays mutable (`final` fixes the binding, not the value);
//! - a static field assigned `new` in a static function is never freed through `this`;
//! - a value `Array` compared to `null` reads as empty (it is null until assigned in Haxe);
//! - a static without an initialiser, of any type, is value-initialised;
//! - a literal of integer constants (a jump table) is a `static const int[]`, not a
//!   `push_back` per element (which cost a large generated program megabytes of code).
//!
//! Skipped (passes vacuously) when no C++ compiler is available.

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

const SRC: &str = r##"package lib;

@:headerCode("#include \"forms_extra.h\"")
class Box {
	public var v:Int = 1;
	public function new() {}
}

class Forms {
	static final seen:Map<Int, Bool> = new Map();
	static var saved:Box;
	static var counts:Array<Int>;
	static var table:Array<Int>;
	static final JUMPS:Array<Int> = [1, 2, 3, 4, 5, 6, 7, 8, -9, 0x80000000];

	public static function register(char:Int):Int {
		return char + EXTRA_BIAS;
	}

	public static function run():Int {
		saved = new Box();
		saved = new Box();
		seen.set(3, true);
		seen.set(4, true);
		var hits = 0;
		if (counts != null) counts[0]++;
		else hits += 100;
		counts = [for (_ in 0...4) 0];
		if (counts != null) counts[0]++;
		else {}
		if (seen.exists(3)) hits += 10;
		else {}
		var j = 0;
		for (v in JUMPS) j += v;
		return register(hits) + counts[0] + saved.v + table.length + j;
	}
}
"##;

const EXTRA_H: &str = "#define EXTRA_BIAS 1000\n";

const MAIN_CPP: &str = r#"#include <stdio.h>
#include "lib/Forms.h"
using namespace lib;
int main() {
	printf("run=%d\n", Forms::run());
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
fn runtime_forms_compile_and_run() {
    let Some(gxx) = find_gxx() else {
        eprintln!("skipping: no C++ compiler");
        return;
    };
    let root = std::env::temp_dir().join(format!("hatchet_rtforms_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("Forms.hx"), SRC).unwrap();
    let main_cpp = root.join("main.cpp");
    std::fs::write(&main_cpp, MAIN_CPP).unwrap();
    let inc = root.join("inc");
    std::fs::create_dir_all(&inc).unwrap();
    std::fs::write(inc.join("forms_extra.h"), EXTRA_H).unwrap();
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
        "transpiling failed:\n{}{}",
        String::from_utf8_lossy(&gen.stdout),
        String::from_utf8_lossy(&gen.stderr)
    );
    let source = std::fs::read_to_string(out.join("lib").join("Forms.cpp")).unwrap();
    assert!(
        !source.contains("this->saved"),
        "a static is never freed through `this`:\n{source}"
    );
    assert!(
        source.contains("_data[] = {") && !source.contains("push_back(8)"),
        "an integer-constant literal is a data table:\n{source}"
    );
    let exe = out.join(if cfg!(windows) {
        "rtforms.exe"
    } else {
        "rtforms"
    });
    let mut cmd = Command::new(&gxx);
    cmd.args(["-std=c++98", "-pedantic", "-Wall"])
        .arg("-I")
        .arg(&out)
        .arg("-I")
        .arg(&inc)
        .arg(&main_cpp);
    for f in cpp_files(&out) {
        cmd.arg(f);
    }
    cmd.arg("-o").arg(&exe);
    let compile = cmd.output().expect("run g++");
    assert!(
        compile.status.success(),
        "did not compile under g++ -std=c++98:\n{}\n--- Forms.cpp ---\n{source}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&exe).output().expect("run the demo");
    let stdout = String::from_utf8_lossy(&run.stdout);
    // hits = 100 (counts null) + 10 (seen has 3) = 110; register adds 1000; counts[0] is 1
    // after the second guard; saved.v is 1; table is empty; JUMPS sums to 27 - 2^31, and the
    // total wraps: 1112 + 27 - 2147483648 = -2147482509.
    assert_eq!(stdout.trim(), "run=-2147482509", "got: {stdout}");
}
