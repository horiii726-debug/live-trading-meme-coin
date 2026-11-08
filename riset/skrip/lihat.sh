#!/usr/bin/env bash
# ringkasan uji kertas: bot jalan? berapa koin dinilai / dibeli, posisi dan akun kertas
L=/root/rhc/kertas/lihat
echo "bot pengumpul: $(systemctl is-active rhc-kertas)  mesin keputusan: $(systemctl is-active rhc-keputusan)"
echo "koin dinilai: $(($(wc -l < $L/keputusan.csv)-1))  BELI kertas: $(grep -c ',BELI$' $L/keputusan.csv)"
echo "--- 5 keputusan terakhir"; tail -5 $L/keputusan.csv
echo "--- posisi kertas"; cat $L/posisi.txt
