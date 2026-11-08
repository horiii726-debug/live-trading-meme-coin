#!/usr/bin/env bash
# latih.sh <label> <latih: ds,ds,...> <uji: ds,ds,...> : CatBoost (C++) pada fitur swap, uji di data yang tidak dipakai melatih
set -e
Y=$1; TR=$2; TE=$3
cd /root/rhc/uji/fitur; mkdir -p model; W=$PWD/model/$Y
FEATS="jual_m1 beli_m1 beli_awal eth_beli eth_jual jual_pertama maks_jual beli_0_30 beli_30_60 ret_m1x lp0 dev_lama dev_rug_lama beli_awal_share"
tsv() { # $1 = daftar ds -> tsv (label, fitur), hanya layak=1 dan label terdefinisi
  for d in $(echo $1 | tr , ' '); do
    awk -F, -v OFS='\t' -v Y=$Y -v FS2="$FEATS" 'NR==1{for(i=1;i<=NF;i++)c[$i]=i; n=split(FS2,F," "); next}
      $c["layak"]==1 && $c[Y]!="NaN" { s=$c[Y]; for(j=1;j<=n;j++){v=$c[F[j]]; if(v=="NaN"||v=="") v="nan"; if(F[j]=="lp0" && v!="nan") v=log(v+1); s=s"\t"v}; print s, $c["ds"]"|"$c["pool"] }' gabt_$d.csv
  done
}
tsv $TR | cut -f1-15 > $W.latih.tsv
tsv $TE > $W.uji_id.tsv; cut -f1-15 $W.uji_id.tsv > $W.uji.tsv
printf "0\tLabel\n" > $W.cd
/root/rhc/bin/catboost fit --learn-set $W.latih.tsv --column-description $W.cd --loss-function Logloss --iterations 300 --depth 4 --learning-rate 0.05 --random-seed 1 --thread-count 4 --model-file $W.bin --train-dir $W.train --logging-level Silent
/root/rhc/bin/catboost calc --input-path $W.uji.tsv --column-description $W.cd --model-file $W.bin --prediction-type Probability --output-path $W.pred.tsv >/dev/null
# skor per koin: ds|pool, label, p
paste <(cut -f16 $W.uji_id.tsv) <(cut -f1 $W.uji_id.tsv) <(tail -n +2 $W.pred.tsv | awk -F'\t' '{print $NF}') > $W.skor.tsv
echo "$Y latih=$(wc -l < $W.latih.tsv) (pos $(awk '$1==1' $W.latih.tsv | wc -l)) uji=$(wc -l < $W.uji.tsv)"
for d in $(echo $TE | tr , ' '); do
  awk -F'\t' -v D=$d 'BEGIN{print "skor,y"} index($1,D"|")==1{print $3","$2}' $W.skor.tsv > $W.$d.csv
  echo -n "  uji $d: "; /root/rhc/alat/target/release/alat auc $W.$d.csv skor y
done
/root/rhc/bin/catboost fstr --input-path $W.latih.tsv --column-description $W.cd --model-file $W.bin --output-path $W.fstr.tsv >/dev/null 2>&1 || true
echo "  fitur terpenting: $(sort -k2 -gr $W.fstr.tsv 2>/dev/null | head -5 | awk -v FS2="$FEATS" 'BEGIN{n=split(FS2,F," ")} {printf "%s(%.0f) ", F[$1+1], $2}')"
