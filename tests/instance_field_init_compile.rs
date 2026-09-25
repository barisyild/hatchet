//! Compile + run gate for **instance field initialisers**. `var x:Int = 7;` runs at the
//! start of the constructor in Haxe; C++98 has no in-class member initialisers, and the
//! initialisers used to be dropped, leaving the fields uninitialised. A subclass's own
//! initialisers run after its `super(...)`, which sees the base's already set.
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

class Base {
	public var a:Int = 7;
	public var b = 8;
	public var label:String = "base";
	public function new() {}
}

class Derived extends Base {
	public var c:Int = 40;
	public var sum:Int = 0;
	public function new() {
		super();
		sum = a + b + c;
	}
}
"#;

const MAIN_CPP: &str = r#"#include <stdio.h>
#include "lib/Base.h"
using namespace lib;
int main() {
	Derived* d = new Derived();
	printf("a=%d b=%d c=%d sum=%d label=%s\n", d->a, d->b, d->c, d->sum, d->label.c_str());
	delete d;
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
fn instance_field_initialisers_run_in_the_constructor() {
    let Some(gxx) = find_gxx() else {
        eprintln!("skipping: no C++ compiler");
        return;
    };
    let root = std::env::temp_dir().join(format!("hatchet_instinit_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("Base.hx"), SRC).unwrap();
    let main_cpp = root.join("main.cpp");
    std::fs::write(&main_cpp, MAIN_CPP).unwrap();
    let out = root.join("out");
    let gen = Command::new(env!("CARGO_BIN_EXE_hatchet"))
        .arg("--src").arg(&lib).arg("--out").arg(&out).arg("--force")
        .output().expect("run hatchet");
    assert!(gen.status.success(), "transpiling failed:\n{}", String::from_utf8_lossy(&gen.stderr));
    let exe = out.join(if cfg!(windows) { "instinit.exe" } else { "instinit" });
    let mut cmd = Command::new(&gxx);
    cmd.args(["-std=c++98", "-pedantic", "-Wall"]).arg("-I").arg(&out).arg(&main_cpp);
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
    assert_eq!(stdout.trim(), "a=7 b=8 c=40 sum=55 label=base", "got: {stdout}");
}
