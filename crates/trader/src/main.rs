//! rhc-trader: Robinhood Chain memecoin strategy with an optional live execution layer (Uniswap v4).
//!
//! The decision engine (engine + features + account rules) is the backtested one, byte-for-byte; see
//! `scripts/parity.sh`. Execution is off (paper) unless EXEC_MODE says otherwise.
//!
//! Usage:
//!   rhc-trader live   [--bot DIR] [--env FILE]                 run the strategy (paper / shadow / live per EXEC_MODE)
//!   rhc-trader sim    <data_dir> <from_block> <to_block> <out.jsonl> [--bot DIR] [--env FILE]
//!                     [--skor CSV] [--thr0 X] [--akun-in F] [--akun-out F] [--devpre CSV] [--danger F] [--naik F]
//!   rhc-trader exec selftest [--env FILE] [--pool POOL_ID]     prove the execution path on chain (simulation only)
//!   rhc-trader exec status   [--bot DIR] [--env FILE]          print the executor status
//!   rhc-trader wallet address [--env FILE]                    address and ETH balance of EXEC_KEY_FILE
//!   rhc-trader unduh|fast <from> <to> <dir> | fast1|menit1 <dir>   historical data download (research)
mod belajar;
mod bot;
mod catmodel;
mod cfg;
mod exec;
mod fast_download;
mod kelas;
mod live;
mod loggate;
mod net;
mod radar;
mod sim;
mod unduh;

const USAGE: &str = "usage:
  rhc-trader live   [--bot DIR] [--env FILE]
  rhc-trader sim    <data_dir> <from_block> <to_block> <out.jsonl> [--bot DIR] [--env FILE] [--skor CSV] [--thr0 X] [--akun-in F] [--akun-out F]
  rhc-trader exec selftest [--env FILE] [--pool POOL_ID]
  rhc-trader exec status   [--bot DIR] [--env FILE]
  rhc-trader wallet address [--env FILE]
  rhc-trader unduh|fast <from> <to> <dir> | fast1|menit1 <dir>";

/// Settings read from the process environment only (not from the .env parser). They are exported from
/// `--env FILE` at startup so a value written in the file is never silently ignored. The environment wins.
const PROCESS_ENV_KEYS: [&str; 4] = ["MAX_FEE", "DS", "RADAR_BOOT_HARI", "START_BLOCK"];

fn env_file_map(path: &str) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
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

fn setting(env_file: Option<&str>, key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty()).or_else(|| env_file.and_then(|f| env_file_map(f).remove(key))).filter(|s| !s.is_empty())
}

fn wallet_address(cfg: &cfg::Cfg, env_file: Option<&str>) -> Result<(), String> {
    let path = setting(env_file, "EXEC_KEY_FILE").ok_or("EXEC_KEY_FILE is not set")?;
    let signer = exec::wallet::load_signer(std::path::Path::new(&path))?;
    let rpc = net::Rpc::new(&cfg.rpc_urls, cfg.doh)?;
    let bal = rpc.call("eth_getBalance", serde_json::json!([format!("{:#x}", signer.address()), "latest"]))?;
    let wei = bal.as_str().and_then(|s| u128::from_str_radix(s.trim_start_matches("0x"), 16).ok()).ok_or("eth_getBalance: bad response")?;
    println!("address {:#x}\nbalance {:.6} ETH ({wei} wei)", signer.address(), wei as f64 / 1e18);
    Ok(())
}

fn exec_status(bot_dir: &str, env_file: Option<&str>) -> Result<(), String> {
    let dir = setting(env_file, "EXEC_STATE_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(bot_dir).join("rust_live").join("exec"));
    let p = dir.join("exec_status.json");
    let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    println!("{t}");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let env_file = opt("--env");
    // before any thread starts: set_var is only sound while the process is single-threaded
    if let Some(f) = &env_file {
        for (k, v) in env_file_map(f) {
            if PROCESS_ENV_KEYS.contains(&k.as_str()) && !v.is_empty() && std::env::var_os(&k).is_none() {
                std::env::set_var(&k, &v);
            }
        }
    }
    let bot_dir = opt("--bot").unwrap_or_else(|| ".".into());
    let cfg = cfg::Cfg::load(env_file.as_deref());
    let urls: Vec<String> = cfg.rpc_urls.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let r = match (args.get(1).map(|s| s.as_str()), args.get(2).map(|s| s.as_str())) {
        (Some("sim"), _) if args.len() >= 6 => sim::jalan(
            cfg,
            sim::SimArg {
                dir: args[2].clone(),
                from: args[3].parse().unwrap_or(0.0),
                to: args[4].parse().unwrap_or(f64::INFINITY),
                out: args[5].clone(),
                bot_dir,
                dev_pre: opt("--devpre"),
                skor: opt("--skor"),
                thr0: opt("--thr0").and_then(|v| v.parse().ok()),
                danger: opt("--danger"),
                naik: opt("--naik"),
                akun_in: opt("--akun-in"),
                akun_out: opt("--akun-out"),
            },
        ),
        (Some("live"), _) => live::jalan(cfg, &bot_dir, env_file.as_deref()),
        (Some("exec"), Some("selftest")) => exec::selftest::run(&urls, cfg.doh, opt("--pool").as_deref(), opt("--init-block").and_then(|b| b.parse().ok())),
        (Some("exec"), Some("status")) => exec_status(&bot_dir, env_file.as_deref()),
        (Some("wallet"), Some("address")) => wallet_address(&cfg, env_file.as_deref()),
        (Some("unduh"), _) if args.len() >= 5 => net::Rpc::new(&cfg.rpc_urls, cfg.doh).and_then(|r| unduh::unduh(&r, args[2].parse().unwrap_or(0), args[3].parse().unwrap_or(0), &args[4])),
        (Some("fast"), _) if args.len() >= 5 => net::Rpc::new(&cfg.rpc_urls, cfg.doh).and_then(|r| fast_download::download_fast(&r, args[2].parse().unwrap_or(0), args[3].parse().unwrap_or(0), &args[4])),
        (Some("fast1"), _) if args.len() >= 3 => net::Rpc::new(&cfg.rpc_urls, cfg.doh).and_then(|r| fast_download::minute1_fast(&r, &args[2])),
        (Some("menit1"), _) if args.len() >= 3 => net::Rpc::new(&cfg.rpc_urls, cfg.doh).and_then(|r| unduh::menit1(&r, &args[2])),
        _ => Err(USAGE.into()),
    };
    if let Err(e) = r {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    }
}
