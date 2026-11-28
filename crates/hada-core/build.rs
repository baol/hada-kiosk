//! Fingerprint of the source that shapes a cached dashboard (the model, and the
//! parsing and normalising that fill it), so a dashboard cache written by a build
//! that made it differently is never used (see `dashboard_cache.rs`).

fn main() {
    // FNV-1a 64: stable across Rust versions, unlike std's hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for file in [
        "src/model.rs",
        "src/lovelace.rs",
        "src/weather.rs",
        "src/normalise.rs",
        "src/domain.rs",
    ] {
        println!("cargo:rerun-if-changed={file}");
        let text = std::fs::read(file).unwrap_or_else(|e| panic!("reading {file}: {e}"));
        for b in text {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    println!("cargo:rustc-env=HADA_MODEL_HASH={hash}");
}
