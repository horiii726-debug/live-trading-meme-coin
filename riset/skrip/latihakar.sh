#!/usr/bin/env bash
# fitur akar umur U: latih ags1,ags4,u_ds5d,u_ds24 -> uji m1,m2,h23,n2
set -e
U=$1; cd /root/rhc/uji/akar
C=/root/rhc/bin/catboost; A=/root/rhc/alat/target/release/alat
FT="lp_fullrange lp_lebar_log tick_spacing fee maks_swap_per_blok share_swap_blok_ramai beli_kembar beli_bulat beli_blok_lahir tunda_beli_pertama whale_share jual_share_eth n_beli beli_median_eth beli_cv interval_cv beli_naik_ukuran harga_naik"
tsv() { awk -F, -v OFS='\t' -v Y=$1 -v FT="$FT" 'NR==1{for(i=1;i<=NF;i++)c[$i]=i; n=split(FT,F," "); next} {s=$c[Y]; for(j=1;j<=n;j++){v=$c[F[j]]; if(v=="NaN") v="nan"; s=s"\t"v}; print s"\t"$c["pool"]}' $2; }
printf "0\tLabel\n" > cd
for Y in kelas_roket kelas_pumprug; do
  for d in ags1 ags4 u_ds5d u_ds24; do tsv $Y a${U}_$d.csv; done | cut -f1-19 > l.tsv
  $C fit --learn-set l.tsv --column-description cd --loss-function Logloss --iterations 400 --depth 4 --learning-rate 0.05 --random-seed 1 --thread-count 4 --model-file $PWD/m_${Y}_$U.bin --train-dir $PWD/tr --logging-level Silent >/dev/null 2>&1
  echo "== $Y umur $U (latih $(wc -l < l.tsv), positif $(awk '$1==1' l.tsv | wc -l))"
  for d in m1 m2 h23 n2; do
    tsv $Y a${U}_$d.csv > t.tsv; cut -f1-19 t.tsv > x.tsv
    $C calc --input-path x.tsv --column-description cd --model-file m_${Y}_$U.bin --prediction-type Probability --output-path pr.tsv >/dev/null 2>&1
    paste <(cut -f20 t.tsv) <(cut -f1 t.tsv) <(tail -n +2 pr.tsv | awk -F'\t' '{print $NF}') > s_${Y}_${U}_$d.tsv
    awk -F'\t' 'BEGIN{print "y,skor"} {print $2","$3}' s_${Y}_${U}_$d.tsv > a.csv
    echo -n "   uji $d: "; $A auc a.csv skor y
  done
done
