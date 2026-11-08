# Hasil v2: bedah sistem + riset fitur (8 Nov 2026)

v1 = keadaan repo sebelum riset ini (tag `v1` = commit `aed3cf3`). v2 = branch `v2` (kode bot + folder `riset/`).
Semua angka dari data rantai nyata, dihitung di VPS. **Logika trading bot belum diubah**; model baru belum dipasang di bot.

## Data
8 dataset nyata: ags1, ags4 (Agustus), u_ds5d, u_ds24, m1, m2 (September), 23 jam (1–2 Nov), hari live 2 Nov.
±97 ribu koin, ±7.300 layak dibeli (pasangan ETH, fee ≤ 1%, tanpa hook, hidup di detik 60).
Model dilatih di ags1/ags4/u_ds5d/u_ds24, diuji di m1, m2, 23 jam, 2 Nov.

## Masalah sistem lama (v1)
| Komponen | Temuan |
|---|---|
| Skor ML lama | memilih koin yang AKAN rug: AUC vs "tidak rug" 0,09 (23 jam) / 0,10 (2 Nov) |
| Koin yang dibeli | 98% akhirnya rug (median ±8 menit). Untung backtest = sempat jual sebelum rug |
| Aturan honeypot | salah arah: koin dengan penjual menit-1 rug 89,5% vs 22% tanpa penjual |
| Aturan dev | tidak pernah aktif di 2 data (hasil identik nyala/mati) |
| p_naik / model bahaya | tidak jalan tanpa TP1 |
| DOMPET_RUG | memblokir hampir semua koin (10 beli dari 334) |
| `TUTUP_SAAT_RUG=0` | slot macet (53 beli) |
| `KELLY_SEED` kosong di env | diam-diam diabaikan, pakai nilai berkas |
| Simulator | 23 jam: $40 → $68.069; 2 Nov: $40 → $28,50. 23 varian, tidak ada yang untung di 2 Nov |
| Batas 1% LP, rem drawdown | bekerja benar |

## Pola koin naik (konsisten di 7–8 dari 8 data)
| | Pump-rug | Roket asli (≥6×) |
|---|---|---|
| Menit 1 | 22 beli, 14 jual, +0,99 ETH | 4 beli, 1 jual (sama dengan koin mati) |
| Menit 2 | 24 beli | masih 4 beli, +0,12 ETH (koin mati: 0 beli) |
| Puncak | menit 6–10, 53% sudah rug di menit 10 | menit 13–50, 96% masih hidup di menit 10 |
| LP | rentang penuh | terkonsentrasi / satu sisi |
| Ukuran & jeda beli | seragam, teratur (skrip) | acak (organik) |

## Fitur baru yang lolos (fitur akar, umur 60 detik)
`lp_fullrange`, `lp_lebar_log`, `tick_spacing`, `fee`, `maks_swap_per_blok`, `share_swap_blok_ramai`, `beli_kembar`,
`beli_bulat`, `beli_blok_lahir`, `tunda_beli_pertama`, `whale_share`, `jual_share_eth`, `n_beli`, `beli_median_eth`,
`beli_cv`, `interval_cv`, `beli_naik_ukuran`, `harga_naik`. Rumus: `riset/alat/src/akar.rs`.

Ditolak: `swap_per_tx`, `tx_multi_kolam`, `lp_n_kurang`, `hook` (AUC 0,50); `lp0`, `jual_pertama` (berbalik arah antar
periode); sinyal "lanjut dibeli di menit 2" (beli di 120 dtk tetap rugi); TP moonshot bertingkat/trailing (menang m1, kalah m2).

## Akurasi model baru (CatBoost C++, data tidak dilihat saat latih)
| Model | m1 | m2 | 23 jam | 2 Nov |
|---|---|---|---|---|
| Rug (fitur swap) | – | – | 0,947 | 0,910 |
| Pump-rug (fitur akar) | 0,909 | 0,829 | 0,937 | 0,836 |
| Roket ≥6× (fitur akar) | 0,727 | 0,628 | 0,880 | – (tidak ada roket) |
| Naik 1,5× sebelum −30% (potret 10 umur) | 0,795 | 0,710 | 0,909 | 0,854 |

Tiga rug live 2 Nov ada di peringkat 18, 21, 63 dari 357 skor risiko rug.

## Hasil dagang (beli detik 60, TP 2× / stop −30%, fee kolam dihitung, gas belum)
| Data | Tanpa filter | Filter fitur akar (p_roket ≥ 0,125 & p_pumprug ≤ 0,018, ambang dari m1) |
|---|---|---|
| m1 (validasi) | win 25%, −1%/trade | 184 trade, win 42%, **+31%/trade**, rug 1% |
| m2 (uji) | win 30%, 0%/trade | 59 trade, win 34%, **+20%/trade** (tanpa 5 terbaik +2%), rug 0% |
| 23 jam | win 43%, +23%/trade | 2 trade (filter menahan diri) |
| 2 Nov | win 11%, +1%/trade | 1 trade |

## Uji live kertas (8 Nov 00:31–00:56 UTC, tanpa transaksi)
14 koin dinilai, 0 beli kertas. Detail: `riset/live_kertas/`.

## Belum / kelemahan
- Model roket masih tumpul (presisi 3–18%); roket ≥5× yang tertangkap sedikit.
- Gas ±$0,21/trade memakan akun $40.
- Belum diuji di data baru 3–8 Nov; uji kertas terlalu singkat.
- Belum: SHAP/ablasi, hubungan dompet & sumber dana (data dompet hanya menit 1), laporan per temuan.

## Isi folder
`alat/` alat ukur Rust (kebenaran, akar, pola, arus, strategi, potret, dagang, status) · `skrip/` skrip uji di VPS ·
`model/` model CatBoost · `hasil/` tabel hasil mentah · `live_kertas/` keputusan uji kertas ·
`MEMECOIN_MOONSHOT_...md` spesifikasi riset dari pemilik.
