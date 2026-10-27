//! Pure update logic: version compare, GitHub release parsing, checksums and status texts.

use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// A `major.minor.patch` version. Any `-suffix` is ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl FromStr for Version {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("not a version: {s:?}");
        let t = s.trim();
        let t = t.strip_prefix('v').unwrap_or(t);
        let core = t.split('-').next().unwrap_or("");
        let mut parts = core.split('.');
        let mut next = || -> Result<u32, String> {
            let p = parts.next().ok_or_else(bad)?;
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            p.parse().map_err(|_| bad())
        };
        let version = Version {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The version this build was compiled as.
pub fn current_version() -> Version {
    env!("CARGO_PKG_VERSION")
        .parse()
        .expect("CARGO_PKG_VERSION is a valid version")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: Version,
    pub page_url: String,
    pub exe_url: String,
    pub sha_url: String,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
    browser_download_url: String,
}

/// Parses GitHub's `releases/latest` JSON. Drafts, prereleases and releases without both
/// `lite-bar.exe` and `lite-bar.exe.sha256` assets are an `Err`.
pub fn parse_latest_release(json: &str) -> Result<ReleaseInfo, String> {
    let raw: RawRelease =
        serde_json::from_str(json).map_err(|e| format!("bad release JSON: {e}"))?;
    if raw.draft || raw.prerelease {
        return Err("latest release is a draft or prerelease".into());
    }
    let version = raw.tag_name.parse()?;
    let asset = |name: &str| {
        raw.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
            .ok_or_else(|| format!("release has no {name} asset"))
    };
    Ok(ReleaseInfo {
        version,
        page_url: raw.html_url.clone(),
        exe_url: asset("lite-bar.exe")?,
        sha_url: asset("lite-bar.exe.sha256")?,
    })
}

pub fn newer_release(current: &Version, latest: &ReleaseInfo) -> bool {
    latest.version > *current
}

/// Reads the first token of a `.sha256` file (`<hex>` or `<hex>  filename`).
pub fn parse_sha256_file(text: &str) -> Option<[u8; 32]> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let token = text.split_whitespace().next()?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (byte, pair) in out.iter_mut().zip(token.as_bytes().chunks(2)) {
        let pair = std::str::from_utf8(pair).ok()?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(ReleaseInfo),
    Downloading(ReleaseInfo),
    Failed(String),
    CheckFailed,
}

impl UpdateState {
    pub fn status_text(&self, current: &Version) -> String {
        match self {
            UpdateState::Idle => String::new(),
            UpdateState::Checking => "Checking…".into(),
            UpdateState::UpToDate => format!("You're up to date (v{current})"),
            UpdateState::Available(r) => format!("Update available: v{}", r.version),
            UpdateState::Downloading(r) => format!("Downloading v{}…", r.version),
            UpdateState::Failed(why) => format!("Update failed: {why}"),
            UpdateState::CheckFailed => "Couldn't check for updates".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = r#"{
      "url": "https://api.github.com/repos/paymonr/lite-bar/releases/1",
      "html_url": "https://github.com/paymonr/lite-bar/releases/tag/v0.5.0",
      "id": 1,
      "tag_name": "v0.5.0",
      "name": "Lite Bar 0.5.0",
      "draft": false,
      "prerelease": false,
      "author": {"login": "paymonr", "id": 7},
      "body": "notes",
      "assets": [
        {"name": "notes.txt", "size": 5, "browser_download_url": "https://github.com/x/notes.txt"},
        {"name": "lite-bar.exe", "size": 100, "download_count": 3,
         "browser_download_url": "https://github.com/paymonr/lite-bar/releases/download/v0.5.0/lite-bar.exe"},
        {"name": "lite-bar.exe.sha256", "size": 80,
         "browser_download_url": "https://github.com/paymonr/lite-bar/releases/download/v0.5.0/lite-bar.exe.sha256"}
      ]
    }"#;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    fn info(version: &str) -> ReleaseInfo {
        ReleaseInfo {
            version: v(version),
            page_url: "p".into(),
            exe_url: "e".into(),
            sha_url: "s".into(),
        }
    }

    #[test]
    fn version_parses_and_orders() {
        assert_eq!(v("v0.4.0"), v("0.4.0"));
        assert_eq!(v("1.2.3-beta.1"), v("1.2.3"));
        assert!(v("v0.4.0") < v("0.4.1"));
        assert!(v("0.4.1") < v("0.10.0"));
        assert_eq!(v("0.4.0").to_string(), "0.4.0");
        for bad in ["", "v", "1.2", "1.2.3.4", "a.b.c", "1.2.x", "-1.2.3"] {
            assert!(bad.parse::<Version>().is_err(), "{bad}");
        }
    }

    #[test]
    fn current_version_is_the_package_version() {
        assert_eq!(current_version().to_string(), env!("CARGO_PKG_VERSION"));
        assert_eq!(current_version(), v("0.4.0"));
    }

    #[test]
    fn parses_latest_release() {
        let r = parse_latest_release(RELEASE).unwrap();
        assert_eq!(r.version, v("0.5.0"));
        assert_eq!(
            r.page_url,
            "https://github.com/paymonr/lite-bar/releases/tag/v0.5.0"
        );
        assert!(r.exe_url.ends_with("/v0.5.0/lite-bar.exe"));
        assert!(r.sha_url.ends_with("/v0.5.0/lite-bar.exe.sha256"));
    }

    #[test]
    fn release_missing_an_asset_is_err() {
        let no_sha = RELEASE.replace("lite-bar.exe.sha256", "other.sha256");
        assert!(parse_latest_release(&no_sha).is_err());
        let no_exe = RELEASE.replace("\"lite-bar.exe\"", "\"other.exe\"");
        assert!(parse_latest_release(&no_exe).is_err());
    }

    #[test]
    fn draft_and_prerelease_are_err() {
        let draft = RELEASE.replace("\"draft\": false", "\"draft\": true");
        assert!(parse_latest_release(&draft).is_err());
        let pre = RELEASE.replace("\"prerelease\": false", "\"prerelease\": true");
        assert!(parse_latest_release(&pre).is_err());
    }

    #[test]
    fn bad_json_is_check_failed() {
        assert!(parse_latest_release("not json").is_err());
        assert!(parse_latest_release("{}").is_err());
        assert!(parse_latest_release(
            &RELEASE.replace("\"tag_name\": \"v0.5.0\"", "\"tag_name\": \"nope\"")
        )
        .is_err());
    }

    #[test]
    fn newer_release_compares_versions() {
        assert!(newer_release(&v("0.4.0"), &info("0.4.1")));
        assert!(!newer_release(&v("0.4.0"), &info("0.4.0")));
        assert!(!newer_release(&v("0.4.0"), &info("0.3.9")));
    }

    #[test]
    fn sha_file_variants() {
        let hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let want = sha256(b"abc");
        assert_eq!(parse_sha256_file(hex), Some(want));
        assert_eq!(parse_sha256_file(&format!("{hex}\n")), Some(want));
        assert_eq!(
            parse_sha256_file(&format!("{hex}  lite-bar.exe\r\n")),
            Some(want)
        );
        assert_eq!(
            parse_sha256_file(&format!(
                "\u{feff}{}  *lite-bar.exe\r\n",
                hex.to_uppercase()
            )),
            Some(want)
        );
        assert_eq!(parse_sha256_file(""), None);
        assert_eq!(parse_sha256_file("zz"), None);
        assert_eq!(parse_sha256_file(&hex[..63]), None);
        assert_eq!(parse_sha256_file(&format!("{hex}0")), None);
        assert_eq!(parse_sha256_file(&hex.replace('b', "g")), None);
        assert_eq!(parse_sha256_file(&format!("+{}", &hex[1..])), None);
    }

    #[test]
    fn sha256_known_vector() {
        let hex: String = sha256(b"abc").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn status_texts() {
        let cur = v("0.4.0");
        let t = |s: UpdateState| s.status_text(&cur);
        assert_eq!(t(UpdateState::UpToDate), "You're up to date (v0.4.0)");
        assert_eq!(
            t(UpdateState::Available(info("0.5.0"))),
            "Update available: v0.5.0"
        );
        assert_eq!(
            t(UpdateState::Downloading(info("0.5.0"))),
            "Downloading v0.5.0…"
        );
        assert_eq!(
            t(UpdateState::Failed("checksum mismatch".into())),
            "Update failed: checksum mismatch"
        );
        assert_eq!(t(UpdateState::CheckFailed), "Couldn't check for updates");
        assert_eq!(t(UpdateState::Checking), "Checking…");
        assert_eq!(t(UpdateState::Idle), "");
    }
}
