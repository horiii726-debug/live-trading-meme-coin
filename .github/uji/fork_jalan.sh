#!/usr/bin/env bash
# Test only: real signed transactions from the real executor against a forked copy of Robinhood Chain.
# Nothing here touches the real chain: anvil keeps the fork in memory and only READS real state from the public RPC.
#   fork_jalan.sh <path to rhc-trader built with the e2e subcommand>
set -uo pipefail
BIN=$1
ROOT=$PWD
PUB=https://rpc.mainnet.chain.robinhood.com
W=/tmp/e2e
mkdir -p "$W/tls"
say(){ printf '\n=== %s\n' "$*"; }

say "anvil: fork of the live chain"
tip=$(curl -s -m 20 -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}' $PUB | python3 -c "import json,sys;print(int(json.load(sys.stdin)['result'],16))")
FORK=$((tip - 50))
echo "tip $tip, fork at $FORK"
anvil --fork-url "$PUB" --fork-block-number "$FORK" --chain-id 4663 --port 8545 --host 127.0.0.1 --retries 30 --fork-retry-backoff 1500 --no-storage-caching > "$W/anvil.log" 2>&1 &
for i in $(seq 1 120); do
    r=$(curl -s -m 3 -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' http://127.0.0.1:8545) && [ -n "$r" ] && break
    sleep 1
done
echo "anvil: $r"
echo "$r" | grep -q 0x1237 || { echo "GAGAL: anvil tidak hidup"; tail -30 "$W/anvil.log"; exit 1; }

say "TLS in front of anvil (the bot speaks only https on 443)"
cd "$W/tls"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -keyout ca.key -out ca.crt -subj "/CN=uji-ca" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -keyout srv.key -out srv.csr -subj "/CN=fork1.uji.local" 2>/dev/null
printf 'subjectAltName=DNS:fork1.uji.local,DNS:fork2.uji.local,DNS:fork3.uji.local\n' > ext
openssl x509 -req -in srv.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 2 -extfile ext -out srv.crt 2>/dev/null
cat srv.crt srv.key > srv.pem
sudo cp ca.crt /usr/local/share/ca-certificates/uji-ca.crt
sudo update-ca-certificates >/dev/null
# fork3 points at an address nothing listens on: a dead first endpoint
printf '127.0.0.1 fork1.uji.local fork2.uji.local\n127.0.0.2 fork3.uji.local\n' | sudo tee -a /etc/hosts >/dev/null
sudo nohup socat OPENSSL-LISTEN:443,bind=127.0.0.1,fork,reuseaddr,cert="$W/tls/srv.pem",verify=0 TCP:127.0.0.1:8545 > "$W/socat.log" 2>&1 &
sleep 2
curl -s -m 5 -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}' https://fork1.uji.local; echo
cd "$ROOT"

say "a fresh wallet with about \$100 of ETH on the fork"
openssl rand -hex 32 > "$W/wallet.key"; chmod 600 "$W/wallet.key"
ADDR=$(cast wallet address --private-key "0x$(cat "$W/wallet.key")")
BAL=$(python3 -c "print(hex(int(100/2220*1e18)))")
curl -s -H 'content-type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"anvil_setBalance\",\"params\":[\"$ADDR\",\"$BAL\"]}" http://127.0.0.1:8545 >/dev/null
echo "wallet $ADDR, balance $BAL"

# the installed settings, exactly, with only the RPC, mode and wallet set
sed -e "s#^RPC_URLS=.*#RPC_URLS=https://fork1.uji.local,https://fork2.uji.local#" -e "s#^EXEC_MODE=.*#EXEC_MODE=live#" \
    -e "s#^EXEC_KEY_FILE=.*#EXEC_KEY_FILE=$W/wallet.key#" -e "s#^KELLY_SEED=.*#KELLY_SEED=$ROOT/models/kelly_seed_backtest_last100.json#" -e "s@^#*EXEC_LIVE_CONFIRM=.*@EXEC_LIVE_CONFIRM=I_ACCEPT_REAL_MONEY_RISK@" \
    config/trader.env.example > "$W/fork.env"
sed -e "s#^RPC_URLS=.*#RPC_URLS=$PUB#" "$W/fork.env" > "$W/pub.env"

say "pick real memecoin pools: most traded in the last blocks, then the bot's own selftest on each"
python3 .github/uji/pilih_pool.py "$PUB" "$tip" > "$W/calon.txt"
wc -l "$W/calon.txt"
: > "$W/pool.txt"
while read -r p; do
    if timeout 120 "$BIN" exec selftest --env "$W/pub.env" --pool "${p%@*}" --init-block "${p#*@}" > "$W/st.txt" 2>&1 && grep -q "SELFTEST OK" "$W/st.txt"; then
        echo "$p" >> "$W/pool.txt"; tail -3 "$W/st.txt"
    else
        echo "pool $p ditolak selftest: $(tail -1 "$W/st.txt")"
    fi
    [ "$(wc -l < "$W/pool.txt")" -ge 3 ] && break
done < "$W/calon.txt"
P1=$(sed -n 1p "$W/pool.txt"); P2=$(sed -n 2p "$W/pool.txt"); P3=$(sed -n 3p "$W/pool.txt")
[ -n "$P3" ] || { echo "GAGAL: kurang dari 3 pool lolos selftest"; exit 1; }
echo "pools: $P1 $P2 $P3"

export E2E_KEY_RPC=$PUB E2E_KEY_FILE=$W/wallet.key
gagal=0
jalan(){  # jalan <scenario> <env file> <state name> <pool> <pool2>
    local s=$1 env=$2 d=$W/s_$3
    mkdir -p "$d"
    say "SKENARIO $s"
    EXEC_STATE_DIR=$d/exec timeout 420 "$BIN" exec e2e "$s" --env "$env" --bot "$d" --pool "$4" --pool2 "${5:-}" 2>&1 | tee "$d/$s.log"
    local rc=${PIPESTATUS[0]}
    if ! grep -q "SKENARIO $s: LULUS" "$d/$s.log" && [ "${6:-}" != mati ]; then echo "SKENARIO $s: GAGAL (exit $rc)"; gagal=$((gagal + 1)); fi
    rm -f "$d/exec/positions.json.tmp"
}
jalan normal "$W/fork.env" normal "$P1"
jalan banyak "$W/fork.env" banyak "$P2" "$P3"
jalan beli_gagal "$W/fork.env" belig "$P1"
jalan jual_basi "$W/fork.env" jbasi "$P2"
jalan nonce "$W/fork.env" nonce "$P1"
jalan tidak_bisa_jual "$W/fork.env" stuck "$P3"
jalan stop_beli "$W/fork.env" stopb "$P1" "$P2"
jalan jual_semua "$W/fork.env" flat "$P2"
jalan kill_nyala "$W/fork.env" kill "$P1"
say "pemilik membuka kill switch (seperti rhc.sh jual-semua), lalu menjual"
rm -f "$W/s_kill/KILL_SWITCH"
jalan kill_lanjut "$W/fork.env" kill "$P1"
say "proses mati paksa di tengah transaksi, lalu restart"
jalan mati "$W/fork.env" mati "$P2" "" mati
sleep 1
jalan pulih "$W/fork.env" mati "$P2"
jalan tx_hilang "$W/fork.env" hilang "$P1" "$P3"
curl -s -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"evm_setAutomine","params":[true]}' http://127.0.0.1:8545 >/dev/null
say "endpoint pertama mati: semua harus lewat endpoint kedua"
sed -e "s#^RPC_URLS=.*#RPC_URLS=https://fork3.uji.local,https://fork1.uji.local#" "$W/fork.env" > "$W/mati1.env"
jalan normal "$W/mati1.env" ep "$P3"

say "akhir: saldo ETH wallet, nonce"
curl -s -H 'content-type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"eth_getBalance\",\"params\":[\"$ADDR\",\"latest\"]}" http://127.0.0.1:8545; echo
curl -s -H 'content-type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"eth_getTransactionCount\",\"params\":[\"$ADDR\",\"latest\"]}" http://127.0.0.1:8545; echo
grep -h "SKENARIO .*: \(LULUS\|GAGAL\)" "$W"/s_*/*.log | cut -c1-120
echo "gagal: $gagal"
mkdir -p "$ROOT/hasil"; cp -r "$W"/s_* "$W/anvil.log" "$ROOT/hasil/" 2>/dev/null; rm -f "$ROOT/hasil/wallet.key"
exit "$gagal"
