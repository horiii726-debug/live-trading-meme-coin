# Konteks untuk chat baru — RHC memecoin bot

Salin seluruh isi berkas ini ke chat Claude yang baru.

---

## Siapa saya dan aturan kerja

- Saya non-teknis, menulis bahasa Indonesia, minta **jawaban singkat**.
- Repo GitHub: `horiii726-debug/live-trading-meme-coin` — sekarang **PRIVAT**.
- Bot Rust (`rhc-trader`) untuk memecoin Uniswap v4 di **Robinhood Chain, chainId 4663**, blok ~0,1 detik.
- **JANGAN UBAH SISTEM TRADING tanpa izin saya.**
- Private key **tidak pernah** di GitHub dan **tidak pernah** di chat. RPC Goldsky memang disimpan di repo atas permintaan saya.
- ML dilatih pakai **CatBoost C++**, DL dan eksekusi pakai **Rust**. **Tidak ada Python di bot** (Python hanya untuk analisa/laporan).
- Tanpa Telegram. ¼ Kelly + compounding. Hasil harus jujur — tanpa overfitting, tanpa look-ahead.
- Jangan pakai RPC cadangan untuk trading: kalau Goldsky mati, berhenti trading.

## Yang terjadi 2 November 2026 (hari live pertama, uang sungguhan)

```
MODAL  $41,38  ->  $29,34     RUGI -$11,87 (-28,7%)
3 pembelian uang real, SEMUANYA RUG dalam 1-2 detik setelah beli
```

Ketiga koin lolos semua gerbang (ML, dev bersih, ada penjual, LP $20rb).
Rug terjadi **setelah** bot membeli. Sesudah 15:58 UTC bot dipindah ke mode
shadow (nol transaksi) dan dana tidak tersentuh lagi.

Shadow sampai 20:57 UTC: 401 koin dinilai, 25 trade, **menang 10 (40%)**,
ekuitas kertas $40 → $27,84. Jam 19:00–21:00 jauh lebih baik dari pagi
(`tanda pucuk` memanen +50% sampai +75% berulang kali).

## Temuan utama (semua sudah diuji dengan data, bukan dugaan)

**1. Sistemnya tidak rusak.** 49 uji unit lulus, `SELFTEST OK`, eksekusi
terbukti jalan. Simulasi `rhc-trader sim` mereproduksi hasil live **persis
1:1** ($28,95, 4 beli, 0 menang). Live membeli di **detik ke-60,3**, sama
persis dengan 1.445 trade backtest sebulan — tanpa penyimpangan.

**2. Masalahnya risk-reward.** Dari 2.215 trade backtest sebulan:
```
rata-rata MENANG : +63,3%        rata-rata KALAH : -72,8%
RR               : 0,87 : 1      win rate impas  : 53,5%
backtest         : 82,1%         live hari ini   : 40%   <- DI BAWAH IMPAS
```
Sistem ini menang karena **sering benar**, bukan karena RR. Penyebab
struktural: TP1 menjual 60% posisi di 1,2× (untung terkunci maks +12% dari
posisi penuh) sementara rug menghapus 100%. Atas dipotong, bawah tidak dibatasi.

**3. Pasarnya berubah, bukan kodenya.** 158 kolam dicek langsung ke rantai:
**71% RUG**, 5% naik. Backtest sebulan: rug 11,7%, menang 82,1%.

**4. Keramaian menit-1 adalah umpan.** Kolam dengan penjual menit-1 rug 67–77%;
tanpa penjual 0–43%. Aturan `HONEYPOT=1` mewajibkan ada penjual — di medan ini
justru menyaring **masuk** yang rug.

**5. Backtest sebulan BERSIH.** Diaudit: tidak ada look-ahead (fitur dihitung
mundur saja), LP cap bekerja (posisi median 0,28% ekuitas), ekuitas $183.830
cocok persis dengan rekonstruksi, 10 trade terbaik hanya 8,4% dari laba.
`model bahaya` menang 100% karena hanya boleh memicu **setelah** TP1 1,2× —
bukan kecurangan.

## Gagasan yang sudah diuji dan GAGAL — jangan diulang

| Gagasan | Hasil |
|---|---|
| Tunggu 10 menit sebelum beli | 21 trade, win 19%, −12% |
| Pindai semua umur pakai kriteria radar | 11 trade, win **0%**, −82% |
| Radar + ambang ML | 8 trade, win 0%, −70% |
| Naikkan lantai skor ke 0,50 | memblokir rugi **dan** semua pemenang |
| Longgarkan batas umur koin (0,5–100 menit) | **tidak berpengaruh sama sekali** |

**Skor ML tidak berlaku untuk koin lama: AUC 0,465** (di bawah acak). Model
dilatih dengan fitur menit-1, jadi tidak membawa informasi untuk koin berumur jam.
Untuk koin lama butuh **model kedua** yang dilatih dengan fitur radar.

## Yang menjanjikan tapi BELUM TERBUKTI

Sapuan take-profit di data 23,6 jam (3.168 kolam):
```
TP1 1,2 / FRAC 0,6 (sekarang) : 297 trade | win 57,2% | PF 1,32 | $25.067
TP1 2,0 / FRAC 0,5            : 275 trade | win 57,1% | PF 1,21 | $36.730  (+47%)
tanpa TP1                     : 242 trade | win 49,2% | PF 1,22 | $19.214
```
TP1 di 2,0× menjual 50% memberi +47% dengan win rate sama. **Satu hari data —
belum boleh dipakai**, harus diuji di beberapa hari.

## Bug yang DITEMUKAN tapi BELUM DIPERBAIKI

**`lp_m1` = 0 pada simulasi 1 hari** → syarat `if lp_cap > 0 && lp_m1 > 0`
gagal → batas likuiditas **dilewati diam-diam**, posisi tumbuh sampai $4.008.
Terbukti: LP_CAP 1,0 sampai 0,0001 memberi hasil **identik**. Semua angka
ekuitas dari simulasi 1 hari **belum sah** sampai ini diperbaiki. Backtest
sebulan tidak kena bug ini.

## Perubahan kode (commit `ff30ca0`, semuanya bawaan MATI)

- `cfg.rs`/`bot.rs`: `AMBANG_MIN` — lantai ambang skor (bawaan 0)
- `cfg.rs`/`bot.rs`: `UMUR_MIN_BLOK` — tunda beli sampai kolam berumur N blok (bawaan 0)
- `engine.rs`: `TP1` dan `TP1_FRAC` bisa diatur untuk riset (bawaan 1.2 / 0.6 = seperti semula)
- `unduh.rs` (alat riset): kenali pesan batas rentang Goldsky, lebar segmen 3000→800,
  batas bawah jendela fitur, rentang pencarian dev 36600→750

**Logika trading tidak berubah.**

## Keadaan sekarang

- **VPS sudah dihapus total.** Private key, `.env`, `/opt/rhc`, service — semua
  terhapus, pemindaian seluruh mesin: nol sisa kunci.
- **Dana ~0,012913 ETH (~$29) aman di MetaMask**, alamat
  `0x449f4c025c568d18d194d7f59890e56b9d543801`.
- **GitHub** `ff30ca0`: folder `hasil-live/2026-11-02/` berisi bukti mentah —
  `decisions.jsonl`, `trades.jsonl`, `labels.jsonl`, `exec/journal.jsonl`,
  `radar.json`, `live.log`, `store/`, `backtest/`, plus README lengkap.
- **Arsip lokal** di `C:\Users\horii\rhc-kerja\arsip\`:
  - `arsip_rhc_2026-11-02.tar.gz` (17,5 MB) — seluruh keadaan bot
  - `data_23jam.tar.gz` (47,1 MB) — data rantai 23,6 jam siap dipakai ulang

## Catatan teknis yang berguna

- Goldsky batas `eth_getLogs`: **800 blok**. Pesan errornya "exceeded max
  allowed range" (bukan "exceeds limit").
- RPC publik `rpc.mainnet.chain.robinhood.com` **403 Forbidden** dari VPS
  datacenter — tidak bisa dipakai.
- Radar **sudah** menghitung 34 fitur per kolam untuk semua umur:
  `tumbuh_tx`, `tumbuh_pelaku`, `beli_5m`, `pelaku_5m`, `likuid_usd`, `umur_mnt`,
  dll. Tapi sengaja **tidak tersambung ke beli** ("RADAR — bukan sinyal beli").
- Radar menulis `radar_labels.jsonl` (fitur + `maks_1j`, `maks_6j`) tapi butuh
  **6 jam** sebelum label pertama. Snapshot hanya di memori — mati = hilang.
- Latih ulang ML butuh **20 trade selesai** atau **300 label**. Di live sehari
  cuma terkumpul 3 label, jadi butuh berhari-hari.
- Latensi jual bot: **0,28 detik** dari sinyal ke transaksi terkirim — cepat,
  bukan penyebab masalah.

## Syarat sebelum live lagi (disepakati)

1. Perbaiki bug `lp_m1`, jalankan ulang simulasi
2. Uji TP1 2,0/0,5 di beberapa hari berbeda
3. Tinjau ulang aturan honeypot (mewajibkan penjual menit-1 tampaknya memilih umpan)
4. Shadow **≥50 trade, win rate ≥35%, ekuitas naik — dua hari berturut-turut**
5. Baru live dengan modal kecil ($5), bukan seluruh dana
