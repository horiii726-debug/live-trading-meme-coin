//! KLASIFIKASI ASET sebelum peringkat/trading.
//!  - Ekuitas  : saham ter-tokenisasi Robinhood (AAPL, NVDA, HIMS, ...) -> DIKECUALIKAN (hanya dicatat sebagai terdeteksi).
//!               Ciri on-chain (diverifikasi 2026-10-04): BeaconProxy dengan beacon 0xe10b6f6b275de231345c20d14ab812db62151b00,
//!               kode proxy identik 283 byte, nama "<Perusahaan> • Robinhood Token". Beacon = bukti utama; nama = cadangan.
//!  - Quote    : ETH native, WETH, USDG (alat tukar, bukan objek trading).
//!  - Meme     : token kripto/meme lain -> BOLEH diberi peringkat & ditradingkan.
use crate::net::Rpc;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub const BEACON_SAHAM: &str = "e10b6f6b275de231345c20d14ab812db62151b00";
const SLOT_BEACON: &str = "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50"; // EIP-1967 beacon slot

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kelas {
    Meme,
    Ekuitas,
    Quote,
}
impl Kelas {
    pub fn nama(&self) -> &'static str {
        match self {
            Kelas::Meme => "meme",
            Kelas::Ekuitas => "ekuitas",
            Kelas::Quote => "quote",
        }
    }
    fn dari(s: &str) -> Option<Kelas> {
        match s {
            "meme" => Some(Kelas::Meme),
            "ekuitas" => Some(Kelas::Ekuitas),
            "quote" => Some(Kelas::Quote),
            _ => None,
        }
    }
}

pub fn quote_dikenal(t: &str) -> bool {
    t == crate::radar::NATIVE || t == crate::radar::WETH || t == crate::radar::USDG
}

fn nama_token(rpc: &Rpc, t: &str) -> Option<String> {
    let h = rpc.quick("eth_call", json!([{"to": t, "data": "0x06fdde03"}, "latest"]), 3000).ok()?;
    let h = h.as_str()?.trim_start_matches("0x").to_string();
    let n = usize::from_str_radix(h.get(64..128)?, 16).ok()?;
    let b: Vec<u8> = (0..n).filter_map(|i| u8::from_str_radix(h.get(128 + 2 * i..130 + 2 * i)?, 16).ok()).collect();
    Some(String::from_utf8_lossy(&b).into_owned())
}

/// Klasifikasi satu token lewat RPC. Err = RPC gagal (JANGAN dianggap meme; coba lagi nanti).
pub fn klasifikasi(rpc: &Rpc, token: &str) -> Result<Kelas, String> {
    let t = token.to_lowercase();
    if quote_dikenal(&t) {
        return Ok(Kelas::Quote);
    }
    let slot = rpc.quick("eth_getStorageAt", json!([t, SLOT_BEACON, "latest"]), 3000)?;
    if slot.as_str().map_or(false, |s| s.to_lowercase().ends_with(BEACON_SAHAM)) {
        return Ok(Kelas::Ekuitas);
    }
    if nama_token(rpc, &t).map_or(false, |n| n.trim_end().ends_with("Robinhood Token")) {
        return Ok(Kelas::Ekuitas);
    }
    Ok(Kelas::Meme)
}

/// Klasifikasi banyak token sekaligus: SATU batch eth_getStorageAt (beacon saham) + SATU batch name() (cadangan).
pub fn klasifikasi_batch(rpc: &Rpc, tok: &[String]) -> Result<Vec<(String, Kelas)>, String> {
    let mut out = Vec::new();
    let mut cek: Vec<String> = Vec::new();
    for t in tok {
        if quote_dikenal(t) {
            out.push((t.clone(), Kelas::Quote));
        } else {
            cek.push(t.clone());
        }
    }
    if cek.is_empty() {
        return Ok(out);
    }
    let calls: Vec<(&str, Value)> = cek.iter().map(|t| ("eth_getStorageAt", json!([t, SLOT_BEACON, "latest"]))).collect();
    let v = rpc.batch(&calls, std::time::Duration::from_secs(15))?;
    let mut sisa = Vec::new();
    for (t, r) in cek.iter().zip(v.iter()) {
        match r {
            Ok(x) if x.as_str().map_or(false, |s| s.to_lowercase().ends_with(BEACON_SAHAM)) => out.push((t.clone(), Kelas::Ekuitas)),
            Ok(_) => sisa.push(t.clone()),
            Err(_) => {} // gagal -> tidak diputuskan, diminta ulang nanti
        }
    }
    if !sisa.is_empty() {
        let calls: Vec<(&str, Value)> = sisa.iter().map(|t| ("eth_call", json!([{"to": t, "data": "0x06fdde03"}, "latest"]))).collect();
        let v = rpc.batch(&calls, std::time::Duration::from_secs(15))?;
        for (t, r) in sisa.iter().zip(v.iter()) {
            let nama = r.as_ref().ok().and_then(|x| x.as_str()).and_then(|h| {
                let h = h.trim_start_matches("0x");
                let n = usize::from_str_radix(h.get(64..128)?, 16).ok()?;
                let b: Vec<u8> = (0..n.min(200)).filter_map(|i| u8::from_str_radix(h.get(128 + 2 * i..130 + 2 * i)?, 16).ok()).collect();
                Some(String::from_utf8_lossy(&b).into_owned())
            });
            let ek = nama.map_or(false, |n| n.trim_end().ends_with("Robinhood Token"));
            out.push((t.clone(), if ek { Kelas::Ekuitas } else { Kelas::Meme }));
        }
    }
    Ok(out)
}

/// Cache klasifikasi (disimpan ke disk agar tidak memanggil RPC ulang setelah restart).
#[derive(Default)]
pub struct Cache {
    pub peta: HashMap<String, Kelas>,
}
impl Cache {
    pub fn muat(p: &Path) -> Cache {
        let mut c = Cache::default();
        if let Ok(t) = std::fs::read_to_string(p) {
            if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&t) {
                for (k, v) in o {
                    if let Some(k2) = v.as_str().and_then(Kelas::dari) {
                        c.peta.insert(k, k2);
                    }
                }
            }
        }
        c
    }
    pub fn simpan(&self, p: &Path) {
        let o: serde_json::Map<String, Value> = self.peta.iter().map(|(k, v)| (k.clone(), json!(v.nama()))).collect();
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, Value::Object(o).to_string()).is_ok() {
            let _ = std::fs::rename(tmp, p);
        }
    }
}
