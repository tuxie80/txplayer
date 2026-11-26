//! 100% Local LAN Remote Control HTTP Server (zero external crates, opt-in).
//!
//! Binds on `0.0.0.0:<port>` when enabled by the user so any phone or tablet on the same Wi-Fi
//! can inspect the live Signal Path, control transport/volume/seek/DSP A-B, and play library tracks.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct RemoteTrackSummary {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub format: String,
    pub dr: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RemoteSnapshot {
    pub playing: bool,
    pub position: f64,
    pub duration: f64,
    pub volume: f32,
    pub dsp_active: bool,
    pub dsp_summary: String,
    pub signal_path: String,
    pub current: Option<RemoteTrackSummary>,
    pub queue: Vec<RemoteTrackSummary>,
    pub catalog_sample: Vec<RemoteTrackSummary>,
}

#[derive(Debug, Clone)]
pub enum RemoteCommand {
    TogglePlayback,
    Next,
    Prev,
    Seek(f64),
    SetVolume(f32),
    ToggleDspAb,
    PlayTrackId(u64),
}

pub struct LanRemoteServer {
    running: Arc<AtomicBool>,
    snapshot: Arc<Mutex<RemoteSnapshot>>,
    pub rx: Receiver<RemoteCommand>,
    tx: Sender<RemoteCommand>,
    pub port: u16,
    pub lan_url: Option<String>,
    pub last_error: Option<String>,
}

impl LanRemoteServer {
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        Self {
            running: Arc::new(AtomicBool::new(false)),
            snapshot: Arc::new(Mutex::new(RemoteSnapshot::default())),
            rx,
            tx,
            port: 7878,
            lan_url: None,
            last_error: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn update_snapshot(&self, snap: RemoteSnapshot) {
        if let Ok(mut guard) = self.snapshot.try_lock() {
            *guard = snap;
        }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.lan_url = None;
    }

    pub fn start(&mut self, port: u16) {
        self.stop();
        let port = port.clamp(1024, 65535);
        self.port = port;
        let listener = match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(e) => {
                self.last_error = Some(format!("Port {port} unavailable: {e}"));
                return;
            }
        };
        let _ = listener.set_nonblocking(true);
        self.last_error = None;
        let ip = detect_lan_ip().unwrap_or_else(|| "127.0.0.1".into());
        self.lan_url = Some(format!("http://{ip}:{port}"));

        let flag = Arc::new(AtomicBool::new(true));
        self.running = flag.clone();
        let snap = self.snapshot.clone();
        let tx = self.tx.clone();

        thread::Builder::new()
            .name("txplayer-lan-remote".into())
            .spawn(move || {
                while flag.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(800)));
                            handle_http_client(&mut stream, &snap, &tx);
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(35));
                        }
                        Err(_) => {
                            thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            })
            .ok();
    }
}

/// Detects the machine's active LAN IPv4 without sending any external packets.
pub fn detect_lan_ip() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:80").ok()?;
    let addr = sock.local_addr().ok()?;
    Some(addr.ip().to_string())
}

fn parse_query_param(target: &str, key: &str) -> Option<String> {
    let (_, query) = target.split_once('?')?;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == key
        {
            return Some(url_decode(v));
        }
    }
    None
}

fn url_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn dispatch_remote_request(
    request_line: &str,
    snap: &RemoteSnapshot,
    tx: &Sender<RemoteCommand>,
) -> (&'static str, &'static str, String) {
    let mut parts = request_line.split_whitespace();
    let _method = parts.next().unwrap_or("GET");
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");

    match path {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", REMOTE_UI_HTML.to_string()),
        "/api/status" => {
            let json = serde_json::to_string(snap).unwrap_or_else(|_| "{}".into());
            ("200 OK", "application/json", json)
        }
        "/api/search" => {
            let q = parse_query_param(target, "q")
                .unwrap_or_default()
                .to_lowercase();
            let hits: Vec<&RemoteTrackSummary> = snap
                .catalog_sample
                .iter()
                .filter(|t| {
                    q.is_empty()
                        || t.title.to_lowercase().contains(&q)
                        || t.artist.to_lowercase().contains(&q)
                        || t.album.to_lowercase().contains(&q)
                })
                .take(40)
                .collect();
            let json = serde_json::to_string(&hits).unwrap_or_else(|_| "[]".into());
            ("200 OK", "application/json", json)
        }
        "/api/cmd" => {
            let act = parse_query_param(target, "action").unwrap_or_default();
            let val = parse_query_param(target, "val").unwrap_or_default();
            let ok = match act.as_str() {
                "toggle" | "play" | "pause" => tx.send(RemoteCommand::TogglePlayback).is_ok(),
                "next" => tx.send(RemoteCommand::Next).is_ok(),
                "prev" => tx.send(RemoteCommand::Prev).is_ok(),
                "dsp_ab" => tx.send(RemoteCommand::ToggleDspAb).is_ok(),
                "vol" => val
                    .parse::<f32>()
                    .ok()
                    .map(|v| tx.send(RemoteCommand::SetVolume(v.clamp(0.0, 1.0))).is_ok())
                    .unwrap_or(false),
                "seek" => val
                    .parse::<f64>()
                    .ok()
                    .map(|s| tx.send(RemoteCommand::Seek(s.max(0.0))).is_ok())
                    .unwrap_or(false),
                "play_id" => val
                    .parse::<u64>()
                    .ok()
                    .map(|id| tx.send(RemoteCommand::PlayTrackId(id)).is_ok())
                    .unwrap_or(false),
                _ => false,
            };
            (
                if ok { "200 OK" } else { "400 Bad Request" },
                "application/json",
                format!("{{\"ok\":{ok}}}"),
            )
        }
        _ => ("404 Not Found", "text/plain", "Not Found".into()),
    }
}

fn handle_http_client(
    stream: &mut TcpStream,
    snapshot: &Arc<Mutex<RemoteSnapshot>>,
    tx: &Sender<RemoteCommand>,
) {
    let mut buf = [0u8; 2048];
    let Ok(n) = stream.read(&mut buf) else {
        return;
    };
    if n == 0 {
        return;
    }
    let req = String::from_utf8_lossy(&buf[..n]);
    let line = req.lines().next().unwrap_or("GET / HTTP/1.1");
    let snap = snapshot
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    let (status, ctype, body) = dispatch_remote_request(line, &snap, tx);
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
}

const REMOTE_UI_HTML: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1,maximum-scale=1">
<title>TxPlayer LAN Remote</title>
<style>
:root{--bg:#0b1114;--card:#10191f;--brd:#1d2b35;--tx:#e8edf2;--mut:#8d97a1;--acc:#2dd4bf;--gold:#e6b85c}
*{box-sizing:border-box}body{margin:0;padding:18px;background:var(--bg);color:var(--tx);font:15px/1.45 -apple-system,BlinkMacSystemFont,sans-serif;max-width:520px;margin-inline:auto}
.top{display:flex;justify-content:space-between;align-items:center;margin-bottom:14px}
.brand{font-weight:700;letter-spacing:.02em}.badge{font:600 11px ui-monospace,monospace;color:var(--gold);border:1px solid rgba(230,184,92,.4);background:rgba(230,184,92,.12);padding:2px 8px;border-radius:99px}
.card{background:var(--card);border:1px solid var(--brd);border-radius:12px;padding:16px;margin-bottom:12px}
.title{font-size:18px;font-weight:700;margin:0 0 2px}.sub{color:var(--mut);font-size:13px;margin:0 0 8px}
.sig{font:11.5px ui-monospace,monospace;color:var(--acc);padding:5px 8px;background:rgba(45,212,191,.1);border:1px solid rgba(45,212,191,.28);border-radius:7px;margin-bottom:12px}
.ctrls{display:flex;gap:8px;justify-content:center;margin:10px 0}
button{background:#15222b;color:var(--tx);border:1px solid var(--brd);border-radius:9px;padding:10px 16px;font-size:15px;font-weight:600;cursor:pointer}
button.pri{background:var(--acc);color:#041614;border-color:var(--acc);min-width:110px}
input[type=range]{width:100%;accent-color:var(--acc)}
.row{display:flex;justify-content:space-between;font:11.5px ui-monospace,monospace;color:var(--mut)}
.search{width:100%;padding:9px 12px;border-radius:8px;border:1px solid var(--brd);background:var(--bg);color:var(--tx);margin-bottom:8px}
.item{padding:8px 6px;border-bottom:1px solid var(--brd);cursor:pointer;display:flex;justify-content:space-between;gap:8px;align-items:center}
.item:last-child{border:none}.item small{color:var(--mut);display:block;font-size:12px}.fmt{font:10.5px ui-monospace,monospace;color:var(--gold);white-space:nowrap}
</style></head><body>
<div class="top"><span class="brand">TxPlayer · LAN Remote</span><span class="badge">LOCAL WI-FI</span></div>
<div class="card">
  <div class="title" id="t">Not Playing</div>
  <div class="sub" id="a">—</div>
  <div class="sig" id="s">Signal Path: Standby</div>
  <input id="seek" type="range" min="0" max="100" value="0">
  <div class="row"><span id="pos">0:00</span><span id="dur">0:00</span></div>
  <div class="ctrls">
    <button onclick="cmd('prev')">⏮</button>
    <button class="pri" id="pp" onclick="cmd('toggle')">Play</button>
    <button onclick="cmd('next')">⏭</button>
    <button id="dsp" onclick="cmd('dsp_ab')">DSP A/B</button>
  </div>
  <div class="row"><span>Volume</span><span id="vl">100%</span></div>
  <input id="vol" type="range" min="0" max="100" value="100">
</div>
<div class="card">
  <input class="search" id="q" placeholder="Search library tracks…" oninput="doSearch()">
  <div id="list"></div>
</div>
<script>
const $=id=>document.getElementById(id);
const fmt=s=>{s=Math.max(0,Math.floor(s||0));return Math.floor(s/60)+':'+String(s%60).padStart(2,'0')};
let busy=false;
async function cmd(a,v){await fetch('/api/cmd?action='+a+(v!==undefined?'&val='+encodeURIComponent(v):''),{method:'POST'});poll();}
$('vol').addEventListener('change',e=>cmd('vol',(e.target.value/100).toFixed(2)));
$('seek').addEventListener('change',e=>cmd('seek',e.target.value));
async function doSearch(){
  const r=await fetch('/api/search?q='+encodeURIComponent($('q').value));
  const items=await r.json();
  $('list').innerHTML=items.slice(0,20).map(x=>`<div class="item" onclick="cmd('play_id',${x.id})"><div><strong>${x.title}</strong><small>${x.artist} · ${x.album}</small></div><span class="fmt">${x.format}${x.dr?' · DR'+x.dr:''}</span></div>`).join('');
}
async function poll(){
  if(busy)return; busy=true;
  try{
    const r=await fetch('/api/status'); const d=await r.json();
    $('t').textContent=d.current?d.current.title:'Standby';
    $('a').textContent=d.current?(d.current.artist+' · '+d.current.album):'Select a track below';
    $('s').textContent=d.signal_path||('DSP: '+d.dsp_summary);
    $('pp').textContent=d.playing?'Pause':'Play';
    $('dsp').textContent=d.dsp_active?'DSP: ON':'DSP: OFF';
    $('seek').max=Math.max(1,Math.floor(d.duration||1)); $('seek').value=Math.floor(d.position||0);
    $('pos').textContent=fmt(d.position); $('dur').textContent=fmt(d.duration);
    $('vol').value=Math.round((d.volume||0)*100); $('vl').textContent=$('vol').value+'%';
  }catch(_){}
  busy=false;
}
poll(); doSearch(); setInterval(poll, 1200);
</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_http_dispatcher_serves_status_search_and_commands() {
        let (tx, rx) = unbounded();
        let snap = RemoteSnapshot {
            playing: true,
            position: 42.0,
            duration: 210.0,
            volume: 0.85,
            dsp_active: true,
            dsp_summary: "EQ (Classical)".into(),
            signal_path: "FLAC 24/96 · Exclusive".into(),
            current: Some(RemoteTrackSummary {
                id: 1,
                title: "Moonlight Sonata".into(),
                artist: "Maurizio Pollini".into(),
                album: "Beethoven Sonatas".into(),
                format: "FLAC · 24/96".into(),
                dr: Some(14),
            }),
            queue: Vec::new(),
            catalog_sample: vec![RemoteTrackSummary {
                id: 1,
                title: "Moonlight Sonata".into(),
                artist: "Maurizio Pollini".into(),
                album: "Beethoven Sonatas".into(),
                format: "FLAC · 24/96".into(),
                dr: Some(14),
            }],
        };

        let (st, ct, body) = dispatch_remote_request("GET /api/status HTTP/1.1", &snap, &tx);
        assert_eq!(st, "200 OK");
        assert_eq!(ct, "application/json");
        assert!(body.contains("Moonlight Sonata"));

        let (st2, _, body2) =
            dispatch_remote_request("GET /api/search?q=pollini HTTP/1.1", &snap, &tx);
        assert_eq!(st2, "200 OK");
        assert!(body2.contains("Beethoven Sonatas"));

        let (st3, _, _) =
            dispatch_remote_request("POST /api/cmd?action=dsp_ab HTTP/1.1", &snap, &tx);
        assert_eq!(st3, "200 OK");
        assert!(matches!(rx.try_recv(), Ok(RemoteCommand::ToggleDspAb)));

        let (st4, _, _) =
            dispatch_remote_request("POST /api/cmd?action=vol&val=0.65 HTTP/1.1", &snap, &tx);
        assert_eq!(st4, "200 OK");
        match rx.try_recv() {
            Ok(RemoteCommand::SetVolume(v)) => assert!((v - 0.65).abs() < 1e-4),
            other => panic!("unexpected cmd: {other:?}"),
        }
    }
}
