#!/usr/bin/env python3
"""Answer mulai.sh's questions through a real pseudo-terminal, the way the owner does in an SSH session.

    ketik.py "<shell command>" [--no-questions]

The wallet is a fresh random key: it holds no ETH, so the live bot can never buy anything with it. The key is typed
into the hidden prompt and never printed. --no-questions (the re-run / update case) fails if anything is asked.
"""
import os
import pty
import secrets
import select
import sys
import time

cmd = sys.argv[1]
no_questions = "--no-questions" in sys.argv
key = secrets.token_hex(32)
# the deposit is typed the Indonesian way ("1.000" = one thousand) and must end up as START_USD=1000
answers = [(b"RPC: ", b""), (b"Private key: ", key.encode()), (b"[100]: ", b"1.000"), (b"Benar? (y/n) [y]: ", b"y")]

pid, fd = pty.fork()
if pid == 0:
    os.execvp("bash", ["bash", "-c", cmd])
# a person presses Enter a few times while the build runs; those must not answer the questions later
time.sleep(2)
os.write(fd, b"\n\n\n")

buf, asked, deadline = b"", 0, time.time() + 55 * 60
while True:
    if time.time() > deadline:
        print("\nKETIK: TIMEOUT", flush=True)
        os.kill(pid, 9)
        sys.exit(124)
    ready, _, _ = select.select([fd], [], [], 5)
    if not ready:
        continue
    try:
        chunk = os.read(fd, 4096)
    except OSError:  # EIO: the child closed the terminal
        break
    if not chunk:
        break
    sys.stdout.buffer.write(chunk)
    sys.stdout.flush()
    buf = (buf + chunk)[-4096:]
    prompt, reply = answers[asked % len(answers)]
    if prompt in buf:
        if no_questions:
            print("\nKETIK: GAGAL — pertanyaan muncul lagi saat dijalankan ulang", flush=True)
            os.kill(pid, 9)
            sys.exit(3)
        os.write(fd, reply + b"\n")
        asked += 1
        buf = b""

_, status = os.waitpid(pid, 0)
code = os.waitstatus_to_exitcode(status)
if not no_questions and asked < len(answers):
    print(f"\nKETIK: GAGAL — hanya {asked} dari {len(answers)} pertanyaan yang muncul", flush=True)
    sys.exit(2)
print(f"\nKETIK: selesai, kode keluar {code}, pertanyaan dijawab {asked}", flush=True)
sys.exit(code)
