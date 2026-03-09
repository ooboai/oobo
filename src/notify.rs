use std::fs;

use crate::paths;

/// Send an OS notification with the oobo icon.
pub fn send(title: &str, message: &str) {
    #[cfg(target_os = "macos")]
    {
        let notifier = macos_notifier_path();
        if !notifier.exists() {
            let _ = ensure_macos_notifier();
        }

        if notifier.exists() {
            let result = std::process::Command::new(&notifier)
                .args([title, message])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();

            if result.is_ok_and(|s| s.success()) {
                return;
            }
        }

        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            message.replace('"', "\\\""),
            title.replace('"', "\\\"")
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let icon = paths::oobo_home().join("assets").join("icon.png");
        if !icon.exists() {
            let assets_dir = paths::oobo_home().join("assets");
            let _ = paths::ensure_dir(&assets_dir);
            let _ = fs::write(&icon, ICON_PNG);
        }
        let mut args = vec![title.to_string(), message.to_string()];
        if icon.exists() {
            args = vec![
                "-i".to_string(),
                icon.to_string_lossy().to_string(),
                title.to_string(),
                message.to_string(),
            ];
        }
        let _ = std::process::Command::new("notify-send")
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

#[cfg(target_os = "macos")]
fn macos_notifier_path() -> std::path::PathBuf {
    paths::oobo_home()
        .join("Oobo.app")
        .join("Contents")
        .join("MacOS")
        .join("oobo-notify")
}

#[cfg(target_os = "macos")]
fn ensure_macos_notifier() -> Result<(), String> {
    let app_dir = paths::oobo_home().join("Oobo.app");
    let contents = app_dir.join("Contents");
    let macos_dir = contents.join("MacOS");
    let resources = contents.join("Resources");

    paths::ensure_dir(&macos_dir)?;
    paths::ensure_dir(&resources)?;

    let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>dev.oobo.notifications</string>
    <key>CFBundleName</key>
    <string>oobo</string>
    <key>CFBundleDisplayName</key>
    <string>oobo</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleExecutable</key>
    <string>oobo-notify</string>
    <key>LSUIElement</key>
    <true/>
    <key>CFBundleVersion</key>
    <string>1.0</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
</dict>
</plist>"#;
    fs::write(contents.join("Info.plist"), plist).map_err(|e| format!("write Info.plist: {e}"))?;

    let icon_png = paths::oobo_home().join("assets").join("icon.png");
    if !icon_png.exists() {
        let assets_dir = paths::oobo_home().join("assets");
        paths::ensure_dir(&assets_dir)?;
        fs::write(&icon_png, ICON_PNG).map_err(|e| format!("write icon: {e}"))?;
    }

    let iconset = resources.join("AppIcon.iconset");
    paths::ensure_dir(&iconset)?;

    let sizes = [16, 32, 64, 128, 256, 512];
    for size in sizes {
        let out = iconset.join(format!("icon_{size}x{size}.png"));
        let _ = std::process::Command::new("sips")
            .args([
                "-z",
                &size.to_string(),
                &size.to_string(),
                icon_png.to_str().unwrap_or(""),
                "--out",
                out.to_str().unwrap_or(""),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        if size <= 256 {
            let size2x = size * 2;
            let out2x = iconset.join(format!("icon_{size}x{size}@2x.png"));
            let _ = std::process::Command::new("sips")
                .args([
                    "-z",
                    &size2x.to_string(),
                    &size2x.to_string(),
                    icon_png.to_str().unwrap_or(""),
                    "--out",
                    out2x.to_str().unwrap_or(""),
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    let icns_path = resources.join("AppIcon.icns");
    let status = std::process::Command::new("iconutil")
        .args([
            "-c",
            "icns",
            iconset.to_str().unwrap_or(""),
            "-o",
            icns_path.to_str().unwrap_or(""),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("iconutil failed: {e}"))?;

    if !status.success() {
        return Err("iconutil conversion failed".to_string());
    }

    let _ = fs::remove_dir_all(&iconset);

    let swift_src = macos_dir.join("notify.swift");
    fs::write(
        &swift_src,
        r#"import Cocoa
import UserNotifications

if CommandLine.arguments.count < 2 { exit(0) }

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

let title = CommandLine.arguments[1]
let body = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : ""

let center = UNUserNotificationCenter.current()

let plainCategory = UNNotificationCategory(
    identifier: "PLAIN",
    actions: [],
    intentIdentifiers: [],
    options: [])
center.setNotificationCategories([plainCategory])

func deliver() {
    let content = UNMutableNotificationContent()
    content.title = title
    content.body = body
    content.categoryIdentifier = "PLAIN"
    let req = UNNotificationRequest(
        identifier: UUID().uuidString, content: content, trigger: nil)
    center.add(req) { error in
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            exit(error == nil ? 0 : 1)
        }
    }
}

func denied() {
    DispatchQueue.main.async { exit(1) }
}

center.getNotificationSettings { settings in
    switch settings.authorizationStatus {
    case .notDetermined:
        center.requestAuthorization(options: [.alert, .sound, .badge]) { granted, _ in
            if granted { deliver() }
            else { denied() }
        }
    case .authorized, .provisional, .ephemeral:
        deliver()
    default:
        denied()
    }
}

DispatchQueue.main.asyncAfter(deadline: .now() + 8) { exit(1) }
app.run()
"#,
    )
    .map_err(|e| format!("write swift source: {e}"))?;

    let binary = macos_dir.join("oobo-notify");
    let compile = std::process::Command::new("swiftc")
        .args([
            "-O",
            "-o",
            binary.to_str().unwrap_or(""),
            swift_src.to_str().unwrap_or(""),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("swiftc failed: {e}"))?;

    let _ = fs::remove_file(&swift_src);

    if !compile.success() {
        return Err("Swift compilation failed".to_string());
    }

    let _ = std::process::Command::new("codesign")
        .args([
            "-s",
            "-",
            "--force",
            "--deep",
            app_dir.to_str().unwrap_or(""),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    let _ = std::process::Command::new("/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister")
        .args(["-f", app_dir.to_str().unwrap_or("")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    Ok(())
}

/// Embedded oobo icon (512x512 PNG).
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");
