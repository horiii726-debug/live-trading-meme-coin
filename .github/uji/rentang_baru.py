#!/usr/bin/env python3
"""rentang_baru.py <first block> -> prints "FROM TO END TIP" for a fresh window that ends 9.5 h before the chain tip
(so every coin born in it has its full engine horizon of data), END = replay end block = TO + 324000 (capped at the tip).
"""
import json
import os
import sys
import time
import urllib.request

URL = os.environ.get("RPC_URLS", "https://rpc.mainnet.chain.robinhood.com").split(",")[0]


def rpc(method, params):
    for i in range(8):  # the public RPC answers 429 under load; Cloudflare refuses urllib's default User-Agent
        try:
            q = urllib.request.Request(URL, json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
                                       {"content-type": "application/json", "user-agent": "curl/8.5.0"})
            return json.load(urllib.request.urlopen(q, timeout=30))["result"]
        except Exception as e:  # noqa: BLE001 - retried, then fatal
            print(f"{method}: {e}, retry", file=sys.stderr)
            time.sleep(3 * (i + 1))
    raise SystemExit(f"{method}: RPC unreachable")


tip = int(rpc("eth_blockNumber", []), 16)
ts = lambda b: int(rpc("eth_getBlockByNumber", [hex(b), False])["timestamp"], 16)  # noqa: E731
bps = 1_000_000 / (ts(tip) - ts(tip - 1_000_000))
frm = int(sys.argv[1])
to = tip - int(9.5 * 3600 * bps)
print(frm, to, min(to + 324000, tip), tip)
