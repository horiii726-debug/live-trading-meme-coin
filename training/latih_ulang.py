# LATIH ULANG MODEL FILTER DENGAN GERBANG MUTU (jalan otomatis di VPS, tanpa Claude).
# Model baru HANYA dipakai kalau terbukti tidak lebih buruk dari model yang sedang jalan di data TERBARU yang
# tidak ikut dilatih (holdout kronologis). Kalau gagal gerbang, model lama tetap dipakai. Semua keputusan dicatat.
#
# Data latih  = riwayat uji maju (data/arena_feats*.csv) + label live bot Rust (bot/rust_live/labels.jsonl:
#               fitur menit-1 + hasil simulasi bayangan berukuran tetap, sama dengan label backtest).
# Gerbang     = di holdout (30% label live terbaru, min. 150 koin): rata-rata ret 10% skor teratas kandidat >= model lama
#               DAN AUC kandidat >= AUC lama - 0,01.
# Bot Rust memuat model baru otomatis (cek kesamaan Rust vs Python wajib lolos). Hanya model FILTER yang dilatih ulang.
import json, os, sys, time, shutil
import numpy as np, pandas as pd
from catboost import CatBoostClassifier
from sklearn.metrics import roc_auc_score

HERE = os.path.dirname(os.path.abspath(__file__))
BOT = os.path.abspath(os.environ.get("BOT_DIR", os.path.join(HERE, "..", "bot")))
DATA = os.path.abspath(os.environ.get("DATA_DIR", os.path.join(HERE, "..", "data")))
LABEL = os.path.join(BOT, "rust_live", "labels.jsonl")
LOG = os.path.join(BOT, "rust_live", "latih_ulang.jsonl")
MIN_LIVE = int(os.environ.get("MIN_LIVE", "300"))
MIN_HOLD = int(os.environ.get("MIN_HOLD", "150"))


def catat(**k):
    k["waktu"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    print(json.dumps(k))
    with open(LOG, "a", encoding="utf8") as f:
        f.write(json.dumps(k) + "\n")


def prediksi(model, X):
    """Prediktor oblivious tree identik dengan Rust (perbandingan fitur dalam float32)."""
    X32 = X.astype(np.float32)
    s = np.zeros(len(X))
    for t in model["trees"]:
        idx = np.zeros(len(X), dtype=np.int64)
        for k, sp in enumerate(t["splits"]):
            idx |= (X32[:, sp["feat"]] > np.float32(sp["border"])).astype(np.int64) << k
        s += np.asarray(t["leaf"])[idx]
    z = model.get("scale", 1.0) * s + model.get("bias", 0.0)
    return 1.0 / (1.0 + np.exp(-z))


def metrik(p, ret):
    win = ret > 1
    auc = roc_auc_score(win, p) if len(set(win)) > 1 else float("nan")
    top = p >= np.quantile(p, 0.90)
    return {"auc": round(float(auc), 4), "ret_top10": round(float(ret[top].mean()), 4), "n_top": int(top.sum())}


def ekspor(m, FE, X_ck):
    tmp = os.path.join(BOT, "cat_raw_tmp.json")
    m.save_model(tmp, format="json")
    j = json.load(open(tmp))
    os.remove(tmp)
    trees = [{"splits": [{"feat": int(s["float_feature_index"]), "border": float(s["border"])} for s in t["splits"]], "leaf": [float(v) for v in t["leaf_values"]]} for t in j["oblivious_trees"]]
    sc = j["scale_and_bias"]
    bias = sc[1][0] if isinstance(sc[1], list) else sc[1]
    return {"type": "catboost", "feats": FE, "trees": trees, "scale": float(sc[0]), "bias": float(bias)}


def main():
    cur = json.load(open(os.path.join(BOT, "filter_cat.json")))
    FE = cur["feats"]
    if not os.path.exists(LABEL):
        return catat(hasil="lewati", alasan="belum ada labels.jsonl")
    L = []
    for l in open(LABEL, encoding="utf8"):
        try:
            d = json.loads(l)
            if all(k in d["f"] for k in FE) and np.isfinite(d["ret"]):
                L.append({**{k: d["f"][k] for k in FE}, "ret": d["ret"], "decide": d["decide"], "pool": d["pool"]})
        except Exception:
            pass
    live = pd.DataFrame(L).drop_duplicates("pool", keep="last").sort_values("decide").reset_index(drop=True)
    if len(live) < MIN_LIVE:
        return catat(hasil="lewati", alasan=f"label live baru {len(live)} (< {MIN_LIVE})")
    n_hold = max(MIN_HOLD, int(0.3 * len(live)))
    if len(live) - n_hold < 50:
        return catat(hasil="lewati", alasan="label live belum cukup untuk dibagi latih/holdout")
    hold, live_latih = live.iloc[-n_hold:], live.iloc[:-n_hold]
    base = []
    for f in ["arena_feats.csv", "arena_feats_okt.csv"]:
        p = os.path.join(DATA, f)
        if os.path.exists(p):
            base.append(pd.read_csv(p)[FE + ["ret"]])
    latih = pd.concat(base + [live_latih[FE + ["ret"]]], ignore_index=True)
    X, y = latih[FE].fillna(-1).values, (latih.ret > 1).values
    Xh, rh = hold[FE].fillna(-1).values, hold.ret.values
    m = CatBoostClassifier(iterations=200, learning_rate=0.05, depth=4, l2_leaf_reg=5.0, random_seed=0, verbose=0, allow_writing_files=False)
    m.fit(X, y)
    kand = ekspor(m, FE, None)
    p_kand, p_lama = prediksi(kand, Xh), prediksi(cur, Xh)
    mk, ml = metrik(p_kand, rh), metrik(p_lama, rh)
    lolos = mk["ret_top10"] >= ml["ret_top10"] and mk["auc"] >= ml["auc"] - 0.01
    info = dict(n_latih=int(len(latih)), n_live_latih=int(len(live_latih)), n_holdout=int(n_hold), kandidat=mk, lama=ml)
    if not lolos:
        return catat(hasil="DITOLAK (model lama tetap)", **info)
    # cek kesamaan: prediktor Python (sama dengan Rust) vs CatBoost asli
    idx = np.random.RandomState(0).choice(len(X), min(500, len(X)), replace=False)
    p_cb = m.predict_proba(X[idx])[:, 1]
    selisih = float(np.abs(prediksi(kand, X[idx]) - p_cb).max())
    if selisih > 1e-9:
        return catat(hasil="DITOLAK (ekspor tidak sama)", selisih=selisih, **info)
    kand.update({"thr_start": float(np.quantile(p_kand, 0.90)), "n_latih": int(len(latih)), "waktu": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "gerbang": {"kandidat": mk, "lama": ml}})
    arsip = os.path.join(BOT, "arsip_model")
    os.makedirs(arsip, exist_ok=True)
    tag = time.strftime("%Y%m%d_%H%M%S", time.gmtime())
    for f in ["filter_cat.json", "filter_cat_check.json"]:
        shutil.copy2(os.path.join(BOT, f), os.path.join(arsip, f"{tag}_{f}"))
    # tulis CEK dulu, baru model (bot memuat ulang saat berkas model berubah)
    ck = {"X": X[idx].tolist(), "p": p_cb.tolist()}
    for nama, isi in [("filter_cat_check.json", ck), ("filter_cat.json", kand)]:
        t = os.path.join(BOT, nama + ".tmp")
        json.dump(isi, open(t, "w"))
        os.replace(t, os.path.join(BOT, nama))
    catat(hasil="DITERIMA (model baru dipasang)", arsip=tag, **info)


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        catat(hasil="ERROR (model lama tetap)", pesan=str(e)[:200])
        sys.exit(1)
