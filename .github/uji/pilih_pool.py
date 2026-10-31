#!/usr/bin/env python3
"""pilih_pool.py <rpc> <tip>: native-ETH memecoin pools most traded in the last blocks, busiest first: "pool@init_block"."""
import collections
import json
import sys
import time
import urllib.request

URL, TIP = sys.argv[1], int(sys.argv[2])
PM = "0x8366a39cc670b4001a1121b8f6a443a643e40951"
T_SWAP = "0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f"
T_INIT = "0xdd466e674ea557f56295e2d0218a125ea4b4f0f6f3307b95f85e6110838d6438"
ETH_USDG = "0x24107d152f14a76d292123265ae3f3c71f863fc2f4ef7ba49d64e78d28ea379e"


def rpc(method, params):
    for i in range(10):
        try:
            q = urllib.request.Request(URL, json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
                                       {"content-type": "application/json", "user-agent": "curl/8.5.0"})
            r = json.load(urllib.request.urlopen(q, timeout=30))
            if "result" in r:
                return r["result"]
            print(method, r.get("error"), file=sys.stderr)
        except Exception as e:  # noqa: BLE001 - the public RPC answers 429 under load: wait and retry
            print(method, e, file=sys.stderr)
        time.sleep(2 * (i + 1))
    raise SystemExit(f"{method}: RPC unreachable")


n = collections.Counter()
for a in range(TIP - 12000, TIP - 200, 1000):
    for log in rpc("eth_getLogs", [{"address": PM, "topics": [T_SWAP], "fromBlock": hex(a), "toBlock": hex(a + 999)}]):
        n[log["topics"][1]] += 1
for pool, k in n.most_common(40):
    if pool == ETH_USDG or k < 5:
        continue
    ini = rpc("eth_getLogs", [{"address": PM, "topics": [T_INIT, pool], "fromBlock": hex(TIP - 9_900_000), "toBlock": hex(TIP)}])
    if ini:
        print(f"{pool}@{int(ini[0]['blockNumber'], 16)}")
