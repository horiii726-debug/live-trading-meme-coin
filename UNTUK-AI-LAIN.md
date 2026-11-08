# UNTUK AI LAIN: serah-terima, data, dan SEMUA kekurangan yang diketahui

Tujuan file ini: AI lain bisa memeriksa ulang pekerjaan kami tanpa mulai dari nol, dan langsung tahu di mana kami
mungkin salah. Tulis ulang/koreksi file ini kalau menemukan hal baru. Ditulis 9 Nov 2026 (branch `v2`).

## 1. Konteks singkat

- Bot `rhc-trader` (Rust) untuk memecoin Uniswap v4 di **Robinhood Chain 4663** (blok ±0,1 dtk). Pemilik non-teknis,
  berbahasa Indonesia, minta jawaban singkat.
- Live uang nyata 2 Nov 2026: $41,38 → $29,34, 3 beli semuanya rug 1–2 dtk sesudah beli. Bot tidak jalan sejak itu.
- Aturan pemilik (wajib): eksekusi Rust, ML CatBoost C++ (jangan Python di bot); ¼ Kelly + compounding tidak boleh
  diganti; jangan ubah sistem trading tanpa izin; jangan pernah `EXEC_MODE=live`, jangan sentuh private key; jangan
  pakai subagent/AI agent; semua angka harus dari data nyata; teks singkat.
- Aturan kerja lengkap: `CLAUDE-CODE-INSTRUCTIONS.md` (lokal, tidak di GitHub) dan `RINGKASAN-UNTUK-CHAT-BARU.md`.

## 2. Peta file (branch `v2`)

| File | Isi |
|---|---|
| `REVERSE-ENGINEERING.md` | bedah 15 komponen bot (rumus, setelan, hasil uji, contoh koin) + riset v2 |
| `riset/LAPORAN-V2.md` | ringkasan hasil v2 |
| `riset/MEMECOIN_MOONSHOT_..._RESEARCH.md` | spesifikasi riset dari pemilik (aslinya Solana; dipakai untuk Robinhood) |
| `riset/alat/` | alat ukur Rust (hanya std): `kebenaran`, `auc`, `semua_auc`, `ethusd`, `akar`, `pola`, `arus`, `strategi`, `pilihan`, `sinyal_m2`, `potret`, `dagang`, `status` |
| `riset/skrip/` | skrip bash uji (jalur VPS `/root/rhc/...`, sesuaikan) |
| `riset/model/` | model CatBoost riset (`.bin`): rug, pump-rug, roket (umur 60 & 300 dtk), potret TP1,5 |
| `riset/hasil/` | tabel mentah: `semua.log` (23 varian komponen × 6 potongan), `strategi.csv` (entry×TP), `auc_*.csv` |
| `riset/live_kertas/` | 14 keputusan uji live kertas 8 Nov |
| tag `v1` | keadaan sebelum riset (`aed3cf3` = main) |

Bot di `v2` = `aed3cf3` + 1 commit perbaikan (LP cap gagal tertutup, harga ETH saat lahir di sim, TP1 dari `.env`,
`DOMPET_RUG` bawaan mati). **Model riset belum dipasang di bot.**

## 3. Data (TIDAK ada di GitHub, terlalu besar)

| Dataset | Lokasi di PC pemilik | Ukuran | Blok `init` |
|---|---|---|---|
| ags1, ags4, u_ds5d, u_ds24, m1, m2 | `C:\Users\horii\rhc-quant\rhc_vps\data\<ds>\` | 65 MB–1,1 GB | 25,9 jt – 65,7 jt |
| 23 jam (1–2 Nov) | `C:\Users\horii\rhc-kerja\arsip\data_23jam.tar.gz` → `hari1/` | 47 MB (gz) | 103,2 jt – 104,06 jt |
| hari live 2 Nov | `hasil-live/2026-11-02/` (di repo) + `arsip_rhc_2026-11-02.tar.gz` | 2,4 MB + 17 MB | 104,29 jt – 104,52 jt |
| data lain | `am_ags01..am_ags24` (hanya pools/liq/swaps, ±2,5 GB) | | belum dipakai |

Format kolom:
- `pools.csv`: `pool_id,c0,c1,fee,tick_spacing,hooks,init_block[,log_idx]` (ETH = `0x000…000`)
- `liq.csv`: `pool_id,block,log_idx,tick_lo,tick_hi,liq_delta` (BigInt; negatif = LP dicabut)
- `swaps_sorted.csv`: `pool_id,block,log_idx,tx_hash,a0,a1,sqrt_price(X96),liquidity,tick,fee`
  (`a` = delta pemanggil: negatif = pemanggil membayar)
- `swaps_m1.json`: `{pool: [[blk, li, a0, a1, sqrtP, L, fee], …]}` menit 1 saja
- `transfers.json`: `{pool: {token, tr: [[blk, li, from, to, amt], …]}}` menit 1 saja
- `dev_creator.json`: `{pool: {dev, why, …}}`
- `trades.jsonl`: daftar kolam (+ hasil backtest lama, `ethUsd`)
RPC: Goldsky di `config/rpc-bawaan.txt` (kredit berbayar pemilik; batas getLogs 800 blok; sesekali 429).
RPC publik `rpc.mainnet.chain.robinhood.com` 403 dari datacenter.

## 4. Cara mengulang

```bash
cargo build --release -p rhc-trader                         # bot; uji: cargo test --workspace --release (49 lulus)
rhc-trader sim <dir> <dari_blok> <ke_blok> out.jsonl --bot models --env <env> [--skor skor.csv]
cd riset/alat && cargo build --release                      # alat riset
alat kebenaran <dir> kb.csv          # label kebenaran per kolam (rug, puncak, sempat 1,5×/2×/3×)
alat akar <dir> <ds> 60 a.csv        # fitur akar umur 60 dtk + kelas (roket/pump-rug/mati)
alat semua_auc a.csv kelas_roket "" 8
alat arus <dir1,dir2,...>            # arus trader per menit per kelas
alat strategi <dir> <ds>             # grid entry × TP
alat pilihan <dir> pools.txt 60 <ds> # hasil aturan keluar untuk kolam terpilih
catboost fit/calc                    # CatBoost 1.2.10 (deploy/catboost.sh), parameter di skrip latihakar.sh
```
Skrip `riset/skrip/*.sh` memakai jalur VPS `/root/rhc/...` dan berkas akhir baris LF.

## 5. SEMUA kekurangan yang diketahui

### 5A. Sistem bot (diurut dari paling parah) — detail di `REVERSE-ENGINEERING.md`
1. Skor ML lama memilih koin yang akan rug: AUC vs "tidak rug" 0,09 (23 jam) / 0,10 (2 Nov).
2. 98% koin yang dibeli bot akhirnya rug; untung backtest = jual sebelum rug. 2 Nov rug 2–16 dtk sesudah keputusan.
3. Label ML `ret > 1` dari trade bayangan → model belajar keramaian, bukan keamanan.
4. Aturan honeypot (wajib ada penjual) salah arah.
5. Simulator optimis untuk hari sebelum live (+73.000% di 23 jam vs live rugi); 1 dari 7 run adaptif identik beda hasil
   ($29.350 vs $24.788), sebab belum diketahui; posisi terbuka di akhir data dinilai harga terakhir.
6. Ambang relatif (top 1−QTHR) tetap membeli saat pasar jelek.
7. Aturan dev tidak pernah aktif (perug pakai dompet baru).
8. DOMPET_RUG terlalu kasar (sisa 10 beli dari 334).
9. Model bahaya / p_naik / trailing / stop bencana mati bila TP1 dimatikan.
10. Fitur kembar: `impact_r2`=`impact_cos`, `hhi`=`l2_l1`; `hook` selalu 0,50; `dev_rug_lama` tak berguna.
11. Jebakan setelan: `KELLY_SEED=` kosong di env diabaikan; `TUTUP_SAAT_RUG=0` membuat slot macet;
    `on()` menganggap selain `1`/`true` = mati; angka setelan yang salah ketik jatuh ke bawaan diam-diam.
12. Gas ±$0,21/trade membuat akun $40 tidak layak untuk banyak posisi kecil.

### 5B. Data
1. **`dev_creator.json` sama persis di keenam dataset lama** (md5 `78559474…`, 2.847 entri; 156 "tanpa mint").
   Dev hanya ada untuk 2 (ags1) s/d 1.349 (m1) kolam → fitur dev (`dev_lama`, `dev_rug_lama`) hampir NaN di data lama.
2. `swaps_m1.json`/`transfers.json` data lama hanya untuk koin yang dulu dibeli backtest lama (ags1: 2 koin, m1: ±1.350
   dari 31.092) → 34 fitur ML lama hanya lengkap di 23 jam & 2 Nov; sampel lama tersaring (bias seleksi).
3. Data dompet hanya menit 1 → hubungan dompet, sumber dana, perilaku holder sesudah menit 1 TIDAK bisa dibedah.
4. 2 Nov: store live hanya ±6,4 jam → horizon 9 jam terpotong, jumlah rug = batas bawah; posisi banyak "terbuka".
   Dataset sim 2 Nov dirakit tanpa transfer (skor diambil dari keputusan live), DOMPET_RUG tak bisa diuji.
5. 23 jam tidak punya `ethUsd` per koin → diisi harga ETH/USDG saat lahir (kolam `0x24107d15…`).
6. Data backtest sebulan 18 Sep–16 Okt yang dulu dipakai (catatan lama: $183.830) **tidak ditemukan** di PC/VPS.
7. Lonjakan harga satu swap di kolam hampir kosong (hasil 10⁴²× sebelum diperbaiki) → riset memakai median 3 swap.
8. Perkiraan cadangan ETH dari `L` dan `√P` mengandaikan satu rentang aktif; untuk LP terkonsentrasi bisa meleset.
9. Waktu dari blok diasumsikan 0,1 dtk/blok (catatan lama: 852.000 blok/hari).
10. Belum ada data 3–9 Nov (perlu unduh via Goldsky = kredit pemilik).

### 5C. Metode riset v2 (yang paling mungkin membuat kami salah)
1. Ambang filter dipilih di m1 lalu hasil m1 ikut dilaporkan (validasi, bukan uji). Uji sesungguhnya hanya m2 (59 trade),
   23 jam (2 trade), 2 Nov (1 trade) → **sampel uji terlalu kecil**.
2. m2 tanpa 5 trade terbaik hanya +2%/trade → edge bisa dari segelintir koin.
3. Sebelum melatih, statistik beberapa fitur swap sempat dilihat di data Nov (kontaminasi ringan).
4. Banyak varian dicoba (± 23 setelan komponen, 5 skor, 11 entry × 14 keluar, 6 keluar bertingkat, 3 ambang ML, dll.)
   → risiko overfit pada pilihan terbaik.
5. Hasil per trade belum termasuk gas; dampak harga hanya perkiraan constant-product; isi jual = min(harga sinyal,
   harga +3 blok).
6. Label "roket" = puncak ≥ 6× dan ≥ 60 dtk sebelum rug — ambang buatan kami, belum disetujui pemilik.
7. Alat riset tidak punya unit test. Bug yang sudah ketemu & diperbaiki: perbandingan NaN di awk, berkas kosong di awk
   (`NR==FNR`), lonjakan 10⁴², pengambilan seed Kelly yang salah. **Mungkin masih ada bug lain.**
8. Belum: SHAP/ablasi per kelompok fitur, kalibrasi probabilitas, PR-AUC, uji acak label untuk model baru.
9. Uji live kertas hanya 25 menit (14 koin, 0 beli) — belum bermakna.
10. Model roket presisi 3–18% → moonshot ≥5× yang tertangkap sedikit (m2: 1 dari 59).

### 5D. Pertanyaan terbuka
1. Apakah filter akar tetap positif di data baru 3–9 Nov dan setelah gas?
2. Apakah pola "LP full-range = rug" bertahan bila perug mengganti cara pasang LP?
3. Bisakah data pemilik LP (tx.from ModifyLiquidity), saldo dev, dan sumber dana dev memisahkan roket aman dari rug?
4. Kenapa satu run sim adaptif tidak deterministik?
5. Apakah roket bisa dikenali di umur 2–5 menit (saat arus beli roket mulai berbeda) dengan entry yang tetap untung?

## 6. Daftar periksa untuk AI pemeriksa (urutan disarankan)
1. Bangun ulang `riset/alat`, jalankan `kebenaran` di 23 jam; cocokkan: 1.270 kolam hidup, rug 54,9%.
2. Periksa `akar.rs`: semua fitur hanya memakai swap/LP dengan blok ≤ init+600 (kausal)?
3. Periksa label kelas di `akar.rs` dan `jalur.rs` (smoothing, batas 200×, rug 80%).
4. Ulangi `latihakar.sh 60`: AUC pump-rug 0,91/0,83/0,94/0,84, roket 0,73/0,63/0,88.
5. Uji acak label untuk model akar (AUC harus ±0,5).
6. Hitung ulang hasil filter dengan gas $0,21 dan ¼ Kelly di akun.
7. Kumpulkan data baru (3–9 Nov), uji sekali tanpa mengubah ambang.
