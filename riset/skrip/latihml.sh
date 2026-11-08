#!/usr/bin/env bash
# latih (Ags-awal Sep) -> validasi (m1,m2) -> uji (h23, n2). CatBoost C++.
set -e
cd /root/rhc/uji/ml
C=/root/rhc/bin/catboost; A=/root/rhc/alat/target/release/alat
cat p_ags1.tsv p_ags4.tsv p_u_ds5d.tsv p_u_ds24.tsv | cut -f1-28 > latih.tsv
printf "0\tLabel\n" > cd
$C fit --learn-set latih.tsv --column-description cd --loss-function Logloss --iterations 400 --depth 5 --learning-rate 0.05 --random-seed 1 --thread-count 4 --model-file $PWD/m.bin --train-dir $PWD/tr --logging-level Silent >/dev/null
for d in m1 m2 h23 n2; do
  cut -f1-28 p_$d.tsv > x.tsv
  $C calc --input-path x.tsv --column-description cd --model-file m.bin --prediction-type Probability --output-path pr.tsv >/dev/null
  # skor_<d>.tsv: ds|pool|umur  ret  masuk  keluar  cadangan  skor
  paste <(cut -f29-35 p_$d.tsv) <(tail -n +2 pr.tsv | awk -F'\t' '{print $NF}') | awk -F'\t' -v OFS='\t' '{print $1"|"$2"|"$3, $4, $5, $6, $7, $8}' > skor_$d.tsv
  paste <(cut -f1 p_$d.tsv) <(cut -f6 skor_$d.tsv) | awk -F'\t' 'BEGIN{print "y,skor"} {print $1","$2}' > a.csv
  echo -n "$d semua umur: "; $A auc a.csv skor y
  for u in 60 120 300 600 1200; do paste <(cut -f1 p_$d.tsv) <(cut -f31 p_$d.tsv) <(cut -f6 skor_$d.tsv) | awk -F'\t' -v U=$u 'BEGIN{print "y,skor"} $2==U {print $1","$3}' > a.csv; echo -n "   umur $u dtk: "; $A auc a.csv skor y; done
done
