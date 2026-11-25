//! hada as an iOS app (iOS 16+), built and started with cargo alone:
//!
//! ```text
//! cargo run -p hada-ios --target aarch64-apple-ios-sim            # simulator
//! cargo run -p hada-ios --target aarch64-apple-ios                # iPhone/iPad
//! cargo run -p hada-ios --target aarch64-apple-ios --release
//! ```
//!
//! Built for iOS, `main` is the app ([`app`]). Built for the Mac, the same `main`
//! is the cargo runner ([`runner`], set in `.cargo/config.toml`): it wraps the iOS
//! executable in an app bundle, signs it, installs it and starts it with its
//! console attached.

/// Bundle identifier and the scene delegate class named in Info.plist.
const BUNDLE_ID: &str = "io.github.baol.hada";
#[allow(dead_code)]
const SCENE_DELEGATE: &str = "HadaSceneDelegate";

#[cfg(target_os = "ios")]
mod app;
#[cfg(target_os = "ios")]
fn main() {
    app::main();
}

#[cfg(not(target_os = "ios"))]
mod runner;
#[cfg(not(target_os = "ios"))]
fn main() -> std::process::ExitCode {
    runner::main()
}
