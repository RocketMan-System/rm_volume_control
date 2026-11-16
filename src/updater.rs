//! Self-updater backed by GitHub Releases.
//!
//! Every release carries one asset per platform (see [`ASSET_NAME`]) and a
//! `SHA256SUMS.txt`. The new binary is downloaded next to the temp dir, verified,
//! swapped with the running one, and the app restarts itself.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const REPO: &str = "RocketMan-System/rm_volume_control";
pub const RELEASES_PAGE: &str = "https://github.com/RocketMan-System/rm_volume_control/releases/latest";
const CHECKSUMS: &str = "SHA256SUMS.txt";
/// Passed to the relaunched process so it waits for the old one to exit.
pub const UPDATED_ARG: &str = "--updated";

#[cfg(all(windows, target_arch = "x86_64"))]
pub const ASSET_NAME: &str = "apps-volume-control-windows-x86_64.exe";
#[cfg(all(windows, target_arch = "aarch64"))]
pub const ASSET_NAME: &str = "apps-volume-control-windows-aarch64.exe";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const ASSET_NAME: &str = "apps-volume-control-linux-x86_64";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub const ASSET_NAME: &str = "apps-volume-control-linux-aarch64";
#[cfg(target_os = "macos")]
pub const ASSET_NAME: &str = "apps-volume-control-macos-universal.zip";

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub asset_url: Option<String>,
    pub checksums_url: Option<String>,
}

/// Parses `v1.2.3` / `1.2.3-rc.1` into comparable numbers (pre-release suffix ignored).
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>());
    let major = parts.next()?.ok()?;
    let minor = parts.next().unwrap_or(Ok(0)).ok()?;
    let patch = parts.next().unwrap_or(Ok(0)).ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(remote: &str, current: &str) -> bool {
    match (parse_version(remote), parse_version(current)) {
        (Some(r), Some(c)) => r > c,
        _ => false,
    }
}

/// Finds the hash for `file` in `sha256sum` output (`<hex>  <name>` or `<hex> *<name>`).
pub fn find_checksum(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut it = line.split_whitespace();
        let hash = it.next()?;
        let name = it.next()?.trim_start_matches('*');
        (name == file && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

fn agent() -> ureq::Agent {
    #[cfg(any(windows, target_os = "macos"))]
    let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build();
    #[cfg(not(any(windows, target_os = "macos")))]
    let tls = ureq::tls::TlsConfig::builder().build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .user_agent(concat!("apps-volume-control/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Returns the latest release if it is newer than the running version.
pub fn check() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp =
        agent().get(&url).header("Accept", "application/vnd.github+json").call().map_err(|e| e.to_string())?;
    let rel: GhRelease = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    if rel.draft || rel.prerelease || !is_newer(&rel.tag_name, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let find = |name: &str| rel.assets.iter().find(|a| a.name == name).map(|a| a.browser_download_url.clone());
    Ok(Some(Release {
        version: rel.tag_name.trim_start_matches('v').to_string(),
        page: rel.html_url.clone(),
        asset_url: find(ASSET_NAME),
        checksums_url: find(CHECKSUMS),
    }))
}

/// Downloads and verifies the platform asset. `progress` receives 0.0..=1.0.
pub fn download(release: &Release, progress: impl Fn(f32)) -> Result<PathBuf, String> {
    let url = release.asset_url.as_ref().ok_or("no asset for this platform")?;
    let expected = match &release.checksums_url {
        Some(u) => {
            let mut resp = agent().get(u).call().map_err(|e| e.to_string())?;
            let sums = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
            Some(find_checksum(&sums, ASSET_NAME).ok_or("asset missing from checksums")?)
        }
        None => None,
    };

    let dir = std::env::temp_dir().join("apps-volume-control-update");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(ASSET_NAME);

    let mut resp = agent().get(url).call().map_err(|e| e.to_string())?;
    let total = resp.body().content_length().unwrap_or(0);
    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        done += n as u64;
        if total > 0 {
            progress(done as f32 / total as f32);
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    let actual = format!("{:x}", hasher.finalize());
    if let Some(expected) = expected {
        if actual != expected {
            return Err("checksum mismatch".into());
        }
    }
    progress(1.0);
    Ok(path)
}

/// Leftovers of the previous version, removed on the next start.
fn backup_path(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

pub fn cleanup_previous() {
    if let Some(target) = install_target() {
        let old = backup_path(&target);
        if old.is_dir() {
            let _ = std::fs::remove_dir_all(old);
        } else {
            let _ = std::fs::remove_file(old);
        }
    }
}

/// The file (or `.app` bundle on macOS) that gets replaced on update.
fn install_target() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    #[cfg(target_os = "macos")]
    {
        // …/Name.app/Contents/MacOS/binary
        let bundle = exe.parent()?.parent()?.parent()?.to_path_buf();
        if bundle.extension().is_some_and(|e| e == "app") {
            return Some(bundle);
        }
    }
    Some(exe)
}

/// Swaps the running executable with the downloaded one and starts it.
/// The caller must quit right after this returns `Ok`.
#[cfg(not(target_os = "macos"))]
pub fn install(downloaded: &Path) -> Result<(), String> {
    let target = install_target().ok_or("unknown install location")?;
    let backup = backup_path(&target);

    // A running executable can be renamed (Windows) or unlinked (Unix), but not overwritten.
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(&target, &backup).map_err(|e| format!("cannot replace {}: {e}", target.display()))?;
    if std::fs::rename(downloaded, &target).is_err() {
        // Different file systems: copy instead.
        if let Err(err) = std::fs::copy(downloaded, &target) {
            let _ = std::fs::rename(&backup, &target);
            return Err(err.to_string());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
    }
    std::process::Command::new(&target).arg(UPDATED_ARG).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
/// Quotes a string for /bin/sh.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
/// Quotes a string as an AppleScript string literal.
pub fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
/// Shell script that swaps the bundle, rolling back on failure, and strips the quarantine
/// flag (the app is not notarized, so Gatekeeper would otherwise refuse to start it).
pub fn macos_swap_script(staged: &str, target: &str) -> String {
    let (s, t, b) = (sh_quote(staged), sh_quote(target), sh_quote(&format!("{target}.old")));
    format!(
        "rm -rf {b} || exit 1; mv {t} {b} || exit 1; \
         if ditto {s} {t}; then rm -rf {b}; else rm -rf {t}; mv {b} {t}; exit 1; fi; \
         xattr -dr com.apple.quarantine {t} 2>/dev/null; exit 0"
    )
}

/// macOS: unpacks the bundle and replaces the installed one. The app has no Developer ID,
/// so the swap runs with administrator rights (system password dialog) and removes the
/// quarantine attribute.
#[cfg(target_os = "macos")]
pub fn install(downloaded: &Path) -> Result<(), String> {
    let target = install_target().ok_or("unknown install location")?;
    if target.extension().is_none_or(|e| e != "app") {
        return Err("not running from an .app bundle".into());
    }
    let dir = downloaded.parent().ok_or("bad download path")?.join("unpacked");
    let _ = std::fs::remove_dir_all(&dir);
    let status = std::process::Command::new("ditto")
        .args(["-x", "-k"])
        .arg(downloaded)
        .arg(&dir)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("failed to unpack update".into());
    }
    let staged = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or("no .app in update archive")?;
    // Our own download carries no quarantine flag, but strip it in case.
    let _ = std::process::Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(&staged).status();

    let script = macos_swap_script(&staged.to_string_lossy(), &target.to_string_lossy());
    let apple = format!(
        "do shell script {} with prompt {} with administrator privileges",
        applescript_quote(&script),
        applescript_quote("Apps Volume Control needs your password to install the update.")
    );
    let out = std::process::Command::new("osascript").args(["-e", &apple]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    std::process::Command::new("open")
        .arg("-n")
        .arg(&target)
        .args(["--args", UPDATED_ARG])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("v3.1.2"), Some((3, 1, 2)));
        assert_eq!(parse_version("2.0"), Some((2, 0, 0)));
        assert_eq!(parse_version("4"), Some((4, 0, 0)));
        assert_eq!(parse_version("1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("nightly"), None);
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("v3.0.1", "3.0.0"));
        assert!(is_newer("3.1.0", "3.0.9"));
        assert!(is_newer("10.0.0", "9.9.9"));
        assert!(!is_newer("v3.0.0", "3.0.0"));
        assert!(!is_newer("2.0.3", "3.0.0"));
        assert!(!is_newer("garbage", "3.0.0"));
    }

    #[test]
    fn finds_checksums() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  other.zip\n{b} *{ASSET_NAME}\nshort  {ASSET_NAME}\n");
        assert_eq!(find_checksum(&sums, ASSET_NAME), Some("b".repeat(64)));
        assert_eq!(find_checksum(&sums, "other.zip"), Some(a));
        assert_eq!(find_checksum(&sums, "missing"), None);
    }

    #[test]
    fn quotes_for_shell() {
        assert_eq!(sh_quote("/Applications/My App.app"), "'/Applications/My App.app'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn quotes_for_applescript() {
        assert_eq!(applescript_quote(r#"say "hi" \ bye"#), r#""say \"hi\" \\ bye""#);
    }

    #[test]
    fn swap_script_rolls_back_and_unquarantines() {
        let script = macos_swap_script("/tmp/new/A.app", "/Applications/A.app");
        assert!(script.contains("mv '/Applications/A.app' '/Applications/A.app.old' || exit 1"));
        assert!(script.contains("ditto '/tmp/new/A.app' '/Applications/A.app'"));
        assert!(script.contains("mv '/Applications/A.app.old' '/Applications/A.app'; exit 1"));
        assert!(script.contains("xattr -dr com.apple.quarantine '/Applications/A.app'"));
    }

    /// The rollback logic is plain POSIX sh; run it with `cp -R` standing in for `ditto`.
    #[cfg(unix)]
    #[test]
    fn swap_script_works_in_sh() {
        let root = std::env::temp_dir().join(format!("avc-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (new, target) = (root.join("new.app"), root.join("A.app"));
        std::fs::create_dir_all(&new).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(new.join("v"), "2").unwrap();
        std::fs::write(target.join("v"), "1").unwrap();

        let script = macos_swap_script(&new.to_string_lossy(), &target.to_string_lossy())
            .replace("ditto ", "cp -R ")
            .replace("xattr ", "true ");
        let ok = std::process::Command::new("sh").args(["-c", &script]).status().unwrap().success();
        assert!(ok);
        assert_eq!(std::fs::read_to_string(target.join("v")).unwrap(), "2");
        assert!(!root.join("A.app.old").exists());

        // A failing copy must restore the previous bundle.
        let failing = macos_swap_script("/nonexistent/x.app", &target.to_string_lossy())
            .replace("ditto ", "cp -R ")
            .replace("xattr ", "true ");
        let ok = std::process::Command::new("sh").args(["-c", &failing]).status().unwrap().success();
        assert!(!ok);
        assert_eq!(std::fs::read_to_string(target.join("v")).unwrap(), "2");
        let _ = std::fs::remove_dir_all(&root);
    }
}
