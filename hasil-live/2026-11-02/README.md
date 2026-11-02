# HASIL LIVE TRADING — RUGI, UANG REAL

**2 November 2026 · Robinhood Chain (4663) · uang sungguhan, bukan simulasi**

```
MODAL  $41,38  ->  $29,34
RUGI   -$11,87  (-28,7%)
3 pembelian dengan uang real, SEMUANYA RUG
```

Catatan jujur hari live pertama. Semua angka diambil dari berkas di folder ini,
bukan dari ingatan.

## Hasil uang nyata

| | |
|---|---|
| Modal awal | 0,018076 ETH = **$41,38** |
| Modal akhir | 0,012913 ETH = **$29,34** |
| **Rugi** | **−$11,87 (−28,7%)** |
| Trade nyata | 3 beli, **semua rug** |
| Lama hidup tiap posisi | 1–2 detik sebelum likuiditas dicabut |

Sesudah 15:58 UTC bot dipindah ke mode `shadow` (nol transaksi) dan sisa dana
tidak tersentuh lagi.

### Tiga pembelian yang merugikan

| Kolam | Skor | Penjual menit-1 | LP | Posisi | Hasil |
|---|---|---|---|---|---|
| `0x62be34f6` | 0,491 | 16 | $20.095 | $5,25 | rug, −100% |
| `0xbbd0e7e0` | 0,486 | 20 | $20.820 | $3,26 | rug, −100% |
| `0x701675ba` | 0,463 | 19 | $19.567 | $1,87 | rug, −100% |

Ketiganya lolos semua gerbang: ML, aturan dev (dev bersih), aturan honeypot
(ada penjual), LP wajar. Rug terjadi 1–2 detik **setelah** pembelian.

## Hasil shadow (kertas, nol transaksi) sampai 20:57 UTC

| | |
|---|---|
| Koin dinilai | 401 |
| Trade | 25 ditutup |
| Menang | 10 (**40%**) |
| Ekuitas kertas | $40 → $27,84 |
| Drawdown maks | 34% |

Jam 19:00–21:00 jauh lebih baik dari pagi: `tanda pucuk` memanen +50% sampai
+75% berulang kali. Pagi 0 menang dari 4; malam 10 dari 25.

## Temuan utama

### 1. Pasarnya, bukan sistemnya

Dari 158 kolam yang diperiksa langsung ke rantai:

| Nasib | Jumlah |
|---|---|
| **RUG** (likuiditas 0) | **111 (71%)** |
| naik | 8 (5%) |
| turun | 7 |
| mati/datar | 30 |

Bandingkan backtest sebulan: rug 11,7%, menang 82,1%.

### 2. Risk-reward sistem ini di bawah 1

Dari 2.215 trade backtest sebulan:

```
rata-rata MENANG : +63,3%
rata-rata KALAH  : −72,8%
RR               : 0,87 : 1
win rate impas   : 53,5%
win rate backtest: 82,1%
win rate live    : 40%     <- DI BAWAH IMPAS
```

Sistem ini tidak menang karena RR, tapi karena sering benar. Begitu win rate
turun di bawah 53,5%, dia rugi. Penyebab strukturalnya: TP1 menjual 60% posisi
di 1,2× (untung terkunci maks +12% dari posisi penuh) sementara rug menghapus
100%. Atas dipotong, bawah tidak dibatasi.

### 3. Keramaian menit-1 adalah umpan

Kolam dengan penjual di menit-1 rug 67–77%; kolam tanpa penjual 0–43%.
Aturan `HONEYPOT=1` mewajibkan ada penjual — di medan ini aturan itu justru
menyaring **masuk** yang rug.

### 4. Yang sudah diuji dan TIDAK membantu

| Gagasan | Hasil |
|---|---|
| Tunggu 10 menit sebelum beli | 21 trade, win 19%, −12% |
| Pindai semua umur pakai kriteria radar | 11 trade, win 0%, −82% |
| Radar + ambang ML | 8 trade, win 0%, −70% |
| Naikkan lantai skor | memblokir rugi, tapi juga memblokir semua pemenang |
| Longgarkan batas umur koin | tidak berpengaruh sama sekali |

Skor ML **tidak berlaku** untuk koin lama: AUC 0,465 (di bawah acak). Model
dilatih dengan fitur menit-1, jadi skornya tidak membawa informasi untuk koin
berumur jam.

### 5. Bukan look-ahead

Audit backtest sebulan:

```
keluar sebelum masuk        : 0
posisi terbuka bersamaan    : maks 4 (batas 6)
ekuitas dari pos/kembali    : $183.830,11 — cocok persis dengan klaim
posisi median               : 0,28% ekuitas (LP cap bekerja)
10 trade terbaik            : hanya 8,4% dari total laba
fitur model bahaya          : dihitung mundur saja, kausal
```

`model bahaya` menang 100% di backtest karena hanya boleh memicu **setelah**
TP1 1,2× — 60% posisi sudah terjual untung. Bukan kecurangan.

### 6. Bug yang ditemukan

- **`lp_m1` = 0 pada simulasi 1 hari** → `if lp_cap > 0 && lp_m1 > 0` gagal →
  batas likuiditas **dilewati**, posisi tumbuh sampai $4.008. Terbukti dengan
  menguji LP_CAP 1,0 sampai 0,0001: hasil identik. Backtest sebulan **tidak**
  kena bug ini (posisi median 0,28% ekuitas).
- `unduh.rs` tidak mengenali pesan batas rentang Goldsky → unduhan batal
- Jendela fitur tanpa batas bawah → menelan semua swap sejak kolam lahir

## Sapuan take-profit (data 23,6 jam, 3.168 kolam)

| TP1 | FRAC | Trade | Win | PF | Ekuitas |
|---|---|---|---|---|---|
| 1,2 / 0,6 (sekarang) | | 297 | 57,2% | 1,32 | $25.067 |
| 1,5 / 0,3 | | 270 | 48,1% | 1,22 | $35.795 |
| **2,0 / 0,5** | | 275 | 57,1% | 1,21 | **$36.730** |
| tanpa TP1 | | 242 | 49,2% | 1,22 | $19.214 |

TP1 di 2,0× memberi +47% dibanding setelan sekarang dengan win rate sama.
**Satu hari data — belum boleh dipakai.** Harus diuji di beberapa hari.

Catatan: angka ekuitas di tabel ini masih kena bug `lp_m1` di atas, jadi hanya
perbandingan relatifnya yang sah.

## Isi folder

| Berkas | Isi |
|---|---|
| `decisions.jsonl` | setiap koin yang dinilai: skor, 34 fitur, dev, LP |
| `trades.jsonl` | setiap trade: masuk, keluar, TP1, puncak, alasan |
| `labels.jsonl` | hasil koin yang sudah dipelajari ML |
| `events.jsonl` | event mesin: tp1, exit_signal |
| `exec/journal.jsonl` | catatan eksekutor: niat jual, kirim, revert |
| `radar.json` | keadaan radar: 1.864 kolam dikenal, 34 fitur per kolam |
| `live.log` | log lengkap |
| `store/` | swap, kolam, likuiditas mentah yang bot kumpulkan |
| `backtest/` | keluaran simulasi 23,6 jam |
| `env_tanpa_rahasia.txt` | setelan yang dipakai (tanpa kunci) |

**Tidak ada private key di folder ini.** Sudah diperiksa.

## Yang harus dievaluasi berikutnya

1. Perbaiki bug `lp_m1` lalu jalankan ulang simulasi — angka ekuitas sekarang
   belum sah
2. Uji TP1 2,0/0,5 di beberapa hari berbeda, bukan satu hari
3. Periksa ulang aturan honeypot: mewajibkan penjual menit-1 tampaknya memilih umpan
4. Model kedua untuk koin lama (fitur radar + `radar_labels.jsonl`), karena
   model menit-1 tidak berlaku di sana
5. Syarat sebelum live lagi: shadow ≥50 trade, win rate ≥35%, ekuitas naik,
   **dua hari berturut-turut**
