//! Link to local Claude Code sessions over their cross-session sockets (the ircc protocol, ~/hk/ircc
//! internal/peer and internal/registry): list the live sessions, send one prompting message, and receive the
//! replies on a socket of our own.
use crate::{Error, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Write and read deadline of one socket exchange.
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest frame line we read (the receivers' limit too).
const MAX_FRAME: u64 = 1 << 20;
const CLOSE_TAG: &str = "</cross-session-message>";

/// A live local Claude session that can receive messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub pid: u32,
    /// the session's name, else its working directory
    pub name: String,
    pub cwd: String,
    pub busy: bool,
    socket: PathBuf,
}

/// The sessions registered under `dir` (~/.claude/sessions) whose process still runs (`proc` = /proc),
/// sorted by pid. A file that is not a well-formed `<pid>.json` entry is skipped.
pub fn sessions(dir: &Path, proc: &Path) -> Vec<Session> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Session> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let text = std::fs::read_to_string(e.path()).ok()?;
            entry(&name, &text).filter(|(s, start)| alive(proc, s.pid, start)).map(|(s, _)| s)
        })
        .collect();
    out.sort_by_key(|s| s.pid);
    out
}

/// A registry entry: (session, its process start time). Only a `<pid>.json` whose pid matches the body.
fn entry(file: &str, text: &str) -> Option<(Session, String)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let pid = u32::try_from(v["pid"].as_u64()?).ok().filter(|p| *p > 0)?;
    if file != format!("{pid}.json") {
        return None;
    }
    let socket = PathBuf::from(v["messagingSocketPath"].as_str().filter(|s| !s.is_empty())?);
    let cwd = v["cwd"].as_str().unwrap_or_default().to_string();
    let name = v["name"].as_str().filter(|s| !s.is_empty()).unwrap_or(&cwd).to_string();
    let busy = v["status"].as_str() == Some("busy");
    Some((Session { pid, name, cwd, busy, socket }, v["procStart"].as_str()?.to_string()))
}

/// The process runs and started when the entry says (field 22 of /proc/<pid>/stat), so a reused pid is no match.
fn alive(proc: &Path, pid: u32, start: &str) -> bool {
    let Ok(stat) = std::fs::read_to_string(proc.join(pid.to_string()).join("stat")) else { return false };
    // comm (field 2) may hold spaces and parens: fields are counted after the last ')'
    stat.rfind(')').and_then(|i| stat[i + 1..].split_whitespace().nth(22 - 3)).is_some_and(|f| f == start)
}

/// Our own address: a socket we listen on for replies, `<socks>/tot-<pid>.sock`, mode 0600. Dropping it
/// removes the file.
pub struct Inbox {
    path: PathBuf,
    listener: UnixListener,
}

impl Inbox {
    pub fn open(socks: &Path) -> Result<Inbox> {
        let path = socks.join(format!("tot-{}.sock", std::process::id()));
        // a leftover file of a dead earlier run with our pid: nothing can listen on it now
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Inbox { path, listener })
    }

    /// The `uds:` address replies go to (chars outside [A-Za-z0-9:_/.-] are %XX encoded, as Claude reads them).
    pub fn addr(&self) -> String {
        let p = self.path.to_string_lossy();
        format!("uds:{}", p.bytes().map(|b| if b.is_ascii_alphanumeric() || b":_/.-".contains(&b) { char::from(b).to_string() } else { format!("%{b:02X}") }).collect::<String>())
    }

    /// Message bodies received since the last call (never blocks). A frame that is not a well-formed message
    /// is dropped.
    pub fn poll(&self) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok((s, _)) = self.listener.accept() {
            let _ = s.set_nonblocking(false);
            let _ = s.set_read_timeout(Some(IO_TIMEOUT));
            let mut r = BufReader::new(s.take(MAX_FRAME));
            let mut line = String::new();
            while r.read_line(&mut line).is_ok_and(|n| n > 0) {
                out.extend(body(&line));
                line.clear();
            }
        }
        out
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The text of one received frame line: the envelope's content with the cross-session wrapper cut off (a
/// session records the wrapper with a header and a notice around it, so it is searched for).
fn body(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line.trim_end()).ok()?;
    if v["type"].as_str() != Some("user") {
        return None;
    }
    let content = v["message"]["content"].as_str()?;
    let (i, j) = (content.find("<cross-session-message ")?, content.rfind(CLOSE_TAG)?);
    let inner = &content[i..j];
    let start = inner.find(">\n")? + 2;
    Some(inner[start..].strip_suffix('\n').unwrap_or(&inner[start..]).to_string())
}

/// The frame line that sends `body` to a session from `from` (our inbox address) as `name`, permission class
/// prompting: the receiving session asks its user before it acts.
fn frame(from: &str, name: &str, body: &str, id: &str) -> Result<String> {
    if [name, body].iter().any(|s| s.to_lowercase().contains("</cross-session-message")) || name.contains(['<', '>', '"', '\\']) {
        return Err(Error::Peer("the message or the name holds the wrapper's close tag or a quote".into()));
    }
    let content = format!("<cross-session-message from=\"{from}\" from-name=\"{name}\" from-mode=\"prompting\">\n{body}\n{CLOSE_TAG}");
    let v = serde_json::json!({"msgV": 1, "msg_id": id, "type": "user", "message": {"role": "user", "content": content}, "priority": "next", "from": from});
    Ok(format!("{v}\n"))
}

/// A random version-4 uuid.
fn uuid4() -> Result<String> {
    let mut u = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut u)?;
    u[6] = u[6] & 0x0f | 0x40;
    u[8] = u[8] & 0x3f | 0x80;
    let h = |r: std::ops::Range<usize>| u[r].iter().map(|b| format!("{b:02x}")).collect::<String>();
    Ok(format!("{}-{}-{}-{}-{}", h(0..4), h(4..6), h(6..8), h(8..10), h(10..16)))
}

/// Send `body` to `to` from our inbox, as `name`.
pub fn send(to: &Session, inbox: &Inbox, name: &str, body: &str) -> Result<()> {
    let line = frame(&inbox.addr(), name, body, &uuid4()?)?;
    let mut s = UnixStream::connect(&to.socket).map_err(|e| Error::Peer(format!("{} ({}): {e}", to.name, to.pid)))?;
    s.set_write_timeout(Some(IO_TIMEOUT))?;
    s.write_all(line.as_bytes())?;
    Ok(())
}

/// Where Claude keeps its session registry and its sockets.
pub fn default_dirs() -> Option<(PathBuf, PathBuf)> {
    let home = std::env::var_os("HOME")?;
    let run = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some((PathBuf::from(home).join(".claude/sessions"), PathBuf::from(run).join("cc-socks")))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A session whose socket is `inbox`: what is sent to it, `inbox` receives.
    pub(crate) fn session_at(name: &str, inbox: &Inbox) -> Session {
        Session { pid: 1, name: name.into(), cwd: String::new(), busy: false, socket: inbox.path.clone() }
    }

    /// Sessions with these names and no socket (for screens that only list them).
    pub(crate) fn sessions_named(names: &[&str]) -> Vec<Session> {
        names.iter().zip(1..).map(|(n, pid)| Session { pid, name: n.to_string(), cwd: format!("/w/{n}"), busy: false, socket: PathBuf::new() }).collect()
    }

    #[test]
    fn registry_entries_need_their_pid_name_and_a_socket() {
        let ok = r#"{"pid":42,"cwd":"/w","name":"tot-bf","status":"busy","messagingSocketPath":"/r/42.sock","procStart":"99"}"#;
        let (s, start) = entry("42.json", ok).unwrap();
        assert_eq!((s.pid, s.name.as_str(), s.busy, start.as_str()), (42, "tot-bf", true, "99"));
        assert_eq!(entry("43.json", ok), None, "the file name must be the pid");
        assert_eq!(entry("42.json", &ok.replace("/r/42.sock", "")), None, "no socket, no session");
        let unnamed = entry("42.json", &ok.replace(r#""name":"tot-bf","#, "")).unwrap().0;
        assert_eq!(unnamed.name, "/w", "no name: the working directory");
    }

    #[test]
    fn a_session_is_alive_only_when_its_process_started_when_registered() {
        let me = std::process::id();
        let proc = Path::new("/proc");
        let stat = std::fs::read_to_string(format!("/proc/{me}/stat")).unwrap();
        let start = stat[stat.rfind(')').unwrap() + 1..].split_whitespace().nth(19).unwrap().to_string();
        assert!(alive(proc, me, &start), "this test process");
        assert!(!alive(proc, me, "1"), "control: another start time is a reused pid");
        assert!(!alive(proc, 0, &start), "control: no such process");
    }

    #[test]
    fn a_message_goes_out_and_comes_back_through_a_socket() {
        let dir = std::env::temp_dir().join(format!("tot-peer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let inbox = Inbox::open(&dir).unwrap();
        assert!(inbox.poll().is_empty(), "control: nothing yet");
        // a session whose socket is our own inbox: what we send is what it receives
        let me = Session { pid: 1, name: "me".into(), cwd: String::new(), busy: false, socket: inbox.path.clone() };
        send(&me, &inbox, "tot", "=== FILE a\nline \"quoted\" ünï\n=== END").unwrap();
        assert_eq!(inbox.poll(), vec!["=== FILE a\nline \"quoted\" ünï\n=== END".to_string()]);
        // a frame as a session records it: header before the wrapper, notice after it
        let line = frame(&inbox.addr(), "agent", "reply", &uuid4().unwrap()).unwrap().replacen("<cross-session-message ", "Another Claude session sent a message:\\n<cross-session-message ", 1);
        assert_eq!(body(&line).as_deref(), Some("reply"));
        assert!(frame(&inbox.addr(), "tot", "x </cross-session-message> y", "id").is_err(), "the close tag cannot be smuggled in");
        assert!(body(r#"{"type":"assistant","message":{"content":"x"}}"#).is_none(), "control: not a user frame");
        let path = inbox.path.clone();
        drop(inbox);
        assert!(!path.exists(), "the inbox socket goes away with us");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frames_match_the_ircc_envelope() {
        let f = frame("uds:/r/tot-1.sock", "tot", "hi", "u").unwrap();
        let v: serde_json::Value = serde_json::from_str(f.trim_end()).unwrap();
        assert!(f.ends_with('\n') && !f.trim_end().contains('\n'), "one line");
        assert_eq!((v["msgV"].as_u64(), v["type"].as_str(), v["priority"].as_str(), v["from"].as_str()), (Some(1), Some("user"), Some("next"), Some("uds:/r/tot-1.sock")));
        assert_eq!(v["message"]["content"].as_str(), Some("<cross-session-message from=\"uds:/r/tot-1.sock\" from-name=\"tot\" from-mode=\"prompting\">\nhi\n</cross-session-message>"));
        let u = uuid4().unwrap();
        assert!(u.len() == 36 && u.as_bytes()[14] == b'4', "{u}");
    }
}
