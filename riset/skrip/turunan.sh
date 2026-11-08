#!/usr/bin/env bash
# turunan.sh <gab.csv> : tambah fitur turunan (ditulis ke gabt_*.csv)
in=$1; out=${in/gab_/gabt_}
awk -F, -v OFS=, '
function r(a,b){ if(a==""||b==""||a=="NaN"||b=="NaN"||a+0<0||b+0<=0) return "NaN"; return a/b }
NR==1{ for(i=1;i<=NF;i++) c[$i]=i; print $0,"beli_per_pembeli","penjual_per_pembeli","jual_beli_usd","lp_per_volume","swap_per_dompet","beli_awal_share","penjual_lebih"; next }
{ nb=$c["n_buy"]; nby=$c["n_buyers"]; ns=$c["n_sellers"]; bu=$c["buy_usd"]; su=$c["sell_usd"]; lp=$c["lp_m1"]; nh=$c["n_holders"]; nsw=$c["n_swap"]; ba=$c["beli_awal"]; bm=$c["beli_m1"]
  print $0, r(nb,nby), r(ns,nby), r(su,bu), r(lp,bu), r(nsw,nh), r(ba,bm), ((ns!="NaN" && nby!="NaN" && ns+0>=0 && nby+0>=0)? ((ns>nby)?1:0) : "NaN") }' $in > $out
echo $out
