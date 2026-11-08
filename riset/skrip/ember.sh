#!/usr/bin/env bash
# ember.sh <csv> <kolom> <batas1,batas2,...>  -> per ember: n, rug%, naik2x 1j%, naik5x 9j%, akhir>1 %
# hanya kolam pasangan ETH yang masih hidup di detik 60
awk -F, -v K="$2" -v B="$3" -v FX="$4" -v FV="$5" '
NR==1{for(i=1;i<=NF;i++) c[$i]=i; nb=split(B,b,","); next}
$c["eth_pair"]==1 && $c["mati_d"]==0 && $c["fee"]<=10000 && $c["hook"]==0 && (FX=="" || $c[FX]==FV) {
  v=$c[K]; if(v=="NaN"||v=="") {e="NaN"} else { e=nb+1; for(j=1;j<=nb;j++) if(v+0 < b[j]+0){e=j; break} }
  n[e]++; r[e]+=$c["rug"]; m1=$c["max1j"]; m9=$c["max9j"]; ak=$c["akhir"]; if(m1!="NaN" && m1+0>=2) d[e]++; if(m9!="NaN" && m9+0>=5) l[e]++; if(ak!="NaN" && ak+0>1) a[e]++; if(m9=="NaN") nn[e]++
}
END{ for(j=1;j<=nb+1;j++){ lo=(j==1?"-inf":b[j-1]); hi=(j>nb?"inf":b[j]); if(n[j]) printf "  %s [%s,%s): n=%d rug=%.0f%% 2x=%.0f%% 5x=%.0f%% untung_akhir=%.0f%%\n", K, lo, hi, n[j], 100*r[j]/n[j], 100*d[j]/n[j], 100*l[j]/n[j], 100*a[j]/n[j] }
     if(n["NaN"]) printf "  %s NaN: n=%d rug=%.0f%%\n", K, n["NaN"], 100*r["NaN"]/n["NaN"] }' "$1"
