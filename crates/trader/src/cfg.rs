//! Setelan bot. Dibaca dari berkas .env (KUNCI=nilai, komentar #), lalu variabel lingkungan menimpa.
//! Default = setelan UJI MAJU RESMI (backtest arena standar: $100 -> $11.372,92 Ags-Sep, Okt $208,70).
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Cfg {
    // jaringan
    pub rpc_urls: String,
    pub doh: bool,
    pub poll_ms: u64,
    pub chunk: u64,
    pub fresh_blocks: f64,
    // masuk
    pub filter: bool,
    pub qthr: f64,
    pub honeypot: bool,
    pub dev_rule: bool,
    pub larang_pemanasan: bool,
    // ukuran & risiko (aturan arena.js yang teruji)
    pub start_usd: f64,
    pub max_pos: usize,
    pub exp_max: f64,
    pub lp_cap: f64,
    pub kelly_maks: f64,
    /// floor of the quarter-Kelly fraction after 100 trades (old behaviour 0.01). Too low a floor can freeze a small
    /// account: size < MIN_POS -> no trades -> Kelly history never updates.
    pub kelly_min: f64,
    pub kill_dd: f64,
    pub rem: bool,
    pub min_pos: f64,
    pub rem_rezim: f64,
    pub rem_harian: f64,
    // keluar (mesin v4)
    pub hz: f64,
    pub danger_th: f64,
    pub up_th: f64,
    pub tp_up: f64,
    pub rug_exit: bool,
    pub trail: f64,
    pub dump_frac: f64,
    /// our own sells move the pool price: "0" off (old behaviour), "1" on, "full" also counts the entry
    pub sell_impact: String,
    /// execution latency in blocks for entry and exit (0.1 s blocks); 30 = 3 s (old behaviour)
    pub lat_blocks: f64,
    // biaya nyata
    pub gas_usd: f64,
    pub approve_usd: f64,
    pub fail_buy: f64,
    pub fail_sell: f64,
    // belajar
    pub retrain_tiap: usize,
    pub python: String,
    // operasional VPS (tanpa Claude)
    pub label: bool,
    pub telegram_token: String,
    pub telegram_chat: String,
    pub kompaksi_mb: u64,
    pub cp_tiap: f64,
}

fn baca_env_file(path: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(t) = std::fs::read_to_string(path) {
        for l in t.lines() {
            let l = l.split('#').next().unwrap_or("").trim();
            if let Some((k, v)) = l.split_once('=') {
                m.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    m
}

impl Cfg {
    pub fn load(env_file: Option<&str>) -> Cfg {
        let file = env_file.map(baca_env_file).unwrap_or_default();
        let get = |k: &str| -> Option<String> { std::env::var(k).ok().filter(|s| !s.is_empty()).or_else(|| file.get(k).cloned()) };
        let num = |k: &str, d: f64| -> f64 { get(k).and_then(|s| s.parse().ok()).unwrap_or(d) };
        let on = |k: &str, d: bool| -> bool { get(k).map(|s| s == "1" || s.eq_ignore_ascii_case("true")).unwrap_or(d) };
        Cfg {
            rpc_urls: get("RPC_URLS").unwrap_or("https://rpc.mainnet.chain.robinhood.com".into()),
            doh: get("DOH_URL").map(|s| !s.is_empty()).unwrap_or(true),
            poll_ms: num("POLL_MS", 300.0) as u64,
            chunk: num("CHUNK", 1200.0) as u64,
            fresh_blocks: num("FRESH_BLOCKS", 300.0),
            filter: on("FILTER", true),
            qthr: num("QTHR", 0.90),
            honeypot: on("HONEYPOT", true),
            dev_rule: on("DEV_RULE", true),
            larang_pemanasan: on("LARANG_PEMANASAN", false),
            start_usd: num("START_USD", 100.0),
            max_pos: num("MAX_POS", 10.0) as usize,
            exp_max: num("EXP_MAX", 0.5),
            lp_cap: num("LP_CAP", 0.01),
            kelly_maks: num("KELLY_MAKS", 0.10),
            kelly_min: num("KELLY_MIN", 0.01),
            kill_dd: num("KILL_DD", 0.60),
            rem: on("REM", true),
            min_pos: num("MIN_POS", 0.25),
            rem_rezim: num("REMREZIM", 0.0),
            rem_harian: num("REM_HARIAN", 0.0),
            hz: num("HZ_LIVE", 324000.0),
            danger_th: num("DANGER_TH", 0.4),
            up_th: num("UP_TH", 0.5),
            tp_up: num("TP_UP", 0.5),
            rug_exit: on("TUTUP_SAAT_RUG", false),
            trail: num("TRAIL_PUNCAK", 0.0),
            dump_frac: num("DUMP_FRAC", 0.05),
            sell_impact: get("SELL_IMPACT").unwrap_or("0".into()),
            lat_blocks: num("LAT_BLOCKS", 30.0),
            gas_usd: num("GAS_USD", 0.06),
            approve_usd: num("APPROVE_USD", 0.03),
            fail_buy: num("FAIL_BUY", 0.0185),
            fail_sell: num("FAIL_SELL", 0.0054),
            retrain_tiap: num("RETRAIN_TIAP", 0.0) as usize,
            python: get("PYTHON").unwrap_or("python".into()),
            label: on("LABEL", true),
            telegram_token: get("TELEGRAM_TOKEN").unwrap_or_default(),
            telegram_chat: get("TELEGRAM_CHAT_ID").unwrap_or_default(),
            kompaksi_mb: num("KOMPAKSI_MB", 300.0) as u64,
            cp_tiap: num("CHECKPOINT_BLOK", 36000.0),
        }
    }
}
