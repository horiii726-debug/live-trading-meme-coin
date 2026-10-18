//! Cek koneksi bot: DoH + eth_chainId + 10x eth_blockNumber (sambungan dipakai ulang). Hanya baca.
#[path = "../net.rs"]
mod net;
fn main() {
    let url = std::env::var("RPC_URLS").unwrap_or("https://rpc.mainnet.chain.robinhood.com".into());
    let t = std::time::Instant::now();
    let rpc = net::Rpc::new(&url, true).expect("rpc");
    println!("eth_chainId {:?} ({} ms termasuk DoH+TLS)", rpc.call("eth_chainId", serde_json::json!([])), t.elapsed().as_millis());
    let mut ms = vec![];
    for _ in 0..10 {
        let t = std::time::Instant::now();
        let r = rpc.call("eth_blockNumber", serde_json::json!([]));
        ms.push(t.elapsed().as_millis());
        if r.is_err() {
            println!("{r:?}");
        }
    }
    ms.sort();
    println!("eth_blockNumber x10: median {} ms, maks {} ms", ms[5], ms[9]);
}
