//! TOTAL KILL SWITCH (owner's rule): if any part of OUR system errs while the bot runs, no order of any kind (buy,
//! sell, approve) may be sent any more, until the owner looks and resumes it himself.
//!
//! On:  only INTERNAL errors: a panic in a trading thread (strategy, minute-1 checks, learner, executor), a non-finite
//!      filter score or learner state, an ML retrain that fails or fails its Rust-vs-CatBoost check, the CatBoost
//!      program missing, account numbers that cannot be right, the execution ledger or the learner state failing to save.
//! Off: everything outside that the bot already handles by itself: RPC 429 / timeouts / connection errors, one coin's
//!      check failing (that coin is skipped), a buy reverting on slippage, a sell being retried. Those must never stop
//!      a healthy bot.
//!
//! When on: `Sender::broadcast`, the one path to the chain, refuses every transaction; the executor stops buying,
//! selling and approving; the strategy loop stops; status.json, live.log, `rhc.sh status` and the SSH login banner say
//! why. The state is the file rust_live/KILL_SWITCH, so a crash, a systemd restart or a reboot keeps it on. Only the
//! owner clears it: `sudo ./rhc.sh lanjut` (or `sudo ./rhc.sh jual-semua`: sell everything, buys stay blocked).
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

pub const BERKAS: &str = "KILL_SWITCH";

static AKTIF: AtomicBool = AtomicBool::new(false);
static JALUR: OnceLock<PathBuf> = OnceLock::new();
type Peringatan = Box<dyn Fn(&str) + Send + Sync>;
static PERINGATAN: OnceLock<Peringatan> = OnceLock::new();

/// First thing at start: where the switch lives. If the file is there the switch is ON (it survives restarts) and its
/// first line (the original reason) is returned.
pub fn siapkan(dir: &Path) -> Option<String> {
    let p = dir.join(BERKAS);
    let _ = JALUR.set(p.clone());
    if p.exists() {
        AKTIF.store(true, Ordering::SeqCst);
        return Some(alasan().unwrap_or_else(|| "(alasan tidak terbaca)".into()));
    }
    None
}

/// How to tell the owner (live.log; Telegram only if he configured it).
pub fn pasang_peringatan(f: Peringatan) {
    let _ = PERINGATAN.set(f);
}

pub fn aktif() -> bool {
    AKTIF.load(Ordering::SeqCst)
}

/// Turn the switch on. Blocks every send at once, then records the reason (a later trigger only adds its line).
/// Safe to call from the panic hook and from any thread.
pub fn picu(alasan: &str) {
    let pertama = !AKTIF.swap(true, Ordering::SeqCst);
    let baris = format!("{} | {}", crate::live::sekarang_iso(), alasan.replace(['\n', '\r'], " "));
    if let Some(p) = JALUR.get() {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{baris}");
            let _ = f.sync_all();
        }
    }
    eprintln!("KILL SWITCH: {baris}");
    if pertama {
        if let Some(w) = PERINGATAN.get() {
            w(&format!(
                "KILL SWITCH AKTIF: semua order dihentikan (beli, jual, approve). Alasan: {alasan}. Cek: sudo ./rhc.sh status | lanjutkan setelah dicek: sudo ./rhc.sh lanjut | jual semua posisi: sudo ./rhc.sh jual-semua"
            ));
        }
    }
}

/// The first recorded reason (the error that stopped the bot).
pub fn alasan() -> Option<String> {
    let t = std::fs::read_to_string(JALUR.get()?).ok()?;
    t.lines().next().map(|l| l.to_string())
}

#[cfg(test)]
pub fn uji_matikan() {
    AKTIF.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_blocks_every_broadcast_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("kill_uji_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(siapkan(&dir), None, "no file: the switch starts off");
        assert!(!aktif());
        // a healthy bot sends: the switch must not get in the way (the endpoint below does not exist; the point is
        // that the send was attempted and not refused by the switch)
        let key = crate::exec::wallet::parse_key("0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80").unwrap();
        let mut s = crate::exec::sender::Sender::new(key.clone(), &["https://127.0.0.1:9".to_string()], false).unwrap();
        let to = alloy_primitives::Address::ZERO;
        let tx = crate::exec::sender::sign_eip1559(&key, 0, to, alloy_primitives::U256::ZERO, alloy_primitives::Bytes::new(), 21_000, 1, 0).unwrap();
        match s.broadcast(&tx) {
            crate::exec::sender::SendOutcome::Rejected { error, .. } => assert!(!error.contains("KILL SWITCH"), "switch off: {error}"),
            _ => {}
        }
        picu("uji: panic sengaja");
        assert!(aktif());
        assert!(dir.join(BERKAS).exists(), "the reason is written to disk");
        match s.broadcast(&tx) {
            crate::exec::sender::SendOutcome::Rejected { error, maybe_sent } => {
                assert!(error.contains("KILL SWITCH"), "{error}");
                assert!(!maybe_sent, "nothing went out");
            }
            o => panic!("a send got through the kill switch: {o:?}"),
        }
        // a restart reads the file: still on, original reason kept
        uji_matikan();
        let r = siapkan(&dir).expect("still on after a restart");
        assert!(r.contains("uji: panic sengaja"), "{r}");
        assert!(aktif());
        uji_matikan();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
