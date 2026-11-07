#!/usr/bin/env python3
"""Synthesize assets/sounds/attention.caf (SPEC §7.11): a soft, light ~0.3 s chime with a
gentle attack and no sharp transients. Usage: make_chime.py <out.wav> (then afconvert)."""
import math, struct, sys, wave

RATE = 44100
DUR = 0.32
out = sys.argv[1]
frames = []
for i in range(int(RATE * DUR)):
    t = i / RATE
    attack = min(1.0, t / 0.025)            # 25 ms fade-in: no click
    decay = math.exp(-t * 9.0)              # soft exponential tail
    env = attack * decay
    # Two gentle partials (E6 + B6) with a touch of the octave.
    s = (0.55 * math.sin(2 * math.pi * 1318.5 * t)
         + 0.30 * math.sin(2 * math.pi * 1975.5 * t)
         + 0.08 * math.sin(2 * math.pi * 2637.0 * t))
    frames.append(int(max(-1, min(1, s * env * 0.35)) * 32767))
# Final 15 ms fade-out.
n = int(RATE * 0.015)
for k in range(n):
    frames[-n + k] = int(frames[-n + k] * (1 - k / n))
with wave.open(out, "wb") as w:
    w.setnchannels(1)
    w.setsampwidth(2)
    w.setframerate(RATE)
    w.writeframes(b"".join(struct.pack("<h", f) for f in frames))
