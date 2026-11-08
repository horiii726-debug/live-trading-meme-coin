#!/usr/bin/env bash
# siapkan.sh <nama_ds> <dir_data> : tabel gab_<ds>.csv = label kebenaran + 34 fitur ML (NaN -> -1, seperti dilihat ML) + fitur swap
set -e
ds=$1; dir=$2
A=/root/rhc/alat/target/release/alat
U=/root/rhc/uji/fitur; mkdir -p $U
$A kebenaran $dir $U/kb_$ds.csv
F=$(jq -r '.feats|join(",")' /root/rhc/repo/models/filter_cat.json)
if [ -n "$KBONLY" ]; then
  : > $U/f_$ds.csv
elif [ "$ds" = n2 ]; then
  # 2 Nov: fitur asli yang dihitung bot live (urutan ML)
  jq -r '[.pool] + (.feats|map(tostring)) | join(",")' /root/rhc/data/arsip2nov/arsip/decisions.jsonl | awk -F, '!s[$1]++' > $U/f_$ds.csv
else
  /root/rhc/target/release/feat_dump $U/fd_$ds.jsonl --root $(dirname $dir) $(basename $dir) >/dev/null 2>&1
  jq -r --arg F "$F" '($F|split(",")) as $k | [.pool] + [$k[] as $x | (.f[$x] // "NaN") | if (type=="string") then "-1" else tostring end] | join(",")' $U/fd_$ds.jsonl > $U/f_$ds.csv
  rm -f $U/fd_$ds.jsonl
fi
awk -F, -v OFS=, -v DS=$ds -v FH="$F" -v KB="$KBONLY" '
function ok(v,t){ if(v=="NaN") return "NaN"; return (v+0>=t)?1:0 }
function sel(t,rug,dt){ if(t=="NaN") return 0; if(rug!=1) return 1; return (dt-t>=100)?1:0 }
FILENAME==ARGV[1] { f[$1]=substr($0, index($0,",")+1); next }
FNR==1 { print "ds,pool,init,layak,rug,rug_cepat,naik2x,naik3x,naik6x,naik11x,naik21x,selamat15,selamat2,selamat3,celaka,jual_m1,beli_m1,beli_awal,eth_beli,eth_jual,jual_pertama,maks_jual,beli_0_30,beli_30_60,ret_m1x,lp0,dev_lama,dev_rug_lama," FH; next }
(KB!="" || ($1 in f)) {
  if(!($1 in f)) { s=""; for(q=0;q<34;q++) s=s (q?",":"") "NaN"; f[$1]=s }
  layak=($5==1 && $9==0 && $3<=10000 && $4==0)?1:0
  rc=($10==1 && $11<=600)?1:0
  print DS,$1,$2,layak,$10,rc,ok($14,2),ok($14,3),ok($14,6),ok($14,11),ok($14,21),sel($27,$10,$11),sel($28,$10,$11),sel($29,$10,$11),(($10==1 && sel($27,$10,$11)==0)?1:0),$7,$8,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,f[$1]
}' $U/f_$ds.csv $U/kb_$ds.csv > $U/gab_$ds.csv
echo "$ds: $(($(wc -l < $U/gab_$ds.csv)-1)) koin, layak $(awk -F, 'NR>1 && $4==1' $U/gab_$ds.csv | wc -l)"
