#!/usr/bin/env bash
# Uji satu komponen per satu. Dasar = setelan pemilik: honeypot MATI, tanpa TP1 tetap (keluar ikut momentum),
# 1/4 Kelly dari trade pertama (KELLY_SEED di live.env), biaya jual nyata (SELL_IMPACT=1).
# Data: 23,6 jam (1-2 Nov) dibagi 4 periode + total, dan hari live 2 Nov (skor asli live).
B=/root/rhc/target/release/rhc-trader
M=/root/rhc/repo/models
O=/root/rhc/uji/hasil
mkdir -p $O
nama=$1; shift
P=(103213455 103425289 103637122 103848955 104060789)
dasar=(HONEYPOT=0 SELL_IMPACT=1 TP1_FRAC=0 MOM_MENIT=2)
for i in 0 1 2 3 total 2nov; do
  D=/root/rhc/data/23jam/hari1; x=()
  if [ $i = total ]; then a=0; b=1e12; elif [ $i = 2nov ]; then a=0; b=1e12; D=/root/rhc/data/2nov_sim; x=(--skor $D/skor_live.csv); else a=${P[$i]}; b=$((${P[$((i+1))]}-1)); fi
  r=$(env "${dasar[@]}" "$@" $B sim $D $a $b $O/${nama}_$i.jsonl --bot $M --env /root/rhc/uji/live.env "${x[@]}" 2>/dev/null | grep -E "^SIM")
  s=$(echo "$r" | sed -E 's/.*\| beli ([0-9]+) .*win ([0-9.]+)% \| PF ([0-9.inf]+) \| akun \$[0-9.]+ -> \$([0-9.]+) \| DD maks ([0-9.]+)%.*/beli=\1 win=\2% PF=\3 akhir=$\4 DD=\5%/')
  p=$(jq -r '.ret' $O/${nama}_$i.jsonl 2>/dev/null | awk '{n++; if($1>1){w++; sw+=$1-1} else {l++; sl+=1-$1}} END{if(n){aw=(w?sw/w:0); al=(l?sl/l:0); be=(aw+al>0?100*al/(aw+al):0); printf "menang=+%.0f%% kalah=-%.0f%% impas=%.0f%%", 100*aw, 100*al, be}}')
  echo "$nama p=$i $s $p"
done
