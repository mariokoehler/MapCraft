//! The companion apps, PhotoCraft and LightCraft (design.md §2.6, §3.6(c), (d); canvas
//! `MapCraft — Companion apps`).
//!
//! - PhotoCraft gets the file on its command line.
//! - LightCraft is driven over its local control port: JSON lines over TCP, one request per
//!   line, each reply carrying the request's `id`. MapCraft finds the photo in LightCraft's
//!   library, selects it and shows it in the Detail view. If LightCraft isn't running, MapCraft
//!   starts it with `--control <port>` and remembers the port.
//!
//! Everything here blocks (process starts, sockets), so the app calls it from a background
//! thread. Paths and replies are never logged: they name the user's photos.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tracing::{info, warn};

/// How long a starting LightCraft gets to answer and open its library.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// One request's answer. A lookup on a NAS may read the file, so this is generous.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// The first answer, `library.info`: LightCraft answers it at once, so a port that stays
/// silent belongs to something else (or a LightCraft still starting: asked again).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);

/// Windows: start a console program without flashing a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Companion {
    PhotoCraft,
    LightCraft,
}

impl Companion {
    pub fn name(self) -> &'static str {
        match self {
            Self::PhotoCraft => "PhotoCraft",
            Self::LightCraft => "LightCraft",
        }
    }
}

/// Camera RAW types among those the import reads (`import::scan`), DNG included.
pub fn is_raw(file_name: &str) -> bool {
    let ext = Path::new(file_name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "dng"
            | "cr2"
            | "cr3"
            | "crw"
            | "nef"
            | "nrw"
            | "arw"
            | "srf"
            | "sr2"
            | "raf"
            | "orf"
            | "rw2"
            | "pef"
            | "srw"
            | "x3f"
            | "3fr"
            | "iiq"
    )
}

/// Whether `path` can be a companion app: an `.exe` that exists. A `.cmd` or `.bat` would pass
/// file names through `cmd.exe`, which reads `&` and `%` in them (see `launch`).
pub fn is_app(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        && path.is_file()
}

/// File types PhotoCraft opens (its README, checked 2026-11-14): no camera RAW, DNG included.
pub fn photocraft_opens(file_name: &str) -> bool {
    let ext = Path::new(file_name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "tif" | "tiff" | "webp" | "psd" | "exr"
    )
}

/// The command-line tool next to an app: `…\photocraft.exe` → `…\photocraft-cli.exe`.
pub fn cli_beside(app: &Path) -> Option<PathBuf> {
    let stem = app.file_stem()?.to_string_lossy();
    Some(app.with_file_name(format!("{stem}-cli.exe")))
}

/// `photocraft-cli 0.23.1 (45066c482, 2026-11-12)` → `0.23.1`.
pub fn parse_version(out: &str) -> Option<String> {
    let v = out.split_whitespace().nth(1)?;
    v.chars()
        .next()
        .filter(char::is_ascii_digit)
        .map(|_| v.to_owned())
}

/// The app's version, read from the command-line tool beside it (`--version`).
pub fn version(app: &Path) -> Option<String> {
    let cli = cli_beside(app)?;
    let mut cmd = Command::new(cli);
    cmd.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok()?;
    parse_version(&String::from_utf8_lossy(&out.stdout))
}

/// Opens `file` in PhotoCraft. `--` first, so a name starting with `-` is never an option
/// (PhotoCraft's `--help`). `Command` quotes each argument; Windows names can't contain `"`.
pub fn open_in_photocraft(app: &Path, file: &Path) -> io::Result<()> {
    detached(Command::new(app).arg("--").arg(file))
        .spawn()
        .map(|_| ())
}

/// A companion outlives MapCraft: it gets none of MapCraft's console or pipes (a pipe it
/// inherited would stay open for as long as it runs).
fn detached(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
}

// ---- LightCraft ------------------------------------------------------------------------------

/// What showing a photo in LightCraft needs.
#[derive(Debug, Clone)]
pub struct Request {
    /// `lightcraft.exe`.
    pub app: PathBuf,
    /// The control port of a LightCraft that MapCraft started earlier, if any.
    pub port: Option<u16>,
    pub file: PathBuf,
    /// Add the file to LightCraft's library first (the user agreed to).
    pub add: bool,
    /// Bring LightCraft's window to the front afterwards (off in tests).
    pub front: bool,
}

/// What happened, as the UI reports it (canvas artboards 3 and 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// MapCraft started LightCraft on this port: remember it.
    Port(u16),
    /// LightCraft is starting; this may take a while.
    Starting,
    /// Shown in the Detail view; `front`: LightCraft's window came to the front.
    Shown {
        front: bool,
    },
    /// LightCraft's library doesn't have the photo. `sidecars`: it writes XMP sidecars.
    NotInLibrary {
        sidecars: bool,
    },
    Failed(Failure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// LightCraft runs, but not one MapCraft can drive (started by hand).
    AlreadyOpen,
    /// It answers but has no library open.
    NoLibrary,
    /// No answer in time, or what answered isn't LightCraft.
    NoAnswer,
    /// Windows couldn't start it.
    CouldNotStart(String),
    /// LightCraft refused (its own message: an unsupported file, say).
    Refused(String),
}

/// Shows `req.file` in LightCraft, reporting each step to `report`.
pub fn show_in_lightcraft(req: &Request, mut report: impl FnMut(Event)) {
    let event = match run(req, &mut report) {
        Ok(e) => e,
        Err(f) => Event::Failed(f),
    };
    info!(
        "LightCraft: {}",
        match &event {
            Event::Shown { .. } => "shown",
            Event::NotInLibrary { .. } => "not in its library",
            Event::Failed(_) => "failed",
            _ => "",
        }
    );
    report(event);
}

fn run(req: &Request, report: &mut impl FnMut(Event)) -> Result<Event, Failure> {
    let mut pid = None;
    let mut conn = match req.port.and_then(|p| Conn::open(p).ok()) {
        Some(mut c) => match c.has_library() {
            Ok(true) => c,
            Ok(false) => return Err(Failure::NoLibrary),
            Err(_) => start(req, report, &mut pid)?,
        },
        None => start(req, report, &mut pid)?,
    };
    let id = if req.add {
        let r = conn.command(
            "library.import",
            json!({"paths": [req.file], "mode": "add"}),
        )?;
        // New, already there, or brought back from LightCraft's Recently Deleted.
        let id = r["imported"]
            .get(0)
            .or_else(|| r["duplicates"].get(0).map(|d| &d["existing"]))
            .or_else(|| r["restored"].get(0))
            .and_then(Value::as_i64);
        match id {
            Some(id) => id,
            None => return Err(Failure::Refused("LightCraft couldn't add the photo".into())),
        }
    } else {
        let r = conn.command("library.importPreview", json!({"paths": [req.file]}))?;
        let c = &r["candidates"][0];
        if let Some(e) = c["error"].as_str() {
            return Err(Failure::Refused(e.to_owned()));
        }
        match c["existing"].as_i64() {
            Some(id) => id,
            None => {
                let prefs = conn.command("library.xmpPreferences", json!({}))?;
                return Ok(Event::NotInLibrary {
                    sidecars: prefs["autoWrite"].as_bool().unwrap_or(false),
                });
            }
        }
    };
    conn.command("library.select", json!({"ids": [id], "active": id}))?;
    conn.call("ui.set", json!({"view": "detail"}))?;
    let front = req.front && bring_to_front(pid.or_else(|| process_id(&req.app)));
    Ok(Event::Shown { front })
}

/// Starts LightCraft with a control port and waits until it has a library open.
fn start(
    req: &Request,
    report: &mut impl FnMut(Event),
    pid: &mut Option<u32>,
) -> Result<Conn, Failure> {
    // A second LightCraft can't open a library that's open in the first (§3.6(d)).
    if process_id(&req.app).is_some() {
        return Err(Failure::AlreadyOpen);
    }
    let port = free_port().map_err(|e| Failure::CouldNotStart(e.to_string()))?;
    let child = detached(
        Command::new(&req.app)
            .arg("--control")
            .arg(port.to_string()),
    )
    .spawn()
    .map_err(|e| Failure::CouldNotStart(e.to_string()))?;
    *pid = Some(child.id());
    report(Event::Port(port));
    report(Event::Starting);
    let began = Instant::now();
    let mut answered = false;
    while began.elapsed() < START_TIMEOUT {
        if let Ok(mut c) = Conn::open(port) {
            match c.has_library() {
                Ok(true) => return Ok(c),
                Ok(false) => answered = true,
                Err(_) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Err(if answered {
        Failure::NoLibrary
    } else {
        Failure::NoAnswer
    })
}

/// A free port on the loopback interface: bind port 0 and let Windows pick.
fn free_port() -> io::Result<u16> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(l.local_addr()?.port())
}

/// One connection to LightCraft's control port.
struct Conn {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl Conn {
    fn open(port: u16) -> io::Result<Self> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let s = TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
        s.set_read_timeout(Some(REPLY_TIMEOUT))?;
        s.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(Self {
            reader: BufReader::new(s.try_clone()?),
            writer: s,
            next_id: 1,
        })
    }

    /// Whether this is LightCraft with a library open: `library.info` names the library's path
    /// then. Anything else on the port fails to parse, which counts as no answer.
    fn has_library(&mut self) -> Result<bool, Failure> {
        let _ = self.writer.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
        let r = self.command("library.info", json!({}));
        let _ = self.writer.set_read_timeout(Some(REPLY_TIMEOUT));
        let r = r?;
        if r.get("photos").is_none() {
            return Err(Failure::NoAnswer);
        }
        Ok(r["path"].is_string())
    }

    /// Runs an engine command (`engine.execute`).
    fn command(&mut self, command: &str, params: Value) -> Result<Value, Failure> {
        self.call(
            "engine.execute",
            json!({"command": command, "params": params}),
        )
    }

    /// One control-protocol request and its reply's `result`.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"id": id, "method": method, "params": params}).to_string() + "\n";
        self.writer
            .write_all(line.as_bytes())
            .map_err(|_| Failure::NoAnswer)?;
        let mut buf = String::new();
        loop {
            buf.clear();
            match self.reader.read_line(&mut buf) {
                Ok(0) | Err(_) => return Err(Failure::NoAnswer),
                Ok(_) => {}
            }
            let Ok(reply) = serde_json::from_str::<Value>(&buf) else {
                return Err(Failure::NoAnswer);
            };
            // Notices (an idle close) carry no id: skip them.
            if reply["id"].as_u64() != Some(id) {
                continue;
            }
            return if reply["ok"].as_bool() == Some(true) {
                Ok(reply["result"].clone())
            } else {
                warn!("LightCraft refused `{method}`");
                Err(Failure::Refused(
                    reply["error"]
                        .as_str()
                        .unwrap_or("LightCraft refused")
                        .to_owned(),
                ))
            };
        }
    }
}

// ---- Windows processes and windows -----------------------------------------------------------

/// The id of a running process started from a file named like `app` (`lightcraft.exe`).
#[cfg(windows)]
pub fn process_id(app: &Path) -> Option<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let want = app.file_name()?.to_string_lossy().to_lowercase();
    // SAFETY: a snapshot handle is closed below; the entry's size field is set before use.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            let name = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase();
            if name == want {
                found = Some(e.th32ProcessID);
                break;
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
        found
    }
}

#[cfg(not(windows))]
pub fn process_id(_app: &Path) -> Option<u32> {
    None
}

/// Brings the visible top-level window of process `pid` to the front, restoring it when
/// minimised. Windows may refuse; then it only flashes in the taskbar. Returns whether it worked.
#[cfg(windows)]
fn bring_to_front(pid: Option<u32>) -> bool {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    let Some(pid) = pid else { return false };
    struct Search {
        pid: u32,
        found: HWND,
    }
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: `lparam` is the `Search` passed to `EnumWindows` below, alive during the call.
        unsafe {
            let s = &mut *(lparam as *mut Search);
            let mut owner = 0u32;
            GetWindowThreadProcessId(hwnd, &mut owner);
            if owner == s.pid && IsWindowVisible(hwnd) != 0 && GetWindowTextLengthW(hwnd) > 0 {
                s.found = hwnd;
                return 0; // stop
            }
            1
        }
    }
    let mut s = Search {
        pid,
        found: std::ptr::null_mut(),
    };
    // SAFETY: the callback only reads window properties and writes into `s`.
    unsafe {
        EnumWindows(Some(each), &mut s as *mut Search as LPARAM);
        if s.found.is_null() {
            return false;
        }
        if IsIconic(s.found) != 0 {
            ShowWindow(s.found, SW_RESTORE);
        }
        SetForegroundWindow(s.found) != 0
    }
}

#[cfg(not(windows))]
fn bring_to_front(_pid: Option<u32>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn photocraft_opens_images_but_no_camera_raw() {
        for ok in ["a.jpg", "B.JPEG", "c.tif", "d.png", "e.psd", "f.webp"] {
            assert!(photocraft_opens(ok), "{ok}");
        }
        for no in ["a.CR3", "b.cr2", "c.RAF", "d.dng", "e.nef", "f"] {
            assert!(!photocraft_opens(no), "{no}");
        }
    }

    #[test]
    fn apps_are_exe_files_and_raws_are_named() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["photocraft.exe", "photocraft.cmd"] {
            std::fs::write(dir.path().join(f), b"").unwrap();
        }
        assert!(is_app(&dir.path().join("photocraft.exe")));
        assert!(
            !is_app(&dir.path().join("photocraft.cmd")),
            "never through cmd.exe"
        );
        assert!(!is_app(&dir.path().join("lightcraft.exe")), "missing");
        assert!(is_raw("a.CR3") && is_raw("b.dng") && !is_raw("c.heic") && !is_raw("d.jpg"));
    }

    #[test]
    fn versions_come_from_the_tool_beside_the_app() {
        assert_eq!(
            cli_beside(Path::new(r"C:\Program Files\PhotoCraft\photocraft.exe")).unwrap(),
            PathBuf::from(r"C:\Program Files\PhotoCraft\photocraft-cli.exe")
        );
        assert_eq!(
            parse_version("photocraft-cli 0.23.1 (45066c482, 2026-11-12)\n").as_deref(),
            Some("0.23.1")
        );
        assert_eq!(
            parse_version("lightcraft-cli 0.21.1").as_deref(),
            Some("0.21.1")
        );
        assert_eq!(parse_version("usage: …"), None);
        assert_eq!(parse_version(""), None);
    }

    /// A pretend LightCraft: answers each request from `reply(method, params)` and hands every
    /// request it saw to the test.
    fn fake_lightcraft(
        reply: impl Fn(&str, &Value) -> Value + Send + 'static,
    ) -> (u16, mpsc::Receiver<Value>) {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { break };
                let mut w = s.try_clone().unwrap();
                for line in BufReader::new(s).lines() {
                    let Ok(line) = line else { break };
                    let req: Value = serde_json::from_str(&line).unwrap();
                    let (method, params) = match req["method"].as_str().unwrap() {
                        "engine.execute" => (
                            req["params"]["command"].as_str().unwrap().to_owned(),
                            req["params"]["params"].clone(),
                        ),
                        m => (m.to_owned(), req["params"].clone()),
                    };
                    // A notice without an id first: the client must skip it.
                    writeln!(w, "{}", json!({"notice": "hello"})).unwrap();
                    let result = reply(&method, &params);
                    let ok = !result.is_null();
                    writeln!(
                        w,
                        "{}",
                        json!({"id": req["id"], "ok": ok, "result": result, "error": "nope"})
                    )
                    .unwrap();
                    let _ = tx.send(json!({"method": method, "params": params}));
                }
            }
        });
        (port, rx)
    }

    fn request(port: u16, add: bool) -> Request {
        Request {
            // A name no process has, so nothing is ever started or found.
            app: PathBuf::from(r"C:\nowhere\no-such-lightcraft-app.exe"),
            port: Some(port),
            file: PathBuf::from(r"Z:\Photos\IMG_1.jpg"),
            add,
            front: false,
        }
    }

    fn methods(rx: &mpsc::Receiver<Value>) -> Vec<String> {
        rx.try_iter()
            .map(|v| v["method"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn a_photo_lightcraft_has_is_selected_and_shown() {
        let (port, rx) = fake_lightcraft(|m, _| match m {
            "library.info" => json!({"path": "lib", "photos": 3}),
            "library.importPreview" => json!({"candidates": [{"existing": 7, "error": null}]}),
            "library.select" => json!({"selected": 1}),
            "ui.set" => json!({}),
            _ => Value::Null,
        });
        let mut events = Vec::new();
        show_in_lightcraft(&request(port, false), |e| events.push(e));
        assert_eq!(events, vec![Event::Shown { front: false }]);
        assert_eq!(
            methods(&rx),
            [
                "library.info",
                "library.importPreview",
                "library.select",
                "ui.set"
            ]
        );
    }

    #[test]
    fn a_missing_photo_is_reported_with_the_sidecar_setting_and_added_only_in_place() {
        let reply = |m: &str, _: &Value| match m {
            "library.info" => json!({"path": "lib", "photos": 3}),
            "library.importPreview" => json!({"candidates": [{"existing": null}]}),
            "library.xmpPreferences" => json!({"autoWrite": true, "naming": "stem"}),
            "library.import" => json!({"imported": [9], "duplicates": []}),
            "library.select" => json!({"selected": 1}),
            "ui.set" => json!({}),
            _ => Value::Null,
        };
        let (port, rx) = fake_lightcraft(reply);
        let mut events = Vec::new();
        show_in_lightcraft(&request(port, false), |e| events.push(e));
        assert_eq!(events, vec![Event::NotInLibrary { sidecars: true }]);
        assert!(!methods(&rx).contains(&"library.import".to_owned()));

        let (port, rx) = fake_lightcraft(reply);
        let mut events = Vec::new();
        show_in_lightcraft(&request(port, true), |e| events.push(e));
        assert_eq!(events, vec![Event::Shown { front: false }]);
        let seen: Vec<Value> = rx.try_iter().collect();
        let import = seen
            .iter()
            .find(|v| v["method"] == "library.import")
            .unwrap();
        // Never `move` or `copy`: those touch the originals.
        assert_eq!(import["params"]["mode"], "add");
        let select = seen
            .iter()
            .find(|v| v["method"] == "library.select")
            .unwrap();
        assert_eq!(select["params"]["active"], 9);
    }

    #[test]
    fn no_library_and_refusals_are_reported() {
        let (port, _rx) = fake_lightcraft(|m, _| match m {
            "library.info" => json!({"photos": 0, "persistent": false}),
            _ => Value::Null,
        });
        let mut events = Vec::new();
        show_in_lightcraft(&request(port, false), |e| events.push(e));
        assert_eq!(events, vec![Event::Failed(Failure::NoLibrary)]);

        let (port, _rx) = fake_lightcraft(|m, _| match m {
            "library.info" => json!({"path": "lib", "photos": 3}),
            "library.importPreview" => {
                json!({"candidates": [{"existing": null, "error": "unsupported file"}]})
            }
            _ => Value::Null,
        });
        let mut events = Vec::new();
        show_in_lightcraft(&request(port, false), |e| events.push(e));
        assert_eq!(
            events,
            vec![Event::Failed(Failure::Refused("unsupported file".into()))]
        );
    }

    #[test]
    fn a_port_nobody_answers_on_tries_to_start_lightcraft() {
        // Bind and drop: the port is free, so nothing answers there. The app path doesn't
        // exist, so starting fails without starting anything.
        let port = free_port().unwrap();
        let mut events = Vec::new();
        show_in_lightcraft(&request(port, false), |e| events.push(e));
        assert!(matches!(
            events.as_slice(),
            [Event::Failed(Failure::CouldNotStart(_))]
        ));
    }
}
