//! Compile + run gate for **function-typed values holding static functions**: a
//! `Null<Int -> Ctx -> Bool>` static set from `Table.call` and called through, the shape
//! a program uses to hand a dispatcher to a library compiled without it. The function type
//! lowers to a C function pointer (`hx_fn<bool(int, Ctx*) >::fn*`), the reference to
//! `&Table::call`, the call to a plain pointer call; `null` is `NULL`, and nothing is boxed
//! or freed.
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

class Ctx {
	public var n:Int = 0;
	public function new() {}
}

class Table {
	public static function call(addr:Int, ctx:Ctx):Bool {
		ctx.n += addr;
		return addr > 0;
	}
}

class Disp {
	static var dispatcher:Null<Int -> Ctx -> Bool> = null;
	public static function bind(f:Int -> Ctx -> Bool):Void {
		dispatcher = f;
	}
	public static function run(addr:Int, ctx:Ctx):Bool {
		final d = dispatcher;
		if (d == null) return false;
		else {}
		return d(addr, ctx);
	}
	public static function setup():Void {
		bind(Table.call);
	}
	public static function direct(ctx:Ctx):Bool {
		return Table.call(5, ctx);
	}
}
"#;

const MAIN_CPP: &str = r#"#include <stdio.h>
#include "lib/Disp.h"
using namespace lib;
int main() {
	Ctx* c = new Ctx();
	int before = Disp::run(3, c) ? 1 : 0;
	Disp::setup();
	int r = Disp::run(3, c) ? 1 : 0;
	int n1 = c->n;
	int dr = Disp::direct(c) ? 1 : 0;
	printf("before=%d r=%d n=%d direct=%d n=%d\n", before, r, n1, dr, c->n);
	delete c;
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
fn static_function_pointers_compile_and_run() {
    let Some(gxx) = find_gxx() else {
        eprintln!("skipping: no C++ compiler");
        return;
    };
    let root = std::env::temp_dir().join(format!("hatchet_fnptr_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("Disp.hx"), SRC).unwrap();
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
        "transpiling failed:\n{}{}",
        String::from_utf8_lossy(&gen.stdout),
        String::from_utf8_lossy(&gen.stderr)
    );
    let source = std::fs::read_to_string(out.join("lib").join("Disp.cpp")).unwrap();
    assert!(
        source.contains("bind(&Table::call);"),
        "a static method as a value is its address:\n{source}"
    );
    assert!(
        !source.contains("new hx_fn") && !source.contains("delete d"),
        "a function pointer is never boxed or freed:\n{source}"
    );
    let exe = out.join(if cfg!(windows) { "fnptr.exe" } else { "fnptr" });
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
    assert_eq!(
        stdout.trim(),
        "before=0 r=1 n=3 direct=1 n=8",
        "got: {stdout}"
    );
}
