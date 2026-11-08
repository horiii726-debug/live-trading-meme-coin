#!/usr/bin/env bash
# Mesin keputusan KERTAS (tanpa transaksi): tiap 20 dtk nilai kolam baru yang sudah berumur 60 dtk dengan model akar
# (roket + pump-rug). BELI kertas bila p_roket >= TR dan p_pumprug <= TP (ambang dari validasi m1). Posisi: TP 2x / stop -30% / rug.
K=/root/rhc/kertas; L=$K/lihat; M=/root/rhc/uji/akar
A=/root/rhc/alat/target/release/alat; C=/root/rhc/bin/catboost
TR=0.1251185049; TP=0.01794435706
FT="lp_fullrange lp_lebar_log tick_spacing fee maks_swap_per_blok share_swap_blok_ramai beli_kembar beli_bulat beli_blok_lahir tunda_beli_pertama whale_share jual_share_eth n_beli beli_median_eth beli_cv interval_cv beli_naik_ukuran harga_naik"
mkdir -p $L; cd $L
[ -f keputusan.csv ] || echo "waktu_utc,pool,init_blok,p_roket,p_pumprug,keputusan" > keputusan.csv
touch beli.txt
while true; do
  for f in pools liq swaps; do [ -f $K/rust_live/store/$f.csv ] || continue; done
  if [ -f $K/rust_live/store/liq.csv ] && [ -f $K/rust_live/store/swaps.csv ]; then
    cp $K/rust_live/store/pools.csv pools.csv; cp $K/rust_live/store/liq.csv liq.csv; cp $K/rust_live/store/swaps.csv swaps_sorted.csv
    tip=$(tail -1 swaps_sorted.csv | cut -d, -f2)
    $A akar $L live 60 f.csv 2>/dev/null
    awk -F, -v OFS='\t' -v T=$tip -v FT="$FT" 'NR==FNR{if(FNR>1) d[$2]=1; next} FNR==1{for(i=1;i<=NF;i++)c[$i]=i; n=split(FT,F," "); next} ($3+603<=T) && !($2 in d) {s="0"; for(j=1;j<=n;j++){v=$c[F[j]]; if(v=="NaN") v="nan"; s=s"\t"v}; print s"\t"$2"\t"$3}' keputusan.csv f.csv > baru.tsv
    if [ -s baru.tsv ]; then
      cut -f1-19 baru.tsv > x.tsv; printf "0\tLabel\n" > cd
      $C calc --input-path x.tsv --column-description cd --model-file $M/m_kelas_roket_60.bin --prediction-type Probability --output-path pr1.tsv >/dev/null 2>&1
      $C calc --input-path x.tsv --column-description cd --model-file $M/m_kelas_pumprug_60.bin --prediction-type Probability --output-path pr2.tsv >/dev/null 2>&1
      now=$(date -u +%Y-%m-%dT%H:%M:%S)
      paste <(cut -f20,21 baru.tsv) <(tail -n +2 pr1.tsv | awk -F'\t' '{print $NF}') <(tail -n +2 pr2.tsv | awk -F'\t' '{print $NF}') | awk -F'\t' -v OFS=, -v W=$now -v A=$TR -v B=$TP '{k=($3>=A && $4<=B)?"BELI":"tolak"; print W,$1,$2,$3,$4,k}' >> keputusan.csv
      awk -F, 'NR>1 && $6=="BELI"{print $2}' keputusan.csv > beli.txt
    fi
    $A status $L beli.txt $K/seed_m1.txt > posisi.txt 2>/dev/null
  fi
  sleep 20
done
