cd /root/rhc/uji
U=./uji.sh
$U A0_live_asli HONEYPOT=1 SELL_IMPACT=0 TP1_FRAC=0.6 MOM_MENIT=0
$U B0_pemilik
# komponen 1: filter ML
$U C1_tanpa_ML FILTER=0
$U C1_ML_beku BELAJAR=0
# komponen 2: ambang relatif
$U C2_QTHR90 QTHR=0.90
$U C2_lantai03 AMBANG_MIN=0.3
# komponen 3: aturan dev
$U C3_dev_mati DEV_RULE=0
# komponen 4: honeypot
$U C4_honeypot_nyala HONEYPOT=1
# komponen 5: dompet perug
$U C5_dompet_rug DOMPET_RUG=1
# komponen 6: batas 1% LP
$U C6_tanpa_LPcap LP_CAP=0
# komponen 7: rem drawdown
$U C7_tanpa_rem REM=0
# komponen 8: keluar
$U C8_TP1_asli TP1_FRAC=0.6 MOM_MENIT=0
$U C8_TP1_2x TP1=2.0 TP1_FRAC=0.5 MOM_MENIT=0
$U C8_mom1 MOM_MENIT=1
$U C8_mom3 MOM_MENIT=3
$U C8_mom5 MOM_MENIT=5
$U C8_tanpa_mom MOM_MENIT=0
$U C8_tanpa_tutup_rug TUTUP_SAAT_RUG=0
# komponen 9: latensi
$U C9_lat30 LAT_BLOCKS=30
# komponen 10: ukuran posisi
$U C10_jadwal_2persen KELLY_SEED=
$U C10_kellymaks10 KELLY_MAKS=0.10
# komponen 11: biaya ganda
$U C11_biaya_ganda GAS_USD=0.12 APPROVE_USD=0.06 FAIL_BUY=0.037 FAIL_SELL=0.0108
echo SELESAI
