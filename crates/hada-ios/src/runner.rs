//! The cargo runner for the iOS builds (`.cargo/config.toml`): cargo hands it the
//! iOS executable; it wraps it in `Hada.app` next to it, signs it, installs it and
//! starts it with its console attached, so `cargo run` shows the app's log.
//!
//! Simulator (`aarch64-apple-ios-sim`): ad-hoc signed, installed on the booted
//! simulator, or `HADA_SIM` (name or UDID).
//!
//! Device (`aarch64-apple-ios`): signed with the newest valid provisioning profile
//! for the bundle ID that Xcode has downloaded, and the matching certificate;
//! installed on the first paired iPhone (then iPad), or `HADA_DEVICE` (name or ID).

use crate::{BUNDLE_ID, SCENE_DELEGATE};
use plist::{Dictionary, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::SystemTime;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

const ICONS: &[(&str, &[u8])] = &[
    ("AppIcon60x60@2x.png", include_bytes!("../res/icon-120.png")),
    ("AppIcon60x60@3x.png", include_bytes!("../res/icon-180.png")),
    (
        "AppIcon76x76@2x~ipad.png",
        include_bytes!("../res/icon-152.png"),
    ),
    (
        "AppIcon83.5x83.5@2x~ipad.png",
        include_bytes!("../res/icon-167.png"),
    ),
];

pub fn main() -> ExitCode {
    let Some(exe) = std::env::args_os().nth(1) else {
        eprintln!(
            "This is the cargo runner of the iOS app. Run it with\n  \
             cargo run -p hada-ios --target aarch64-apple-ios-sim   (simulator)\n  \
             cargo run -p hada-ios --target aarch64-apple-ios       (iPhone/iPad)"
        );
        return ExitCode::from(2);
    };
    match run(Path::new(&exe)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(exe: &Path) -> Res<ExitCode> {
    let in_dir = |name: &str| exe.components().any(|c| c.as_os_str() == name);
    let device = if in_dir("aarch64-apple-ios") {
        true
    } else if in_dir("aarch64-apple-ios-sim") {
        false
    } else {
        return Err(format!("{} is not an aarch64-apple-ios(-sim) build", exe.display()).into());
    };
    let app = exe.with_file_name("Hada.app");
    bundle(exe, &app, device)?;
    let status = if device {
        let profile = find_profile()?;
        eprintln!(
            "signing with profile {:?} (team {})",
            profile.name, profile.team
        );
        fs::copy(&profile.path, app.join("embedded.mobileprovision"))?;
        let entitlements = exe.with_file_name("hada.entitlements");
        profile.entitlements.to_file_xml(&entitlements)?;
        let identity = signing_identity(&profile)?;
        sign(&app, &identity, Some(&entitlements))?;
        let target = match std::env::var("HADA_DEVICE") {
            Ok(d) if !d.is_empty() => d,
            _ => find_device(exe)?,
        };
        check(xcrun(&["devicectl", "device", "install", "app", "--device", &target]).arg(&app))?;
        xcrun(&[
            "devicectl",
            "device",
            "process",
            "launch",
            "--device",
            &target,
            "--terminate-existing",
            "--console",
            BUNDLE_ID,
        ])
        .status()?
    } else {
        sign(&app, "-", None)?;
        let sim = std::env::var("HADA_SIM").unwrap_or_else(|_| "booted".into());
        check(xcrun(&["simctl", "install", &sim]).arg(&app))?;
        xcrun(&[
            "simctl",
            "launch",
            "--console-pty",
            "--terminate-running-process",
            &sim,
            BUNDLE_ID,
        ])
        .status()?
    };
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn xcrun(args: &[&str]) -> Command {
    let mut c = Command::new("xcrun");
    c.args(args);
    c
}

fn check(cmd: &mut Command) -> Res<()> {
    let status = cmd.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd:?} failed ({status})").into())
    }
}

fn dict(pairs: Vec<(&str, Value)>) -> Value {
    Value::Dictionary(
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<Dictionary>(),
    )
}

fn strings(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|s| Value::from(*s)).collect())
}

fn bundle(exe: &Path, app: &Path, device: bool) -> Res<()> {
    if app.exists() {
        fs::remove_dir_all(app)?;
    }
    fs::create_dir_all(app)?;
    fs::copy(exe, app.join("hada"))?;
    for (name, bytes) in ICONS {
        fs::write(app.join(name), bytes)?;
    }
    let version = env!("CARGO_PKG_VERSION");
    let orientations = strings(&[
        "UIInterfaceOrientationPortrait",
        "UIInterfaceOrientationPortraitUpsideDown",
        "UIInterfaceOrientationLandscapeLeft",
        "UIInterfaceOrientationLandscapeRight",
    ]);
    let info = dict(vec![
        ("CFBundleDevelopmentRegion", "en".into()),
        ("CFBundleExecutable", "hada".into()),
        ("CFBundleIdentifier", BUNDLE_ID.into()),
        ("CFBundleName", "hada".into()),
        ("CFBundleDisplayName", "hada".into()),
        ("CFBundlePackageType", "APPL".into()),
        ("CFBundleInfoDictionaryVersion", "6.0".into()),
        ("CFBundleShortVersionString", version.into()),
        ("CFBundleVersion", version.into()),
        (
            "CFBundleSupportedPlatforms",
            strings(&[if device {
                "iPhoneOS"
            } else {
                "iPhoneSimulator"
            }]),
        ),
        ("MinimumOSVersion", "16.0".into()),
        ("UIDeviceFamily", Value::Array(vec![1.into(), 2.into()])),
        (
            "CFBundleIcons",
            dict(vec![(
                "CFBundlePrimaryIcon",
                dict(vec![("CFBundleIconFiles", strings(&["AppIcon60x60"]))]),
            )]),
        ),
        (
            "CFBundleIcons~ipad",
            dict(vec![(
                "CFBundlePrimaryIcon",
                dict(vec![(
                    "CFBundleIconFiles",
                    strings(&["AppIcon60x60", "AppIcon76x76", "AppIcon83.5x83.5"]),
                )]),
            )]),
        ),
        ("UILaunchScreen", dict(vec![])),
        ("UIRequiresFullScreen", true.into()),
        ("UIStatusBarHidden", true.into()),
        ("UIViewControllerBasedStatusBarAppearance", false.into()),
        ("UISupportedInterfaceOrientations", orientations.clone()),
        ("UISupportedInterfaceOrientations~ipad", orientations),
        (
            "NSLocalNetworkUsageDescription",
            "hada connects to Home Assistant on your local network.".into(),
        ),
        (
            "UIApplicationSceneManifest",
            dict(vec![
                ("UIApplicationSupportsMultipleScenes", false.into()),
                (
                    "UISceneConfigurations",
                    dict(vec![(
                        "UIWindowSceneSessionRoleApplication",
                        Value::Array(vec![dict(vec![
                            ("UISceneConfigurationName", "Default".into()),
                            ("UISceneDelegateClassName", SCENE_DELEGATE.into()),
                        ])]),
                    )]),
                ),
            ]),
        ),
    ]);
    info.to_file_xml(app.join("Info.plist"))?;
    Ok(())
}

fn sign(app: &Path, identity: &str, entitlements: Option<&Path>) -> Res<()> {
    let mut cmd = Command::new("codesign");
    cmd.args(["--force", "--timestamp=none", "--sign", identity]);
    if let Some(e) = entitlements {
        cmd.arg("--entitlements").arg(e);
    }
    check(cmd.arg(app))
}

struct Profile {
    path: PathBuf,
    name: String,
    team: String,
    entitlements: Value,
    /// SHA-1 of the certificates it accepts, upper-case hex.
    certificates: Vec<String>,
}

/// The newest unexpired profile for BUNDLE_ID (an exact App ID before a wildcard).
fn find_profile() -> Res<Profile> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let dirs = [
        home.join("Library/Developer/Xcode/UserData/Provisioning Profiles"),
        home.join("Library/MobileDevice/Provisioning Profiles"),
    ];
    let now = SystemTime::now();
    let mut best: Option<(bool, SystemTime, Profile)> = None;
    for path in dirs
        .iter()
        .filter_map(|d| fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .map(|e| e.path())
    {
        if path.extension().is_none_or(|e| e != "mobileprovision") {
            continue;
        }
        let Ok(out) = Command::new("security")
            .args(["cms", "-D", "-i"])
            .arg(&path)
            .output()
        else {
            continue;
        };
        let Ok(Value::Dictionary(d)) = plist::from_bytes::<Value>(&out.stdout) else {
            continue;
        };
        let team = d
            .get("TeamIdentifier")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_string)
            .unwrap_or_default()
            .to_string();
        let entitlements = d
            .get("Entitlements")
            .cloned()
            .unwrap_or(Value::Dictionary(Dictionary::new()));
        let app_id = entitlements
            .as_dictionary()
            .and_then(|e| e.get("application-identifier"))
            .and_then(Value::as_string)
            .unwrap_or_default();
        let exact = app_id == format!("{team}.{BUNDLE_ID}");
        if !exact && app_id != format!("{team}.*") {
            continue;
        }
        let Some(expires) = d
            .get("ExpirationDate")
            .and_then(Value::as_date)
            .map(SystemTime::from)
        else {
            continue;
        };
        if expires <= now {
            continue;
        }
        let certificates = d
            .get("DeveloperCertificates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_data)
            .map(|der| {
                sha1_smol::Sha1::from(der)
                    .digest()
                    .to_string()
                    .to_uppercase()
            })
            .collect();
        let name = d
            .get("Name")
            .and_then(Value::as_string)
            .unwrap_or_default()
            .to_string();
        let better = best
            .as_ref()
            .is_none_or(|(e, t, _)| (exact, expires) > (*e, *t));
        if better {
            best = Some((
                exact,
                expires,
                Profile {
                    path,
                    name,
                    team,
                    entitlements,
                    certificates,
                },
            ));
        }
    }
    best.map(|(_, _, p)| p).ok_or_else(|| {
        format!(
            "no valid provisioning profile for {BUNDLE_ID}. Free (Personal Team) profiles last \
             7 days. To get one, make an iOS app in Xcode with bundle ID {BUNDLE_ID} and your \
             team, and run it once on the device."
        )
        .into()
    })
}

/// A code-signing identity in the keychain whose certificate the profile accepts.
fn signing_identity(profile: &Profile) -> Res<String> {
    let out = Command::new("security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .find(|hash| profile.certificates.iter().any(|c| c == hash))
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "no signing certificate in the keychain matches profile {:?}",
                profile.name
            )
            .into()
        })
}

/// The first paired physical iPhone, else iPad.
fn find_device(exe: &Path) -> Res<String> {
    let json = exe.with_file_name("devices.json");
    check(xcrun(&["devicectl", "list", "devices", "--quiet", "--json-output"]).arg(&json))?;
    let v: serde_json::Value = serde_json::from_slice(&fs::read(&json)?)?;
    let devices = v["result"]["devices"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let pick = |kind: &str| {
        devices.iter().find(|d| {
            d["hardwareProperties"]["reality"] == "physical"
                && d["hardwareProperties"]["deviceType"] == kind
                && d["connectionProperties"]["pairingState"] == "paired"
        })
    };
    let d = pick("iPhone")
        .or_else(|| pick("iPad"))
        .ok_or("no paired iPhone or iPad found; set HADA_DEVICE")?;
    eprintln!(
        "device {}",
        d["deviceProperties"]["name"].as_str().unwrap_or("?")
    );
    Ok(d["identifier"]
        .as_str()
        .ok_or("device without identifier")?
        .to_string())
}
