//! Compiles the Slint UI and generates the Material Design Icons codepoint table
//! from the `@mdi/font` SCSS variables file.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    // --- git commit, shown by --version and logged at startup ---------------
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&manifest)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let hash = git(&["rev-parse", "--short=10", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    println!(
        "cargo:rustc-env=HADA_GIT_HASH={hash}{}",
        if dirty { "-dirty" } else { "" }
    );
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
        if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={git_dir}/{r}");
        }
    }

    // --- MDI codepoints ------------------------------------------------------
    let scss_path = manifest.join("assets/mdi_variables.scss");
    println!("cargo:rerun-if-changed={}", scss_path.display());
    let scss = fs::read_to_string(&scss_path).expect("assets/mdi_variables.scss missing");
    let mut entries: Vec<(String, u32)> = Vec::new();
    let mut in_map = false;
    for line in scss.lines() {
        let line = line.trim();
        if line.starts_with("$mdi-icons:") {
            in_map = true;
            continue;
        }
        if !in_map {
            continue;
        }
        if line.starts_with(')') {
            break;
        }
        // "ab-testing": F01C9,
        let Some((name, code)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().trim_matches('"');
        let code = code.trim().trim_end_matches(',').trim();
        if let Ok(cp) = u32::from_str_radix(code, 16) {
            entries.push((name.to_string(), cp));
        }
    }
    assert!(
        entries.len() > 7000,
        "MDI table looks truncated: {} entries",
        entries.len()
    );
    entries.sort();
    entries.dedup_by(|a, b| a.0 == b.0);
    let mut src = String::with_capacity(entries.len() * 32);
    src.push_str("/// Sorted `(icon name, codepoint)` pairs generated from @mdi/font.\n");
    src.push_str("pub static MDI_CODEPOINTS: &[(&str, u32)] = &[\n");
    for (name, cp) in &entries {
        src.push_str(&format!("    ({name:?}, 0x{cp:X}),\n"));
    }
    src.push_str("];\n");
    fs::write(out.join("mdi_codepoints.rs"), src).unwrap();

    // --- Slint --------------------------------------------------------------
    let ui = manifest.join("ui/app.slint");
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config(ui, config).expect("Slint compilation failed");
}
