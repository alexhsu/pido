//! Checking GitHub for a newer pido, and installing it in place when this
//! build runs as an AppImage.
//!
//! Everything here runs on the background executor: `ureq` is synchronous and
//! the download is a plain stream to a sibling `.part` file, so the UI thread
//! only ever sees the progress callbacks it is handed.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, bail};
use sha2::{Digest as _, Sha256};

/// The running build's version. Cargo.toml carries it; the release workflow
/// stamps the same number into the tag (`v0.3.0`), so the two must be kept in
/// step at release time.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// The repo's latest-release page. It redirects to `/releases/tag/<tag>`,
/// which needs no API token and is not subject to the API's per-IP rate
/// limit (shared egress IPs hit that limit quickly).
const LATEST_PAGE: &str = "https://github.com/alexhsu/pido/releases/latest";
const REPO: &str = "https://github.com/alexhsu/pido";

/// `"v0.10.2"` → `(0, 10, 2)`. A leading `v` is optional and a pre-release or
/// build suffix is ignored, so a tag never fails to compare.
pub fn version_tuple(tag: &str) -> Option<(u64, u64, u64)> {
    let stripped = tag.trim().trim_start_matches('v');
    let core = stripped.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Whether `tag` names a release newer than the running build.
pub fn is_newer(tag: &str) -> bool {
    match (version_tuple(tag), version_tuple(CURRENT)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// What a check found out about the newest release.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseInfo {
    pub tag: String,
    /// The release page, for "open in browser" where nothing installs in
    /// place.
    pub page: String,
    /// The AppImage for this platform, when the release carries one.
    pub asset_url: Option<String>,
    /// Its size in bytes; the redirect check cannot know it (0).
    pub asset_size: u64,
    /// The `.sha256` sibling of the asset.
    pub checksum_url: Option<String>,
}

/// The tag a release-page URL points at: `v0.4.0` from
/// `.../releases/tag/v0.4.0`.
pub fn tag_from_url(url: &str) -> Option<String> {
    let (_, tag) = url.rsplit_once("/tag/")?;
    let tag = tag.trim_end_matches('/');
    (!tag.is_empty() && !tag.contains('/')).then(|| tag.to_string())
}

/// Fetch the newest release without the REST API: ask for the release page
/// and read where GitHub redirects it. The asset URLs follow from the tag —
/// the release workflow names them `pido-<version>-linux-x64.AppImage`.
///
/// The REST API would be richer (asset sizes, checksums) but its 60/hour
/// unauthenticated limit is shared by everyone behind one egress IP, and a
/// desktop client starting behind a office NAT must not fail for that.
pub fn fetch_latest() -> anyhow::Result<ReleaseInfo> {
    let response = ureq::builder()
        .redirects(0)
        .build()
        .get(LATEST_PAGE)
        .set("User-Agent", &format!("pido/{CURRENT}"))
        .timeout(Duration::from_secs(15))
        .call()?;
    let location = response
        .header("Location")
        .map(str::to_string)
        .unwrap_or_else(|| response.get_url().to_string());
    let tag = tag_from_url(&location)
        .ok_or_else(|| anyhow::anyhow!("release page did not point at a tag: {location}"))?;
    Ok(release_for_tag(&tag))
}

/// The release a tag names, with the asset URLs the workflow produces.
pub fn release_for_tag(tag: &str) -> ReleaseInfo {
    let version = tag.trim_start_matches('v');
    let asset = format!("pido-{version}-linux-x64.AppImage");
    let download = format!("{REPO}/releases/download/{tag}");
    ReleaseInfo {
        tag: tag.to_string(),
        page: format!("{REPO}/releases/tag/{tag}"),
        asset_url: Some(format!("{download}/{asset}")),
        // The redirect carries no metadata; the progress line falls back to
        // the server's Content-Length.
        asset_size: 0,
        checksum_url: Some(format!("{download}/{asset}.sha256")),
    }
}

/// Fetch a small text resource with a pido user agent.
pub fn fetch_text(url: &str) -> anyhow::Result<String> {
    let response = ureq::get(url)
        .set("User-Agent", &format!("pido/{CURRENT}"))
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(15))
        .call()?;
    Ok(response.into_string()?)
}

/// Stream `url` into `dest`, reporting `(received, total)` as it goes. A
/// zero `total` means the server sent no length.
pub fn download(url: &str, dest: &Path, mut progress: impl FnMut(u64, u64)) -> anyhow::Result<()> {
    let response = ureq::get(url)
        .set("User-Agent", &format!("pido/{CURRENT}"))
        .timeout(Duration::from_secs(120))
        .call()?;
    let total = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let mut reader = response.into_reader();
    let mut file =
        std::fs::File::create(dest).with_context(|| format!("cannot write {}", dest.display()))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut received = 0u64;
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])?;
        received += read as u64;
        progress(received, total);
    }
    file.flush()?;
    Ok(())
}

/// The hex digest from a `sha256sum`-style line (`"<hex>  <filename>"`).
pub fn parse_sha256(text: &str) -> Option<String> {
    let hex = text.split_whitespace().next()?;
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

/// The SHA-256 of a file, lowercase hex.
pub fn sha256_of(path: &Path) -> anyhow::Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Compare a downloaded file against the published digest.
pub fn verify_sha256(path: &Path, expected: &str) -> anyhow::Result<()> {
    let actual = sha256_of(path)?;
    if actual != expected.to_ascii_lowercase() {
        bail!("checksum mismatch: expected {expected}, got {actual}");
    }
    Ok(())
}

/// The AppImage this process was launched from, when it was one.
///
/// Inside an AppImage's mount `current_exe()` points at the bundled binary in
/// a temp dir, so `$APPIMAGE` is the only reliable path to the file the user
/// actually runs.
pub fn appimage_path() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

/// Move the verified download over `target`, which must be the running
/// AppImage. Same-directory rename is atomic, and the file needs its execute
/// bit back (it came from a plain file write).
#[cfg(unix)]
pub fn replace_in_place(part: &Path, target: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(part, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("cannot chmod {}", part.display()))?;
    if let Err(error) = std::fs::rename(part, target) {
        let _ = std::fs::remove_file(part);
        return Err(anyhow::anyhow!(
            "cannot replace {}: {error}",
            target.display()
        ));
    }
    Ok(())
}

/// In-place replacement only exists for the AppImage; other platforms get the
/// release page instead.
#[cfg(not(unix))]
pub fn replace_in_place(_part: &Path, _target: &Path) -> anyhow::Result<()> {
    bail!("in-place update is only supported for the Linux AppImage")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically_not_lexically() {
        assert_eq!(version_tuple("v0.10.2"), Some((0, 10, 2)));
        assert_eq!(version_tuple("0.3"), Some((0, 3, 0)));
        assert_eq!(version_tuple("v1"), Some((1, 0, 0)));
        assert_eq!(version_tuple("v0.4.0-rc1"), Some((0, 4, 0)));
        assert_eq!(version_tuple("nightly"), None);

        // 0.10 is newer than 0.3 even though "0.10" < "0.3" as text.
        assert!(is_newer("v999.0.0"));
        assert!(!is_newer("v0.0.1"));
        assert!(!is_newer("garbage"));

        // Whatever the crate version is, one patch above it is an update.
        let (major, minor, patch) = version_tuple(CURRENT).expect("own version parses");
        assert!(is_newer(&format!("v{major}.{minor}.{}", patch + 1)));
        assert!(!is_newer(&format!("v{major}.{minor}.{patch}")));
    }

    #[test]
    fn the_release_page_redirect_names_the_tag() {
        assert_eq!(
            tag_from_url("https://github.com/alexhsu/pido/releases/tag/v0.4.1"),
            Some("v0.4.1".to_string())
        );
        assert_eq!(
            tag_from_url("https://github.com/alexhsu/pido/releases/tag/v0.4.1/"),
            Some("v0.4.1".to_string())
        );
        assert_eq!(
            tag_from_url("https://github.com/alexhsu/pido/releases"),
            None
        );
    }

    #[test]
    fn a_tag_maps_to_the_assets_the_workflow_publishes() {
        let release = release_for_tag("v0.4.1");
        assert_eq!(
            release.page,
            "https://github.com/alexhsu/pido/releases/tag/v0.4.1"
        );
        assert_eq!(
            release.asset_url.as_deref(),
            Some(
                "https://github.com/alexhsu/pido/releases/download/v0.4.1/pido-0.4.1-linux-x64.AppImage"
            )
        );
        assert_eq!(
            release.checksum_url.as_deref(),
            Some(
                "https://github.com/alexhsu/pido/releases/download/v0.4.1/pido-0.4.1-linux-x64.AppImage.sha256"
            )
        );
    }

    #[test]
    fn checksum_lines_parse_in_sha256sum_format() {
        let hex = "a".repeat(64);
        assert_eq!(
            parse_sha256(&format!("{hex}  pido.AppImage")),
            Some(hex.clone())
        );
        assert_eq!(parse_sha256(&hex.to_uppercase()), Some(hex.clone()));
        assert_eq!(parse_sha256("deadbeef"), None, "too short");
        assert_eq!(parse_sha256(""), None);
    }
}
