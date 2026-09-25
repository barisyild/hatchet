//! Compile + run gate for **package-qualified type references in expressions**:
//! `util.MathOps.twice(4)` and `util.MathOps.K` with no `import`, which Haxe allows.
//! They used to be emitted segment by segment with dots (`util.MathOps.twice(4)`),
//! which is not C++, and the defining header was never included. A parameter that
//! happens to share the package's name must still read as the parameter.
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

const MATHOPS: &str = r#"package util;

class MathOps {
	public static inline var K:Int = 3;
	public static var calls:Int = 0;

	public static function twice(x:Int):Int {
		calls++;
		return x * 2;
	}
}
"#;

const RUNNER: &str = r#"package app;

class Runner {
	public function new() {}

	public function run():Int {
		var a:Int = util.MathOps.twice(4);
		var b:Int = util.MathOps.K;
		util.MathOps.calls += 10;
		return a * 100 + b * 10 + util.MathOps.calls;
	}

	// `util` here is the parameter, not the package.
	public function shadowed(util:Int):Int {
		return util + 1;
	}
}
"#;

const MAIN_CPP: &str = r#"#include <stdio.h>
#include "app/Runner.h"
int main() {
	app::Runner m;
	printf("run=%d shadowed=%d\n", m.run(), m.shadowed(41));
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
fn qualified_type_paths_compile_and_run() {
    let Some(gxx) = find_gxx() else {
        eprintln!("skipping: no C++ compiler");
        return;
    };
    let root = std::env::temp_dir().join(format!("hatchet_qualified_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("src");
    std::fs::create_dir_all(src.join("util")).unwrap();
    std::fs::create_dir_all(src.join("app")).unwrap();
    std::fs::write(src.join("util").join("MathOps.hx"), MATHOPS).unwrap();
    std::fs::write(src.join("app").join("Runner.hx"), RUNNER).unwrap();
    let main_cpp = root.join("main.cpp");
    std::fs::write(&main_cpp, MAIN_CPP).unwrap();
    let out = root.join("out");
    let gen = Command::new(env!("CARGO_BIN_EXE_hatchet"))
        .arg("--src").arg(&src).arg("--out").arg(&out).arg("--force")
        .output().expect("run hatchet");
    assert!(
        gen.status.success(),
        "transpiling the qualified-path demo failed:\n{}{}",
        String::from_utf8_lossy(&gen.stdout),
        String::from_utf8_lossy(&gen.stderr)
    );
    let main_src = std::fs::read_to_string(out.join("app").join("Runner.cpp")).unwrap();
    assert!(
        !main_src.contains("util.MathOps"),
        "a qualified type path must not survive as a dotted member chain:\n{main_src}"
    );
    let exe = out.join(if cfg!(windows) { "qualified.exe" } else { "qualified" });
    let mut cmd = Command::new(&gxx);
    cmd.args(["-std=c++98", "-pedantic", "-Wall"]).arg("-I").arg(&out).arg(&main_cpp);
    for f in cpp_files(&out) {
        cmd.arg(f);
    }
    cmd.arg("-o").arg(&exe);
    let compile = cmd.output().expect("run g++");
    assert!(
        compile.status.success(),
        "the qualified-path demo did not compile under g++ -std=c++98:\n{}\n--- Runner.cpp ---\n{main_src}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&exe).output().expect("run the demo");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout.trim(), "run=841 shadowed=42", "got: {stdout}");
}
