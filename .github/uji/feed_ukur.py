#!/usr/bin/env python3
"""How much earlier does Robinhood Chain's sequencer feed show a block than the RPC the bot polls?

    feed_ukur.py <seconds> > feed.json

The feed (Arbitrum Nitro broadcaster, wss://feed.mainnet.chain.robinhood.com) streams every sequenced message the
moment the sequencer orders it. The RPC shows the same block only once its node has received and executed it. For
every block produced while this runs: first time the feed delivered it vs first time the RPC reported it as head.
The RPC is polled every 200 ms, so its time is late by up to 200 ms (about 100 ms on average) on top of the real gap.
Measuring only; the bot is not involved.
"""
import asyncio
import json
import statistics
import sys
import threading
import time
import urllib.request

import websockets

FEED = "wss://feed.mainnet.chain.robinhood.com"
RPC = "https://rpc.mainnet.chain.robinhood.com"
DUR = float(sys.argv[1]) if len(sys.argv) > 1 else 300
POLL = 0.2

feed = {}       # sequence number -> (receive time, block hash or None)
rpc_seen = {}   # block number -> first time the RPC reported it (or a later block) as head
info = {"rpc_poll": 0, "rpc_429": 0, "rpc_err": 0, "feed_pesan": 0, "contoh": None}
mulai = time.time()
stop = mulai + DUR


def rpc(method, params):
    req = urllib.request.Request(RPC, data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
                                 headers={"content-type": "application/json", "user-agent": "rhc-uji-feed"})
    with urllib.request.urlopen(req, timeout=5) as r:
        return json.loads(r.read())


def poller():
    last = None
    while time.time() < stop:
        t0 = time.time()
        try:
            d = rpc("eth_blockNumber", [])
            t = (t0 + time.time()) / 2  # the node answered somewhere in between
            info["rpc_poll"] += 1
            if "result" in d:
                b = int(d["result"], 16)
                if last is not None:  # blocks that already existed when we started are not timed
                    for x in range(last + 1, b + 1):
                        rpc_seen.setdefault(x, t)
                last = b if last is None else max(last, b)
            else:
                info["rpc_err"] += 1
        except Exception as e:  # noqa: BLE001
            info["rpc_429" if "429" in str(e) else "rpc_err"] += 1
        time.sleep(max(0.0, POLL - (time.time() - t0)))


async def baca_feed():
    async with websockets.connect(FEED, max_size=None, open_timeout=20) as ws:
        while time.time() < stop:
            try:
                raw = await asyncio.wait_for(ws.recv(), timeout=max(0.1, stop - time.time()))
            except asyncio.TimeoutError:
                break
            t = time.time()
            m = json.loads(raw)
            if info["contoh"] is None:
                info["contoh"] = str(raw)[:600]
            for x in m.get("messages") or []:
                info["feed_pesan"] += 1
                feed.setdefault(x["sequenceNumber"], (t, x.get("blockHash")))


th = threading.Thread(target=poller, daemon=True)
th.start()
galat_feed = None
try:
    asyncio.run(baca_feed())
except Exception as e:  # noqa: BLE001
    galat_feed = f"{type(e).__name__}: {e}"
th.join(timeout=10)

out = {"feed": FEED, "detik": DUR, "galat_feed": galat_feed, **info}
# block number = sequence number + offset; found from the block hash the feed carries
offset = None
for s, (_, h) in sorted(feed.items())[-50:]:
    if h:
        try:
            b = rpc("eth_getBlockByHash", [h, False]).get("result")
            if b:
                offset = int(b["number"], 16) - s
                break
        except Exception:  # noqa: BLE001
            pass
out["offset_blok"] = offset
if offset is not None:
    d = sorted((rpc_seen[s + offset] - t) * 1000 for s, (t, _) in feed.items() if s + offset in rpc_seen)
    if d:
        q = lambda p: d[min(len(d) - 1, int(p * len(d)))]  # noqa: E731
        out["blok_dibandingkan"] = len(d)
        out["feed_lebih_cepat_ms"] = {"median": round(statistics.median(d)), "p10": round(q(0.1)), "p90": round(q(0.9)),
                                      "min": round(d[0]), "maks": round(d[-1])}
        out["catatan"] = "positif = feed lebih dulu; angka RPC termasuk jeda polling 200 ms (rata-rata ~100 ms)"
json.dump(out, sys.stdout, indent=1)
print()
