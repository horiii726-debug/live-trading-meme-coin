# REVERSE ENGINEERING — bot rhc-trader (Robinhood Chain 4663)

Dokumen ini membedah **setiap komponen** bot: cara kerja, rumus, setelan, hasil uji di data nyata, dan contoh koin.
Sumber: kode di repo ini (commit `aed3cf3` + perbaikan di branch `v2`) dan uji 8 Nov 2026 (detail angka: `riset/`).
Semua angka hasil uji berasal dari perintah yang dijalankan di data rantai nyata; yang belum diuji ditulis "belum diuji".

Data uji:

| Kode | Periode (perkiraan) | Kolam | Layak dibeli* | Catatan |
|---|---|---|---|---|
| ags1, ags4 | Agustus | 3.526 / 2.337 | 755 / 628 | data dompet menit-1 hanya untuk koin yang dulu dibeli |
| u_ds5d, u_ds24 | awal–pertengahan September | 10.898 / 15.582 | 437 / 1.200 | idem |
| m1, m2 | ±14–19 September | 31.092 / 14.958 | 2.073 / 1.001 | idem |
| 23 jam (h23) | 1–2 Nov | 3.168 | 1.012 | lengkap (data dompet semua koin) |
| 2 Nov (n2) | hari live 2 Nov 14:34–20:57 UTC | 1.530 | 357–361 | dirakit dari store bot live + skor asli live |

*Layak = pasangan ETH, fee ≤ 1%, tanpa hook, belum rug di detik 60, ada swap di menit 1.

---

## 0. Peta alur

```
Robinhood Chain (blok 0,1 dtk)
  └─ [K1] polling eth_getLogs PoolManager tiap 300 ms → log gate (tepat sekali, berurutan) → store/*.csv
       ├─ Initialize  → kolam baru → [K2] filter kolam (ETH, fee, hook) + [K3] klasifikasi aset
       ├─ ModifyLiquidity → likuiditas (deteksi LP ditarik / rug)
       └─ Swap        → harga, volume
  └─ detik 60 (init+600 blok): [K4] dev pembuat + [K5] 34 fitur menit-1
       → [K6] skor = (ML CatBoost + DL)/2   → [K7] ambang relatif (top 1-QTHR dari 100 skor terakhir)
       → [K8] gerbang beli: dikecualikan → ML → dev → umur → dompet perug → honeypot → stop darurat → slot →
              rem harian → eksposur → [K9] ukuran ¼ Kelly × rem drawdown → batas 1% LP → min posisi
  └─ beli di init+600+LAT_BLOCKS → [K10] mesin keluar v4 (rug / LP>30% / dump / tanda pucuk / momentum / stop -40%
       / TP1 / model bahaya / trailing / stop bencana) → [K11] model bahaya & p_naik
  └─ [K12] eksekusi (off / shadow / live) + ledger + [K13] kill switch
  └─ [K14] simulasi bayangan semua koin → label → [K6] latih ulang
  └─ [K15] radar (hanya memantau, TIDAK membeli)
```

---

## K1. Pengumpul data + log gate (`live.rs`, `loggate.rs`, `net.rs`)

**Cara kerja.** Satu permintaan batch `[eth_blockNumber, eth_getLogs(PoolManager, Initialize/Swap/ModifyLiquidity)]`
tiap `POLL_MS`=300 ms. Log gate membaca ulang beberapa blok terakhir (overlap), membuang log di luar rentang, log
dihapus, dan log ganda; log yang terlambat dikirim di blok berikutnya, tidak pernah keluar urutan.

**Kenapa perlu.** RPC publik mengulang ±2–3% event (LP ganda membuat kolam kosong terlihat berisi).

**Hasil.** Bekerja benar (unit test lulus). Uji live kertas 8 Nov: event mengalir; Goldsky sesekali `http 429`, bot
mencoba ulang sendiri. Batas `eth_getLogs` Goldsky 800 blok.

## K2. Filter kolam (`engine.rs` `safe()`)

**Rumus.** Kolam boleh dilacak bila `(c0 = ETH atau c1 = ETH) dan fee ≤ MAX_FEE` dan bit ke-7 alamat hook = 0
(tanpa hook beforeSwap). `MAX_FEE` bawaan 5000 (0,5%), live 10000 (1%).

**Data.** Dari 3.168 kolam di 23 jam, 1.012 layak; sisanya bukan pasangan ETH, fee > 1%, ber-hook, atau mati.
Contoh live 8 Nov yang tidak layak: `0x2d3f6926…` (fee 750000 = 75%), `0xfc4de3b0…` (hook `0x4e3468…`).

## K3. Klasifikasi aset (`kelas.rs`)

Saham ter-tokenisasi Robinhood (BeaconProxy `0xe10b6f6b…`, nama "<Perusahaan> • Robinhood Token") dikecualikan; ETH,
WETH, USDG = alat tukar; sisanya meme. Kegagalan klasifikasi → tidak dibeli. **Belum diuji terpisah** (jarang terjadi).

## K4. Dev pembuat (`live.rs`)

`dev = tx.from` dari mint pertama token. Pencarian gagal → koin tidak dibeli ("dev tidak diketahui").
**Masalah:** perug memakai dompet baru tiap koin (3 rug live 2 Nov: dev `0x31a253…`, `0x8a9d4a…`, `0x6d755d…`,
semuanya baru), jadi aturan dev [K8] tidak pernah kena.

## K5. 34 fitur menit-1 (`crates/features/src/features.rs`)

Dihitung dari swap dan transfer token sampai detik 60. q = sisi ETH; `buy` = pemanggil menerima token.

| Fitur | Rumus |
|---|---|
| `fee`, `hook` | fee kolam (ppm); 1 bila ada hook |
| `n_swap`, `n_buy`, `n_sell` | jumlah swap / beli / jual |
| `buy_usd`, `sell_usd` | Σ \|ETH\|/1e18 × harga ETH, beli / jual |
| `lp_m1` | `2 × (L·√P atau L/√P)/1e18 × ethUsd` pada swap terakhir (nilai LP USD) |
| `ret_m1` | harga swap terakhir / harga swap pertama |
| `flow_lp` | `(buy_usd − sell_usd) / max(lp_m1, 1)` |
| `kyle_lambda` | λ dari kuadrat terkecil `Δln P = λ · volume bertanda` × 100 |
| `impact_r2` | `1 − Σres² / ΣΔlnP²` |
| `impact_cos` | `v·Δ / (‖v‖‖Δ‖)` (nilainya identik dengan `impact_r2` di data: kembar) |
| `impact_norm` | `‖Δ ln P‖` |
| `lam_buy`, `lam_sell` | λ hanya pada beli / jual |
| `lw_mom` | rata-rata Δln P berbobot LP |
| `lw_mom_log` | `ln(max(ret_m1,1e-9)) × log10(max(lp_m1,10))` |
| `n_holders` | dompet bersaldo > 0 (bukan PoolManager) |
| `top1`, `top5` | porsi saldo terbesar / 5 terbesar |
| `hhi` | Σ porsi² (identik `l2_l1` urutannya: kembar) |
| `entropy` | −Σ p ln p |
| `gini` | `1 − 2·Σ cum/n + 1/n` atas porsi menaik |
| `l2_l1` | `√Σ saldo² / Σ saldo` |
| `pool_share` | saldo PoolManager / total saldo positif |
| `n_buyers`, `n_sellers` | dompet unik menerima dari / mengirim ke PoolManager |
| `g_nodes`, `g_edges`, `g_density` | graf transfer: simpul, sisi, `E / (n(n−1)/2)` |
| `g_lambda1` | nilai eigen terbesar (iterasi pangkat 200×) |
| `g_deg_central` | `Σ(dmax − d) / ((n−1)(n−2))` |
| `g_p2p_frac` | porsi transfer antar dompet (bukan lewat kolam) |

**Akurasi tiap fitur** (AUC, label "rug sebelum sempat 1,5×" dan "sempat 2× sebelum rug", 23 jam / 2 Nov):
fitur kuat tapi menangkap pump DAN rug sekaligus: `buy_usd` 0,59/0,86 (celaka) vs 0,84/0,81 (selamat2×), `n_swap`,
`entropy`, `swap_per_dompet`. Tidak berguna: `hook` (0,50 selalu), `dev_rug_lama` (0,49–0,51).
Kembar: `impact_r2`=`impact_cos`, `hhi`=`l2_l1`, `n_buy`=`beli_m1`, `n_sell`=`jual_m1`.
Berbalik arah antar hari: `g_p2p_frac` (0,16 vs 0,68), `fee`, `pool_share`. File: `riset/hasil/auc_baru.csv`.

## K6. Skor ML + DL dan latih ulang (`belajar.rs`, `catmodel.rs`)

**Rumus.** `skor = (ML + DL) / 2`; ML saja bila penjaga DL aktif.
- ML: CatBoost 200 pohon, kedalaman 6, 34 fitur (`models/filter_cat.json`, dilatih 19 Sep, n = 2.797).
  Prediksi Rust identik CatBoost (fitur dibandingkan dalam f32).
- DL: jaringan 34-128-64-32-1 (ReLU, sigmoid), Adam lr 1e-3, L2 1e-3, fitur dinormalkan kuantil (200 titik → normal).
  Belajar dari tiap hasil koin: 1 langkah Adam + 32 koin acak dari 500 terakhir.
- Penjaga DL: AUC out-of-sample 500 hasil terakhir < 0,52 → DL dimatikan; > 0,55 → hidup lagi.
- Label: `y = ret > 1,0` dari trade bayangan (ukuran tetap, tanpa gerbang, latensi 30 blok).
- Latih ulang ML (program CatBoost C++): 20 pilihan terakhir win < 65% atau rug (ret < 0,1) > 20% (minimal 30 skor sejak
  latih terakhir), atau tiap 300 label baru. Bobot data dasar 0,3; label baru `0,5^(umur_hari/3)`; > 45 hari dibuang.
  Model baru dipasang hanya bila Rust = CatBoost (selisih ≤ 1e-5) pada 500 baris.

**Hasil uji (FATAL).**

| Data | AUC skor vs "tidak rug" | AUC vs "naik 2×" | 20% skor teratas: akhirnya rug |
|---|---|---|---|
| 23 jam, ML beku | 0,109 | 0,861 | 85% |
| 23 jam, ML+DL adaptif | 0,089 | 0,906 | 97,7% |
| 2 Nov, skor asli live | 0,099 | 0,903 | – |

Skor memilih koin yang **akan bergerak lalu rug**. Penyebab: label `ret > 1` dari mesin keluar yang sempat menjual
pump sebelum rug, jadi model belajar "keramaian", bukan "keamanan".
Ablasi (setelan pemilik, 23 jam / 2 Nov): tanpa ML $94,65 / $39,21; ML beku $33.326 / $28,50; adaptif $68.069 / $28,50.
Satu dari 7 run adaptif identik memberi hasil lain ($29.350 vs $24.788): sebab belum diketahui.

### K6a. Rumus lengkap ML (CatBoost, `catmodel.rs`)

- Model = 200 pohon *oblivious* (kedalaman 6: tiap level satu fitur + satu batas untuk semua simpul).
- Indeks daun pohon t: `idx = Σ_{k=0..5} 2^k · [ float32(x[fitur_k]) > batas_k ]`.
- `s = Σ_t daun_t[idx_t]`; `p_ML = 1 / (1 + e^-(scale·s + bias))`.
- Fitur NaN diisi −1 sebelum masuk model (`sim.rs`, `live.rs`).
- Latih ulang: `catboost fit --loss-function Logloss --iterations 200 --depth 6 --learning-rate 0.08
  --random-seed 1 --boosting-type Plain --border-count 32 --thread-count 4`, baris berbobot `w`
  (dasar 0,3; label baru `0,5^(umur_hari/3)`). Pemasangan hanya bila `max |p_Rust − p_CatBoost| ≤ 1e-5` di 500 baris.

### K6b. Rumus lengkap DL (`belajar.rs`, setara sklearn `MLPClassifier`)

1. **Normalisasi kuantil** per fitur j (200 titik dari data dasar Ags–Sep, dibuat monoton):
   `u = ½·(interp(v, q_j, ref) − interp(−v, −q_j terbalik, −ref terbalik))`;
   `u = 1` bila `v > q_max`, `u = 0` bila `v < q_min`; `z = Φ⁻¹(u)` dijepit ke `[Φ⁻¹(1e-7), Φ⁻¹(1−1e-7)]` (±5,2).
2. **Maju**: `h1 = ReLU(W1·z + b1)` (128), `h2 = ReLU(W2·h1 + b2)` (64), `h3 = ReLU(W3·h2 + b3)` (32),
   `p_DL = σ(W4·h3 + b4)`. Inisialisasi Glorot: `U(−√(6/(n_in+n_out)), +√(6/(n_in+n_out)))`.
3. **Rugi**: `L = rata(−[y·ln p + (1−y)·ln(1−p)]) + α/2·‖W‖²/n`, α = 1e-3.
4. **Adam** (lr 1e-3, β1 0,9, β2 0,999, ε 1e-8): `g = (∇W + α·W)/n`; `m = β1·m + (1−β1)·g`; `v = β2·v + (1−β2)·g²`;
   `W −= lr · m̂ / (√v̂ + ε)` dengan `m̂ = m/(1−β1^t)`, `v̂ = v/(1−β2^t)`.
5. **Belajar awal**: 30 epoch, batch 128 di data dasar. **Belajar daring**: tiap hasil koin = 1 langkah Adam pada
   koin itu + 32 koin acak dari 500 hasil terakhir.
6. **Penjaga**: tiap koin diprediksi ML dan DL SEBELUM dipelajari → simpan `(p_DL, p_ML, y)` 500 terakhir;
   `AUC_DL < 0,52` → skor = p_ML saja; `AUC_DL > 0,55` → skor = (p_ML + p_DL)/2 lagi (diperiksa mulai 100 hasil).

### K6c. Model bahaya & p_naik (`crates/engine/src/model.rs`)

- Jenis: HistGradientBoosting (pohon biner biasa, bukan oblivious). Tiap simpul: `x[fitur] ≤ ambang` → kiri;
  NaN → arah `missing_go_to_left`.
- `p = 1 / (1 + e^-(baseline + Σ nilai_daun))`, penjumlahan urut pohon (identik bit dengan versi JS lama).
- Masukan: 27 fitur potret di K12. Dipakai hanya di mesin keluar (K11 no. 7–8).

## K7. Ambang relatif (`bot.rs` `terima_skor`)

**Rumus.** `thr = kuantil(100 skor terakhir, QTHR)` (30 skor pertama: `thr0` = 0,747), lalu `max(thr, AMBANG_MIN)`.
`ml_ok = skor ≥ thr`. Bawaan QTHR 0,90; live 0,80.

**Masalah.** Ambang ikut pasar: hari jelek tetap membeli 10–20% teratas. 2 Nov skor median 0,175, p80 0,23 → bot beli
skor 0,23–0,49. Uji: QTHR 0,90 → 23 jam $33.677, 2 Nov $35,52 (rugi lebih kecil). `AMBANG_MIN` 0,3 tidak berpengaruh di
23 jam, 2 Nov $28,28.

## K8. Gerbang beli (`bot.rs` `gerbang`, urutan persis)

1. live/replay: data basi (> `FRESH_BLOCKS` 300 blok sesudah detik 63) → tolak; keputusan belum tiba > 10 dtk → tolak
2. dikecualikan (bukan meme / dev tidak diketahui) → tolak
3. `FILTER=1`: skor tidak hingga → "model belum siap"; `!ml_ok` → "ml"
4. `DEV_RULE=1`: dev punya ret < 0,3 di riwayat → "dev pernah rug"
5. `UMUR_MIN_BLOK` > 0: tunggu; LP habis saat menunggu → tolak
6. `DOMPET_RUG=1`: dompet menit-1 ada di daftar perug → tolak
7. `HONEYPOT=1`: `jual_m1 = 0` → "honeypot (belum ada penjual)"
8. stop darurat aktif → tolak; slot penuh (`MAX_POS`, `MAX_POS_AWAL`) → tolak; `LARANG_PEMANASAN` → tolak
9. `REM_HARIAN`: ekuitas < awal hari × (1 − rem) → tolak
10. eksposur: `terpakai + min(f·eq, kas) > EXP_MAX·eq` → "eksposur penuh"
11. ukuran [K9]; `lp_m1` tidak diketahui → "LP tidak diketahui" (perbaikan v2); ukuran < `MIN_POS` → "kas kurang"

Contoh penolakan (sim setelan live, 23 jam): `ml` 882, `honeypot` 17, `tanpa keputusan` 7.
2 Nov: `ml` 373, `tanpa keputusan` 26, `honeypot` 12, `LP tidak diketahui` 3, `slot penuh` 2.

### K8a. Aturan dev
Tidak pernah aktif di 23 jam maupun 2 Nov: hasil `DEV_RULE=0` identik dengan `=1` ($68.069 / $28,50).
Riwayat dev hanya dari trade bot sendiri + 677 dev di `dev_history.json`.

### K8b. Aturan honeypot (wajib ada penjual menit-1) — SALAH ARAH
| Data | Ada penjual menit-1: rug | Tanpa penjual: rug |
|---|---|---|
| 23 jam (1.270 koin hidup) | **89,5%** (617 koin) | 22,2% (653 koin) |
| 2 Nov | 9 dari 9 rug ada penjual (203 koin) | 0 dari 162 |
Roket ≥6× justru punya sedikit penjual (AUC `jual_m1` vs roket 0,40–0,43 di 6 dari 6 data).
Mematikannya saja hampir tidak berubah ($68.069 vs $66.697), karena ML sudah memilih koin ramai penjual.

### K8c. DOMPET_RUG (bawaan mati)
Dompet beli-lalu-jual di menit-1 kolam yang SUDAH rug → daftar hitam. Di 23 jam daftar berisi 489 dompet dan hanya
**10 beli** tersisa ($41,63): terlalu kasar (dompet bot/router umum ikut masuk). Tidak bisa diuji di 2 Nov (tanpa data
transfer).

## K9. Ukuran posisi (`bot.rs` `kelly`, `gerbang`)

**Rumus.**
- Riwayat `r` = ret trade selesai (diawali `KELLY_SEED` bila diset, mulai blok `KELLY_SEED_FROM`).
- `|r| < 30` → f = 2%; `30 ≤ |r| < 100` → f = 9,4%;
  selain itu `f* = argmax_{f ∈ [0,01; 0,6], langkah 0,005} rata(ln(1 + f·(r − 1)))`, `f = clamp(f*/4, KELLY_MIN, KELLY_MAKS)`.
- Rem drawdown (`REM=1`): dd > 30% → ×0,25; > 20% → ×0,5; > 10% → ×0,75.
- `pos = min(f · eq · mult, kas − (3·GAS_USD + APPROVE_USD))`, lalu `pos ≤ LP_CAP × lp_m1` (1% LP).
- Stop darurat: `1 − eq/puncak > KILL_DD` (60%) → tidak beli lagi.

Setelan live 2 Nov: seed 100 trade backtest → ¼ Kelly ±13% sejak trade pertama, `KELLY_MAKS` 0,15.

| Uji (setelan pemilik) | 23 jam | 2 Nov | DD 2 Nov |
|---|---|---|---|
| ¼ Kelly dari trade 1 (seed) | $68.069 | $28,50 | 31% |
| jadwal 2% → 9,4% (tanpa seed) | $57.886 | $37,37 | 9% |
| KELLY_MAKS 0,10 | $65.683 | $28,10 | 30% |
| tanpa batas 1% LP | $23.406 (PF 1,15) | $28,50 | – |
| tanpa rem drawdown | $68.440 | $23,29 | 42% |

Batas 1% LP dan rem drawdown **bekerja benar**. Jebakan: `KELLY_SEED=` kosong di env diabaikan (nilai berkas dipakai).

## K10. Masuk & eksekusi di simulator (`engine.rs` `enter_at`, `sell_val`, `retries`)

- Masuk di `init + 600 + LAT_BLOCKS` (live 3 blok = 0,3 dtk); `worst_order`: bot lain di blok sama dianggap lebih dulu.
- Token didapat (rumus kolam terkonsentrasi, satu rentang): `d_in = ukuran/ethUsd·1e18·(1 − fee)`;
  bila ETH = token0: `base = L·(√P − 1/(1/√P + d_in/L))`; ETH = token1: `base = L·(1/√P − 1/(√P + d_in/L))`.
- Nilai jual `v = sell_val(state, base)/ukuran`, rumus kebalikan; `SELL_IMPACT=1` menggeser √P oleh jualan sendiri.
- Beli gagal: `h32(pool|buy) < FAIL_BUY` (1,85%) → gas hilang, tanpa koin. Jual gagal: diulang ≤ 5× (0,54%).
- Biaya: `GAS_USD` 0,06/tx, `APPROVE_USD` 0,03.
- Latensi 3 vs 30 blok: $68.069 vs $64.816 (23 jam). Biaya ganda: $58.467 / $26,93.

**Anomali simulator.** Posisi terbuka di akhir data dihitung harga terakhir (contoh `0x73b53520…` "akhir data"
ret 4,0, +$1.379, pemenang terbesar). Kecocokan live: dataset 2 Nov yang dirakit membeli 3 koin rug asli yang sama
dengan live (`0x62be34f6`, `0xbbd0e7e0`, `0x701675ba`), $40 → $29,45 (live $41,38 → $29,34).

## K11. Mesin keluar v4 (`engine.rs` `exit_step`, `cek_darurat`) — urutan per swap

| # | Aturan | Rumus | Aktif |
|---|---|---|---|
| 1 | rug terdeteksi | dicabut/ditambah LP > 80% (`TUTUP_SAAT_RUG=1`) | selalu |
| 2 | LP ditarik >30% | dicabut/ditambah > 30% | selalu |
| 3 | tanpa kemajuan | belum TP1 setelah `stale_min` menit | bila diset |
| 4 | dump ≥5% LP | satu jual orang lain ≥ `DUMP_FRAC`·LP USD | selalu |
| 5 | tanda pucuk | puncak ≥ 3×, ≥ 3 jual di 60 dtk, ≥ 6 jual sejak masuk, rata jual 60 dtk ≥ 1,5× rata semua jual | selalu |
| 6 | momentum melemah | pembeli N menit terakhir ≤ ½ N menit sebelumnya **dan** beli USD ≤ jual USD (`MOM_MENIT`) | bila diset |
| 7 | TP1 | v ≥ `TP1` (1,2) → jual `TP1_FRAC` (60%); ditunda bila p_naik ≥ `TP_UP` | v4 |
| 8 | model bahaya | sesudah TP1: p_bahaya ≥ `DANGER_TH`, kecuali p_naik ≥ `UP_TH` | sesudah TP1 |
| 9 | trailing | sesudah TP1: v ≤ puncak·(1 − `TRAIL_PUNCAK`) | sesudah TP1 |
| 10 | stop bencana | sesudah TP1: v ≤ 10% puncak | sesudah TP1 |
| 11 | stop −40% | sebelum TP1: v ≤ puncak·(1 − 0,40) | sebelum TP1 |

**Data per alasan keluar** (setelan live, 23 jam, 321 beli):

| Alasan | Trade | Ret rata | USD bersih |
|---|---|---|---|
| tanda pucuk | 154 | 2,00× | +$27.868 |
| model bahaya | 70 | 1,42× | +$5.226 |
| rug terdeteksi | 74 | 0,48× | −$7.734 |
| dump ≥5% LP | 13 | 1,53× | +$1.750 |
| akhir data | 1 | 4,01× | +$1.379 |

Contoh 2 Nov (setelan live): `0x62be34f6` rug ret −0,03; `0x7c150cb5` tanda pucuk 1,84×; `0x6fb2c07e` trailing
40% 1,09×; `0xa729e5a0` model bahaya 0,89×; `0x1f0c4149` tanpa swap setelah masuk 0,90×.

**TP (permintaan pemilik: TP ikut momentum, tanpa TP1 tetap).** 23 jam / 2 Nov: TP1 1,2/0,6 $27.217 / $26,15;
TP1 2,0/0,5 $59.989 / $26,04; tanpa TP1 + MOM 1/2/3/5 menit $65.639–73.114 / $27,10–28,50.
Tanpa TP1, model bahaya / trailing / stop bencana **tidak pernah jalan** (p_naik mati → hasil identik).
`TUTUP_SAAT_RUG=0` → posisi kolam rug tidak ditutup, slot macet (53 beli).

**Inti masalah.** Dari 334 koin yang dibeli setelan pemilik di 23 jam, **98% akhirnya rug** (median 494 dtk sesudah
keputusan). Untung = jual sebelum rug. 2 Nov rug datang 2–16 dtk sesudah keputusan → semua varian rugi.

## K12. Model bahaya & p_naik (`engine.rs` `features_inner`, `models/danger_model.json`, `pnaik_model.json`)

Potret tiap ≥ 100 blok sesudah masuk, 27 fitur: `ln v`, `ln puncak`, `1 − v/puncak`, menit sejak masuk, beli/jual 60 dtk
& 300 dtk (jumlah, USD), rata jual/beli 60 dtk, `(beli−jual)/LP`, lonjakan jual, jual terbesar/LP, perubahan v 60/300 dtk,
LP/LP_masuk, LP/LP_300dtk, percepatan aktivitas, tx unik 60 dtk, jeda swap, porsi LP dicabut, hook, fee.
AUC tercatat: p_bahaya 0,823, p_naik 0,795 (205.243 potret). Uji ulang terpisah di data Nov: **belum diuji**.

## K13. Eksekusi, ledger, kill switch (`exec/*`, `kill.rs`)

- Mode `off` (kertas), `shadow` (tanda tangan + simulasi, tanpa kirim), `live` (butuh `EXEC_LIVE_CONFIRM`).
- Order: Buy / TakeProfit / Exit / Closed dari event mesin. Beli opsional; jual wajib (diulang, toleransi melebar).
- Ledger ditulis sebelum broadcast. Cek jual sebelum beli (`EXEC_CEK_JUAL`) menolak kolam ber-hook yang tak bisa dijual.
- Kill switch total untuk error internal (panic, skor NaN, latih ulang gagal, CatBoost hilang, angka akun salah);
  tidak untuk 429/timeout. Hanya dibuka pemilik (`rhc.sh lanjut`).
- Fakta 2 Nov: 531 `sell_presim_reverted` karena LP sudah ditarik, bukan karena token memblokir.
Komponen ini sudah teruji (lihat `CLAUDE-CODE-INSTRUCTIONS` bagian 7.5), tidak diubah.

## K14. Simulasi bayangan & label (`bot.rs` `opsi_bayangan`)

Tiap koin diberi trade bayangan (ukuran tetap, tanpa gerbang, latensi 30) → `labels.jsonl` → [K6].
Cek kausal: 0 pelanggaran (label dipelajari hanya setelah trade bayangannya selesai).

## K15. Radar (`radar.rs`)

Memantau semua DEX + DEX Screener, menulis `radar_labels.jsonl`. **Tidak tersambung ke beli.** Skor ML untuk koin lama
AUC 0,465; ide radar sudah gagal (win 0%).

---

## R. Komponen baru dari riset v2 (belum dipasang di bot)

### R1. Pola koin naik (arus trader per menit, 7 data: 213 roket, 2.294 pump-rug, 3.901 mati)

| Menit | Roket ≥ 6× | Pump-rug | Mati |
|---|---|---|---|
| 1 | 4 beli, 1 jual, +0,01 ETH | 22 beli, 14 jual, +0,99 ETH | 4 beli, 1 jual |
| 2 | 4 beli, +0,12 ETH | 24 beli, harga 1,38× | 0 beli |
| 5 | harga 1,62×, 100% hidup | harga 1,89×, 83% hidup | 82% hidup |
| 10 | harga 2,16×, 96% hidup | 47% hidup | 75% hidup |
Puncak roket menit 13–50; pump-rug menit 6–10, rug ±20–160 dtk sesudah puncak.

### R2. Fitur akar (umur 60 dtk, `riset/alat/src/akar.rs`) — searah di 7–8 dari 8 data

| Fitur | Rumus | Pump-rug (AUC, 8 data) | Roket |
|---|---|---|---|
| `lp_fullrange` | LP pertama tick ≤ −880000 dan ≥ 880000 | 0,60–0,85 (8/8 naik) | 6/7 turun |
| `whale_share` | beli ETH terbesar / total beli ETH | 0,12–0,38 (8/8 turun) | 6/7 naik |
| `beli_cv` | sd ukuran beli / rata | 7/8 turun | 6/7 naik |
| `interval_cv` | sd jeda antar beli / rata | 7/8 turun | 7/7 naik |
| `jual_share_eth` | jual ETH / (beli + jual ETH) | 8/8 naik | – |
| `harga_naik` | harga detik 60 / harga swap pertama | 8/8 naik | – |
| `beli_median_eth` | median ukuran beli | 7/8 naik | – |
| `beli_blok_lahir` | beli di blok lahir | 7/8 turun | 5/7 naik |
Lainnya dipakai: `lp_lebar_log`, `tick_spacing`, `fee`, `maks_swap_per_blok`, `share_swap_blok_ramai`, `beli_kembar`,
`beli_bulat`, `tunda_beli_pertama`, `n_beli`, `beli_naik_ukuran`.
Ditolak (AUC 0,50): `swap_per_tx`, `tx_multi_kolam`, `lp_n_kurang`. Ditolak (berbalik arah): `lp0`, `jual_pertama`.

### R3. Model baru (CatBoost C++, latih ags1/ags4/u_ds5d/u_ds24, uji data lain)

| Model | m1 | m2 | 23 jam | 2 Nov |
|---|---|---|---|---|
| Rug (fitur swap) | – | – | 0,947 | 0,910 |
| Pump-rug (akar) | 0,909 | 0,829 | 0,937 | 0,836 |
| Roket ≥ 6× (akar) | 0,727 | 0,628 | 0,880 | – |
| Naik 1,5× sebelum −30% (10 umur) | 0,795 | 0,710 | 0,909 | 0,854 |
Tiga rug live 2 Nov: peringkat risiko 18, 21, 63 dari 357.

### R4. Uji dagang filter akar (beli detik 60, p_roket ≥ 0,1251 & p_pumprug ≤ 0,0179, ambang dari m1)

| Data | Tanpa filter (TP2/SL30) | Dengan filter |
|---|---|---|
| m1 (validasi) | win 25%, −1%/trade | 184 trade, win 42%, +31%/trade, rug 1% |
| m2 (uji) | win 30%, 0%/trade | 59 trade, win 34%, +20%/trade, tanpa 5 terbaik +2% |
| 23 jam | win 43%, +23% | 2 trade |
| 2 Nov | win 11%, +1% | 1 trade |
TP moonshot (trailing 30%, 1/3@2× + 1/3@5× + trailing 40%): m1 +53% / +45%, m2 +12% → tidak konsisten.
Gas ±$0,21/trade belum masuk; dengan modal $40 gas memakan posisi kecil.

### R5. Uji live kertas 8 Nov 00:31–00:56 UTC (tanpa transaksi)
14 koin dinilai, 0 beli. Contoh: `0x5e54a9b5…` p_roket 0,088 / p_pumprug 0,25 → tolak; `0xebeb2408…` 0,0017 / 0,91 →
tolak (pump-rug); `0xd70f93ec…` 0,066 / 0,023 → tolak (terdekat ke ambang). Daftar: `riset/live_kertas/keputusan.csv`.

---

## Ringkasan: komponen diurut dari paling bermasalah

| # | Komponen | Status |
|---|---|---|
| 1 | Skor ML lama (K6) | FATAL: memilih koin yang akan rug (AUC 0,09–0,10) |
| 2 | Strategi keseluruhan | FATAL: 98% koin dibeli rug; untung = balapan melawan perug |
| 3 | Label ML `ret > 1` | penyebab no. 1 |
| 4 | Aturan honeypot (K8b) | salah arah |
| 5 | Simulator (K10) | terlalu optimis untuk hari sebelum live; 1 run tidak deterministik |
| 6 | Ambang relatif (K7) | membeli walau pasar jelek |
| 7 | Aturan dev (K8a) | tidak pernah aktif |
| 8 | DOMPET_RUG (K8c) | terlalu kasar |
| 9 | Model bahaya / p_naik (K12) | mati tanpa TP1 |
| 10 | 4 pasang fitur kembar, `hook`, `dev_rug_lama` (K5) | sampah |
| 11 | Jebakan setelan (`KELLY_SEED` kosong, `TUTUP_SAAT_RUG=0`) | perlu peringatan |
| — | Log gate, batas 1% LP, rem drawdown, eksekusi, kill switch | bekerja benar |
