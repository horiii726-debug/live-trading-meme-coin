cd /root/rhc/uji
until grep -q SELESAI semua.log; do sleep 10; done
ENVF=/root/rhc/uji/live_noseed.env ./uji2.sh C10_jadwal_2persen_asli
XARGS="--naik -" ./uji2.sh C12_tanpa_pnaik
TP_UP=1.0 UP_TH=1.0 ./uji2.sh C12_pnaik_tak_dipakai
echo EKSTRA_SELESAI
