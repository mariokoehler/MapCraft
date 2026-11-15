//! Releases and self-update (design.md §3.10, M7): ask GitHub for the latest release, download
//! its `mapcraft.exe` with the checksum file, check it, and swap it in for the running exe.
//!
//! Everything up to the verified download takes its URLs from an [`Endpoint`], so tests run it
//! against a pretend GitHub on a local port. The swap and restart ([`install`], [`restart`])
//! are a thin layer no test calls: it would replace the test binary itself.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The asset names a release must carry; nothing else is ever installed.
pub const EXE_ASSET: &str = "mapcraft.exe";
pub const SHA_ASSET: &str = "mapcraft.exe.sha256";

/// Where releases are asked for, and the prefix every asset URL must start with.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub latest_url: String,
    pub download_prefix: String,
}

impl Endpoint {
    pub fn github() -> Self {
        Self {
            latest_url: "https://api.github.com/repos/mariokoehler/MapCraft/releases/latest".into(),
            download_prefix: "https://github.com/mariokoehler/MapCraft/releases/download/".into(),
        }
    }
}

/// A version as three numbers; ordering compares them in turn (0.10.0 > 0.9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// `"v1.2.3"` or `"1.2.3"`; anything else (a suffix such as `-rc1` included) is `None`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        let mut parts = s.split('.').map(|p| p.parse::<u64>().ok());
        let v = Self(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(v)
    }

    /// The running app's version (from `Cargo.toml`).
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml's version is x.y.z")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// A published release with both assets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The release page (its notes).
    pub page_url: String,
    pub exe_url: String,
    /// The exe's size in bytes, as GitHub reports it.
    pub exe_size: u64,
    pub sha_url: String,
}

/// The parts of GitHub's release JSON we read (unknown fields are ignored).
#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<AssetJson>,
}

#[derive(Deserialize)]
struct AssetJson {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// A client with a generic User-Agent (GitHub requires one) and timeouts.
pub fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("MapCraft/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())
}

/// The latest release, or `None` when there's none yet (GitHub answers 404).
pub fn latest(
    client: &reqwest::blocking::Client,
    at: &Endpoint,
) -> Result<Option<Release>, String> {
    let resp = client
        .get(&at.latest_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("can't ask for the latest release: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let text = resp
        .error_for_status()
        .and_then(|r| r.text())
        .map_err(|e| format!("can't ask for the latest release: {e}"))?;
    parse_release(&text, &at.download_prefix).map(Some)
}

/// Reads a release from GitHub's JSON. Its tag must be a version, and it must carry both
/// assets, from under `download_prefix`.
pub fn parse_release(json: &str, download_prefix: &str) -> Result<Release, String> {
    let r: ReleaseJson =
        serde_json::from_str(json).map_err(|e| format!("unexpected release data: {e}"))?;
    let version = Version::parse(&r.tag_name)
        .ok_or_else(|| format!("the release tag {:?} isn't a version", r.tag_name))?;
    let asset = |name: &str| {
        r.assets
            .iter()
            .find(|a| a.name == name)
            .filter(|a| a.browser_download_url.starts_with(download_prefix))
            .ok_or_else(|| format!("release {version} has no usable {name}"))
    };
    let exe = asset(EXE_ASSET)?;
    let sha = asset(SHA_ASSET)?;
    Ok(Release {
        version,
        page_url: r.html_url,
        exe_url: exe.browser_download_url.clone(),
        exe_size: exe.size,
        sha_url: sha.browser_download_url.clone(),
    })
}

/// The hash in a `.sha256` file: the first word, 64 hex digits in either case. A BOM, CRLF and
/// the file name after the hash are allowed.
pub fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    let word = text
        .trim_start_matches('\u{feff}')
        .split_whitespace()
        .next()?;
    if word.len() != 64 || !word.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&word[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// Downloads the release's exe into `dir` (beside the running exe, so the swap is a rename on
/// one drive) and checks its SHA-256. Returns the downloaded file; on any failure nothing is
/// left behind. `progress` gets the bytes so far and the total.
pub fn download(
    client: &reqwest::blocking::Client,
    release: &Release,
    dir: &Path,
    progress: &dyn Fn(u64, u64),
) -> Result<PathBuf, String> {
    let sha_text = client
        .get(&release.sha_url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(|e| format!("can't download the checksum: {e}"))?;
    let want = parse_sha256(&sha_text).ok_or("the checksum file isn't valid")?;
    let path = dir.join("mapcraft.exe.download");
    let result = fetch_checked(client, release, &path, &want, progress);
    if result.is_err() {
        let _ = fs::remove_file(&path);
    }
    result.map(|()| path)
}

fn fetch_checked(
    client: &reqwest::blocking::Client,
    release: &Release,
    path: &Path,
    want: &[u8; 32],
    progress: &dyn Fn(u64, u64),
) -> Result<(), String> {
    let mut resp = client
        .get(&release.exe_url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("can't download the update: {e}"))?;
    let total = resp.content_length().unwrap_or(release.exe_size);
    let mut out = File::create(path).map_err(|e| format!("can't write the update: {e}"))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = resp
            .read(&mut buf)
            .map_err(|e| format!("the download broke off: {e}"))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        out.write_all(&buf[..n])
            .map_err(|e| format!("can't write the update: {e}"))?;
        done += n as u64;
        progress(done, total);
    }
    out.sync_all()
        .map_err(|e| format!("can't write the update: {e}"))?;
    if hash.finalize().as_slice() != want {
        return Err("the download doesn't match its checksum".into());
    }
    Ok(())
}

/// Whether the update can be written into `dir`: tried with a scratch file (a folder under
/// `Program Files` usually refuses).
pub fn folder_writable(dir: &Path) -> bool {
    let probe = dir.join(".mapcraft-write-test");
    let ok = File::create(&probe).is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

/// Puts the downloaded exe in place of the running one. Windows can rename a running exe but
/// not overwrite it; `self-replace` does the rename dance. Never called by tests.
pub fn install(downloaded: &Path) -> Result<(), String> {
    let r = self_replace::self_replace(downloaded)
        .map_err(|e| format!("can't replace MapCraft's exe: {e}"));
    let _ = fs::remove_file(downloaded);
    r
}

/// Starts `exe` (the path the old exe had, now the new version) with `args`. Called after the
/// window has closed.
pub fn restart(exe: &Path, args: &[String]) -> Result<(), String> {
    std::process::Command::new(exe)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("can't start the new version: {e}"))
}

/// The arguments a restarted MapCraft gets: the original ones without `--open` and its value
/// (a start dialog is for one run). Keeping `--data` matters: dropping it would point the new
/// version at the real library.
pub fn restart_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--open" {
            it.next();
        } else {
            out.push(a.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::Arc;

    type Routes = Vec<(&'static str, u16, Vec<u8>)>;

    /// A pretend GitHub on a local port: answers each path with a status and body. The routes
    /// are built from the server's base URL, since the release JSON points back at it.
    fn serve(routes: impl FnOnce(&str) -> Routes) -> String {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let routes = Arc::new(routes(&base));
        std::thread::spawn(move || {
            for stream in l.incoming() {
                let Ok(mut s) = stream else { break };
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut r = std::io::BufReader::new(s.try_clone().unwrap());
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                            break;
                        }
                    }
                    let (status, body) = routes
                        .iter()
                        .find(|(p, _, _)| *p == path)
                        .map(|(_, s, b)| (*s, b.clone()))
                        .unwrap_or((404, b"{}".to_vec()));
                    let _ = write!(
                        s,
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = s.write_all(&body);
                });
            }
        });
        base
    }

    fn release_json(base: &str, tag: &str) -> Vec<u8> {
        format!(
            r#"{{"tag_name": "{tag}", "html_url": "{base}/page", "draft": false,
                "assets": [
                  {{"name": "mapcraft.exe", "browser_download_url": "{base}/dl/mapcraft.exe",
                    "size": 11}},
                  {{"name": "mapcraft.exe.sha256",
                    "browser_download_url": "{base}/dl/mapcraft.exe.sha256", "size": 80}}
                ]}}"#
        )
        .into_bytes()
    }

    fn endpoint(base: &str) -> Endpoint {
        Endpoint {
            latest_url: format!("{base}/latest"),
            download_prefix: format!("{base}/dl/"),
        }
    }

    /// The checksum file exactly as the release workflow writes it.
    fn sha_file(body: &[u8]) -> Vec<u8> {
        let hash: String = Sha256::digest(body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("{hash}  mapcraft.exe\n").into_bytes()
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(Version::parse("v0.10.0"), Some(Version(0, 10, 0)));
        assert!(Version::parse("v0.10.0") > Version::parse("0.9.3"));
        assert_eq!(Version::parse("v1.2"), None);
        assert_eq!(Version::parse("v1.2.3-rc1"), None);
        assert_eq!(Version::parse("v1.2.3.4"), None);
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn checksum_files_are_read_leniently() {
        let h = "AB".repeat(32);
        assert_eq!(
            parse_sha256(&format!("{h}  mapcraft.exe\n")),
            Some([0xab; 32])
        );
        assert_eq!(
            parse_sha256(&format!("\u{feff}{}\r\n", h.to_lowercase())),
            Some([0xab; 32])
        );
        assert_eq!(parse_sha256("abc"), None);
        assert_eq!(parse_sha256(&"zz".repeat(32)), None);
        assert_eq!(parse_sha256(""), None);
    }

    #[test]
    fn a_release_needs_both_assets_from_the_repo() {
        let ok = String::from_utf8(release_json("https://x", "v0.2.0")).unwrap();
        let r = parse_release(&ok, "https://x/dl/").unwrap();
        assert_eq!(r.version, Version(0, 2, 0));
        assert_eq!(r.exe_size, 11);
        assert!(
            parse_release(&ok, "https://elsewhere/").is_err(),
            "foreign URLs"
        );
        let bad_tag = ok.replace("v0.2.0", "latest");
        assert!(parse_release(&bad_tag, "https://x/dl/").is_err());
        let no_sha = ok.replace("\"mapcraft.exe.sha256\"", "\"other.txt\"");
        assert!(parse_release(&no_sha, "https://x/dl/").is_err());
    }

    #[test]
    fn no_release_yet_is_not_an_error() {
        let base = serve(|_| vec![]);
        assert_eq!(latest(&client().unwrap(), &endpoint(&base)), Ok(None));
    }

    #[test]
    fn a_good_download_is_kept_and_a_bad_one_removed() {
        let exe = b"new version".to_vec();
        for (name, sha, body, ok) in [
            ("good", sha_file(&exe), exe.clone(), true),
            (
                "wrong hash",
                sha_file(b"something else"),
                exe.clone(),
                false,
            ),
            ("truncated", sha_file(&exe), exe[..4].to_vec(), false),
            (
                "bad checksum file",
                b"not a hash".to_vec(),
                exe.clone(),
                false,
            ),
        ] {
            let served = serve(|base| {
                vec![
                    ("/latest", 200, release_json(base, "v9.0.0")),
                    ("/dl/mapcraft.exe", 200, body),
                    ("/dl/mapcraft.exe.sha256", 200, sha),
                ]
            });
            let c = client().unwrap();
            let r = latest(&c, &endpoint(&served)).unwrap().unwrap();
            assert_eq!(r.version, Version(9, 0, 0));
            let dir = tempfile::tempdir().unwrap();
            let got = download(&c, &r, dir.path(), &|_, _| {});
            assert_eq!(got.is_ok(), ok, "{name}: {got:?}");
            match got {
                Ok(p) => assert_eq!(fs::read(p).unwrap(), exe),
                Err(_) => assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0, "{name}"),
            }
        }
    }

    #[test]
    fn a_restart_keeps_the_data_folder_but_not_the_start_dialog() {
        let a: Vec<String> = ["--data", r"C:\scratch", "--open", "filters", "--perf"]
            .map(String::from)
            .into();
        assert_eq!(restart_args(&a), ["--data", r"C:\scratch", "--perf"]);
    }
}
