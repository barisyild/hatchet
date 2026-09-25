//! Compile + run gate for three Haxe forms the parser used to reject:
//! **expression-bodied functions** (`function f():Int return x;`, also with the body on
//! the next line), **several declarators in one `var`** (`var a = 1, b = 2;`, whose later
//! names must stay in the same scope), and **`#if` around imports, members and methods**,
//! decided at transpile time against `-D` flags (nested, `#elseif`, `#else`).
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

const SRC: &str = r#"package lib;

#if fast
import lib.Helper;
#end

class Forms {
	#if fast
	public var mode:Int = 1;
	#elseif slow
	public var mode:Int = 2;
	#else
	public var mode:Int = 3;
	#end

	#if (!fast && !slow)
	public var missing:Int = 99;
	#end

	public function new() {}

	public static inline function twice(a:Int):Int return a * 2;

	public static function unsignedLess(a:Int, b:Int):Bool
		return (a ^ 0x80000000) < (b ^ 0x80000000);

	public function split(a:Int):Int {
		final lo = a & 0xFFFF, hi = a >>> 16;
		var x:Int = 1, y = 2;
		return lo + hi + x + y;
	}

	#if fast
	public function extra():Int {
		#if nested_never
		return -1;
		#end
		return Helper.seven();
	}
	#end
}
"#;

const HELPER: &str = r#"package lib;

class Helper {
	public static function seven():Int return 7;
}
"#;

const MAIN_CPP: &str = r#"#include <stdio.h>
#include "lib/Forms.h"
using namespace lib;
int main() {
	Forms f;
	printf("mode=%d twice=%d ult=%d split=%d extra=%d\n", f.mode, Forms::twice(21),
		Forms::unsignedLess(1, -1) ? 1 : 0, f.split(0x00030004), f.extra());
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
fn parser_forms_compile_and_run() {
    let Some(gxx) = find_gxx() else {
        eprintln!("skipping: no C++ compiler");
        return;
    };
    let root = std::env::temp_dir().join(format!("hatchet_forms_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("Forms.hx"), SRC).unwrap();
    std::fs::write(lib.join("Helper.hx"), HELPER).unwrap();
    let main_cpp = root.join("main.cpp");
    std::fs::write(&main_cpp, MAIN_CPP).unwrap();
    let out = root.join("out");
    let gen = Command::new(env!("CARGO_BIN_EXE_hatchet"))
        .arg("--src")
        .arg(&lib)
        .arg("--out")
        .arg(&out)
        .arg("--force")
        .arg("-D")
        .arg("fast")
        .output()
        .expect("run hatchet");
    assert!(
        gen.status.success(),
        "transpiling failed:\n{}{}",
        String::from_utf8_lossy(&gen.stdout),
        String::from_utf8_lossy(&gen.stderr)
    );
    let header = std::fs::read_to_string(out.join("lib").join("Forms.h")).unwrap();
    assert!(
        !header.contains("missing"),
        "an #if branch that does not hold is dropped:\n{header}"
    );
    let exe = out.join(if cfg!(windows) { "forms.exe" } else { "forms" });
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
    // split(0x00030004): lo 4 + hi 3 + 1 + 2 = 10. unsignedLess(1, -1): 1 < 0xFFFFFFFF.
    assert_eq!(
        stdout.trim(),
        "mode=1 twice=42 ult=1 split=10 extra=7",
        "got: {stdout}"
    );
}
