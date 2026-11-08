#!/usr/bin/env bash
# Rakit dataset sim untuk 2 Nov dari store bot live + decisions.jsonl (skor asli live).
set -e
S=/root/rhc/data/arsip2nov/arsip
O=/root/rhc/data/2nov_sim
mkdir -p $O
cp $S/store/pools.csv $O/pools.csv
cp $S/store/liq.csv $O/liq.csv
(head -1 $S/store/swaps.csv; tail -n +2 $S/store/swaps.csv | sort -t, -k2,2n -k3,3n) > $O/swaps_sorted.csv
# keputusan live: pool, skor, dev
jq -r 'select(.pool != null) | [.pool, (.score|tostring), (.dev // "")] | @tsv' $S/decisions.jsonl | awk -F'\t' '!s[$1]++' > $O/kep.tsv
awk -F'\t' 'BEGIN{print "pool,score"} {print $1","$2}' $O/kep.tsv > $O/skor_live.csv
awk -F'\t' 'BEGIN{printf "{"} {if(NR>1) printf ","; printf "\"%s\":{\"dev\":\"%s\",\"why\":\"ok\"}", $1, $3} END{print "}"}' $O/kep.tsv > $O/dev_creator.json
# init per pool
awk -F, 'NR>1{print $1"\t"$7}' $O/pools.csv > $O/init.tsv
# trades.jsonl: hanya pool yang diputuskan live
awk -F'\t' 'NR==FNR{i[$1]=$2; next} ($1 in i){printf "{\"plan\":\"v4\",\"pool\":\"%s\",\"init\":%s}\n", $1, i[$1]}' $O/init.tsv $O/kep.tsv > $O/trades.jsonl
# swaps_m1.json: swap sampai init+600
awk -F'\t' 'NR==FNR{i[$1]=$2; next} FNR==1{next} {split($0,p,","); id=p[1]; if(!(id in i)) next; if(p[2]+0 > i[id]+600) next; r="[" p[2] "," p[3] ",\"" p[5] "\",\"" p[6] "\",\"" p[7] "\",\"" p[8] "\"," p[10] ".0]"; a[id]=(id in a ? a[id] "," : "") r} END{printf "{"; n=0; for(k in a){if(n++) printf ","; printf "\"%s\":[%s]", k, a[k]} print "}"}' $O/init.tsv $S/store/swaps.csv > $O/swaps_m1.json
# transfers.json: tidak ada di store -> kosong (fitur transfer tidak dipakai karena skor dari live)
awk -F'\t' 'BEGIN{printf "{"} {if(NR>1) printf ","; printf "\"%s\":{\"tr\":[[0,0,\"0x0\",\"0x0\",0]]}", $1} END{print "}"}' $O/kep.tsv > $O/transfers.json
wc -l $O/trades.jsonl $O/skor_live.csv
ls -la $O
