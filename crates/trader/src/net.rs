//! Jaringan bot: HTTPS (TLS bawaan Windows lewat native-tls/Schannel) dengan sambungan DIPAKAI ULANG
//! (keep-alive) + DoH (DNS lewat HTTPS ke 1.1.1.1, karena DNS ISP dibajak).
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Tls = native_tls::TlsStream<TcpStream>;

/// Satu sambungan HTTPS ke satu host (IP dipaku), dipakai ulang antar permintaan.
pub struct Conn {
    host: String,
    ip: IpAddr,
    s: Option<BufReader<Tls>>,
}

impl Conn {
    pub fn new(host: &str, ip: IpAddr) -> Conn {
        Conn { host: host.to_string(), ip, s: None }
    }
    fn connect(&mut self, to: Duration) -> Result<(), String> {
        let tcp = TcpStream::connect_timeout(&SocketAddr::new(self.ip, 443), to).map_err(|e| format!("net: tcp {e}"))?;
        tcp.set_nodelay(true).ok();
        tcp.set_read_timeout(Some(to)).ok();
        tcp.set_write_timeout(Some(to)).ok();
        let c = native_tls::TlsConnector::new().map_err(|e| format!("net: tls {e}"))?;
        let t = c.connect(&self.host, tcp).map_err(|e| format!("net: tls {e}"))?;
        self.s = Some(BufReader::new(t));
        Ok(())
    }
    /// Kirim satu permintaan (GET kalau body None). Sambung ulang sekali kalau sambungan lama sudah mati.
    pub fn request(&mut self, path: &str, body: Option<&str>, extra: &str, to: Duration) -> Result<String, String> {
        // the first failure is the one reported: the request may have been written on that try, and a connect error
        // or a 429 on the retry must not make a sent transaction look as if it never left (sender.rs classify)
        let mut pertama: Option<String> = None;
        for percobaan in 0..2 {
            if self.s.is_none() {
                if let Err(e) = self.connect(to) {
                    return Err(pertama.unwrap_or(e));
                }
            }
            if let Some(s) = self.s.as_mut() {
                s.get_ref().get_ref().set_read_timeout(Some(to)).ok();
            }
            match self.once(path, body, extra) {
                Ok(t) => return Ok(t),
                Err(e) => {
                    self.s = None;
                    if percobaan == 1 || e.starts_with("net: http") {
                        return Err(pertama.unwrap_or(e));
                    }
                    pertama = Some(e);
                }
            }
        }
        Err("net: gagal".into())
    }
    fn once(&mut self, path: &str, body: Option<&str>, extra: &str) -> Result<String, String> {
        let host = self.host.clone();
        let s = self.s.as_mut().ok_or("net: tidak tersambung")?;
        let req = match body {
            Some(b) => format!(
                "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n{extra}\r\n{b}",
                b.len()
            ),
            None => format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: keep-alive\r\n{extra}\r\n"),
        };
        s.get_mut().write_all(req.as_bytes()).map_err(|e| format!("net: tulis {e}"))?;
        let mut line = String::new();
        s.read_line(&mut line).map_err(|e| format!("net: baca {e}"))?;
        if line.is_empty() {
            return Err("net: sambungan ditutup".into());
        }
        let code: u32 = line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        let (mut clen, mut chunked, mut close) = (None::<usize>, false, false);
        loop {
            line.clear();
            s.read_line(&mut line).map_err(|e| format!("net: baca {e}"))?;
            let l = line.trim_end();
            if l.is_empty() {
                break;
            }
            let low = l.to_ascii_lowercase();
            if let Some(v) = low.strip_prefix("content-length:") {
                clen = v.trim().parse().ok();
            }
            if low.starts_with("transfer-encoding:") && low.contains("chunked") {
                chunked = true;
            }
            if low.starts_with("connection:") && low.contains("close") {
                close = true;
            }
        }
        let mut buf = Vec::new();
        if chunked {
            loop {
                line.clear();
                s.read_line(&mut line).map_err(|e| format!("net: baca {e}"))?;
                let n = usize::from_str_radix(line.trim().split(';').next().unwrap_or("0"), 16).map_err(|_| "net: chunk rusak".to_string())?;
                if n == 0 {
                    line.clear();
                    s.read_line(&mut line).ok();
                    break;
                }
                let st = buf.len();
                buf.resize(st + n, 0);
                s.read_exact(&mut buf[st..]).map_err(|e| format!("net: baca {e}"))?;
                line.clear();
                s.read_line(&mut line).ok();
            }
        } else if let Some(n) = clen {
            buf.resize(n, 0);
            s.read_exact(&mut buf).map_err(|e| format!("net: baca {e}"))?;
        } else {
            s.read_to_end(&mut buf).map_err(|e| format!("net: baca {e}"))?;
            close = true;
        }
        if close {
            self.s = None;
        }
        let txt = String::from_utf8_lossy(&buf).into_owned();
        if code != 200 {
            return Err(format!("net: http {code} {}", txt.chars().take(80).collect::<String>()));
        }
        Ok(txt)
    }
}

/// DoH: tanya Cloudflare (sambung langsung ke 1.1.1.1; nama sertifikat cloudflare-dns.com).
pub fn doh_resolve(host: &str) -> Result<IpAddr, String> {
    let mut c = Conn::new("cloudflare-dns.com", "1.1.1.1".parse().unwrap());
    let t = c.request(&format!("/dns-query?name={host}&type=A"), None, "accept: application/dns-json\r\n", Duration::from_secs(8))?;
    let r: Value = serde_json::from_str(&t).map_err(|e| format!("doh json {e}"))?;
    r["Answer"]
        .as_array()
        .and_then(|a| a.iter().rev().find(|x| x["type"] == 1))
        .and_then(|x| x["data"].as_str())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("doh: tidak ada A record untuk {host}"))
}

struct Ep {
    host: String,
    path: String,
    ip: Mutex<Option<(IpAddr, Instant)>>,
    pool: Mutex<Vec<Conn>>,
}

pub(crate) fn is_range_error(e: &str) -> bool {
    let e = e.to_lowercase();
    e.contains("max allowed range") || e.contains("block range") || e.contains("range is too large") || e.contains("range too large")
}

/// Klien JSON-RPC: beberapa endpoint (cadangan), tiap endpoint punya kolam sambungan (aman untuk banyak thread).
#[derive(Clone)]
pub struct Rpc {
    eps: Arc<Vec<Ep>>,
    idx: Arc<std::sync::atomic::AtomicUsize>,
    doh: bool,
    /// when the client was first seen away from the first endpoint
    pindah: Arc<Mutex<Option<Instant>>>,
}

/// How long a failover lasts before the first endpoint (the owner's own RPC) gets another try.
const KEMBALI: Duration = Duration::from_secs(5);

impl Rpc {
    pub fn new(urls: &str, doh: bool) -> Result<Rpc, String> {
        let mut eps = Vec::new();
        for u in urls.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let rest = u.strip_prefix("https://").ok_or_else(|| format!("hanya https: {u}"))?;
            let (host, path) = match rest.find('/') {
                Some(i) => (&rest[..i], &rest[i..]),
                None => (rest, "/"),
            };
            eps.push(Ep { host: host.into(), path: path.into(), ip: Mutex::new(None), pool: Mutex::new(Vec::new()) });
        }
        if eps.is_empty() {
            return Err("RPC_URLS kosong".into());
        }
        Ok(Rpc { eps: Arc::new(eps), idx: Arc::new(std::sync::atomic::AtomicUsize::new(0)), doh, pindah: Arc::new(Mutex::new(None)) })
    }
    /// The endpoint to use now. One 429 from the first endpoint must not leave the bot on the slower public RPC until
    /// that one fails too, so after `KEMBALI` the first endpoint is tried again.
    fn kini(&self) -> usize {
        let cur = self.idx.load(std::sync::atomic::Ordering::Relaxed) % self.eps.len();
        if cur == 0 {
            return 0;
        }
        let mut t = self.pindah.lock().unwrap();
        match *t {
            Some(x) if x.elapsed() >= KEMBALI => {
                *t = None;
                self.idx.store(0, std::sync::atomic::Ordering::Relaxed);
                0
            }
            Some(_) => cur,
            None => {
                *t = Some(Instant::now());
                cur
            }
        }
    }
    pub fn rotate(&self) {
        self.idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    /// Host of the endpoint currently in use (for logs; never includes the URL path, which can hold an API key).
    pub fn current_host(&self) -> &str {
        &self.eps[self.idx.load(std::sync::atomic::Ordering::Relaxed) % self.eps.len()].host
    }
    fn ip_of(&self, ep: &Ep) -> Result<IpAddr, String> {
        if let Ok(ip) = ep.host.parse::<IpAddr>() {
            return Ok(ip);
        }
        let mut g = ep.ip.lock().unwrap();
        if let Some((ip, t)) = *g {
            if t.elapsed() < Duration::from_secs(600) {
                return Ok(ip); // DoH diulang tiap 10 menit
            }
        }
        let ip = if self.doh {
            doh_resolve(&ep.host)?
        } else {
            use std::net::ToSocketAddrs;
            (ep.host.as_str(), 443).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("dns kosong")?.ip()
        };
        *g = Some((ip, Instant::now()));
        Ok(ip)
    }
    pub fn call_once(&self, method: &str, params: Value, to: Duration) -> Result<Value, String> {
        let cur = self.kini();
        let r = self.call_at(&self.eps[cur], method, params.clone(), to);
        // Some paid endpoints cap the eth_getLogs block range; fall back to the other endpoints for wide scans only.
        match r {
            Err(e) if method == "eth_getLogs" && is_range_error(&e) => {
                for i in (1..self.eps.len()).map(|k| (cur + k) % self.eps.len()) {
                    if let Ok(v) = self.call_at(&self.eps[i], method, params.clone(), to) {
                        return Ok(v);
                    }
                }
                Err(e)
            }
            // the endpoint itself failed (timeout, connection, HTTP 429/5xx/403, not JSON): a dead or over-quota paid
            // RPC must not stall the poll and the executor's reads, so try the others and stay on the first that answers
            Err(e) if self.eps.len() > 1 && !e.starts_with("rpc:") => self.coba_lain(cur, method, &params, to).unwrap_or(Err(e)),
            r => r,
        }
    }
    /// Try every endpoint after `cur`; the first one that answers (a result, or a real "rpc:" error such as a
    /// revert) becomes the current one and its answer is returned.
    fn coba_lain(&self, cur: usize, method: &str, params: &Value, to: Duration) -> Option<Result<Value, String>> {
        for i in (1..self.eps.len()).map(|k| (cur + k) % self.eps.len()) {
            match self.call_at(&self.eps[i], method, params.clone(), to) {
                Err(e) if !e.starts_with("rpc:") => continue,
                r => {
                    self.idx.store(i, std::sync::atomic::Ordering::Relaxed);
                    return Some(r);
                }
            }
        }
        None
    }
    fn call_at(&self, ep: &Ep, method: &str, params: Value, to: Duration) -> Result<Value, String> {
        let ip = self.ip_of(ep)?;
        let reuse = ep.pool.lock().unwrap().pop().filter(|c| c.ip == ip);
        let mut c = reuse.unwrap_or_else(|| Conn::new(&ep.host, ip));
        let body = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string();
        let r = c.request(&ep.path, Some(&body), "", to);
        if r.is_ok() {
            ep.pool.lock().unwrap().push(c);
        }
        let txt = r?;
        let j: Value = serde_json::from_str(&txt).map_err(|_| format!("net: badjson {}", txt.chars().take(60).collect::<String>()))?;
        if let Some(e) = j.get("error") {
            return Err(format!("rpc: {}", e["message"].as_str().map(|s| s.to_string()).unwrap_or(e.to_string())));
        }
        Ok(j["result"].clone())
    }
    /// Beberapa panggilan dalam SATU permintaan HTTP (batch JSON-RPC). Hasil diurutkan sesuai urutan masukan.
    pub fn batch(&self, calls: &[(&str, Value)], to: Duration) -> Result<Vec<Result<Value, String>>, String> {
        let cur = self.kini();
        match self.batch_at(&self.eps[cur], calls, to) {
            // the request itself failed (timeout, HTTP 429/5xx, not JSON): try the other endpoints, stay on the one that answers
            Err(e) => {
                for i in (1..self.eps.len()).map(|k| (cur + k) % self.eps.len()) {
                    if let Ok(v) = self.batch_at(&self.eps[i], calls, to) {
                        self.idx.store(i, std::sync::atomic::Ordering::Relaxed);
                        return Ok(v);
                    }
                }
                Err(e)
            }
            r => r,
        }
    }
    fn batch_at(&self, ep: &Ep, calls: &[(&str, Value)], to: Duration) -> Result<Vec<Result<Value, String>>, String> {
        let ip = self.ip_of(ep)?;
        let reuse = ep.pool.lock().unwrap().pop().filter(|c| c.ip == ip);
        let mut c = reuse.unwrap_or_else(|| Conn::new(&ep.host, ip));
        let body: Vec<Value> = calls.iter().enumerate().map(|(i, (m, p))| json!({"jsonrpc":"2.0","id":i,"method":m,"params":p})).collect();
        let r = c.request(&ep.path, Some(&Value::Array(body).to_string()), "", to);
        if r.is_ok() {
            ep.pool.lock().unwrap().push(c);
        }
        let txt = r?;
        let j: Value = serde_json::from_str(&txt).map_err(|_| format!("net: badjson {}", txt.chars().take(60).collect::<String>()))?;
        let arr = j.as_array().ok_or_else(|| format!("net: batch bukan array {}", txt.chars().take(80).collect::<String>()))?;
        let mut out: Vec<Result<Value, String>> = (0..calls.len()).map(|_| Err("net: tanpa jawaban".to_string())).collect();
        for x in arr {
            if let Some(i) = x.get("id").and_then(|v| v.as_u64()) {
                if (i as usize) < out.len() {
                    out[i as usize] = match x.get("error") {
                        Some(e) => Err(format!("rpc: {}", e["message"].as_str().map(|s| s.to_string()).unwrap_or(e.to_string()))),
                        None => Ok(x["result"].clone()),
                    };
                }
            }
        }
        Ok(out)
    }
    /// = rpc() paper_live.js: coba ulang dengan jeda bertambah, pindah endpoint; revert/limit langsung dilempar.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut a = 0u32;
        loop {
            match self.call_once(method, params.clone(), Duration::from_secs(60)) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let el = e.to_lowercase();
                    if el.contains("exceeds limit") || el.contains("revert") || el.contains("execution") {
                        return Err(e);
                    }
                    if self.eps.len() > 1 {
                        self.rotate();
                    }
                    if a >= 8 {
                        return Err(e);
                    }
                    std::thread::sleep(Duration::from_millis((600u64 << a).min(30000)));
                    a += 1;
                }
            }
        }
    }
    /// eth_getLogs for `[from, to]` cut into `step`-block parts sent as ONE batch request, so an endpoint's per-request
    /// block-range cap (Goldsky: 1000) does not matter. A failed part is retried alone (`quick`, which also switches
    /// endpoint). Logs come back in chain order. If a part still fails the call fails: a partial answer is never
    /// returned as complete.
    pub fn logs_ranged(&self, filter: &Value, from: u64, to: u64, step: u64, ms: u64) -> Result<Vec<Value>, String> {
        let step = step.max(1);
        let mut parts = Vec::new();
        let mut a = from;
        while a <= to {
            let b = a.saturating_add(step - 1).min(to);
            let mut f = filter.clone();
            f["fromBlock"] = json!(format!("{a:#x}"));
            f["toBlock"] = json!(format!("{b:#x}"));
            parts.push(json!([f]));
            a = b + 1;
        }
        let calls: Vec<(&str, Value)> = parts.iter().map(|p| ("eth_getLogs", p.clone())).collect();
        let answers = self.batch(&calls, Duration::from_millis(ms)).unwrap_or_else(|e| vec![Err(e); calls.len()]);
        let mut out = Vec::new();
        for (p, r) in parts.into_iter().zip(answers) {
            let v = match r {
                Ok(v) => v,
                Err(_) => self.quick("eth_getLogs", p, ms)?,
            };
            out.extend(v.as_array().cloned().ok_or("eth_getLogs: answer is not a list")?);
        }
        Ok(out)
    }
    /// = rpcQuick(): maks 3 percobaan cepat, tiap percobaan pindah endpoint.
    pub fn quick(&self, method: &str, params: Value, ms: u64) -> Result<Value, String> {
        let mut last = String::new();
        for _ in 0..3 {
            match self.call_once(method, params.clone(), Duration::from_millis(ms)) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if e.starts_with("rpc:") {
                        return Err(e);
                    }
                    last = e;
                    self.rotate();
                }
            }
        }
        Err(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn a_failover_goes_back_to_the_first_endpoint() {
        let r = Rpc::new("https://a.example/k,https://b.example", false).unwrap();
        assert_eq!(r.kini(), 0);
        r.idx.store(1, Ordering::Relaxed);
        assert_eq!(r.kini(), 1, "stays on the second endpoint right after the failover");
        assert_eq!(r.kini(), 1);
        *r.pindah.lock().unwrap() = Some(Instant::now() - KEMBALI - Duration::from_millis(1));
        assert_eq!(r.kini(), 0, "the first endpoint gets another try");
        assert_eq!(r.current_host(), "a.example");
        assert!(r.pindah.lock().unwrap().is_none());
    }
}
