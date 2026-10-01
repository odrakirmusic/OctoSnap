#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""OctoSnap's sounds, synthesised from nothing (`spec/09` §4 and §4b, D134).

    synth.py              write every cue as <name>.oga beside this script
    synth.py --wav DIR    also keep the float WAV each one was encoded from, in DIR
    synth.py --check      measure the .oga files already here and fail on any off target

The sounds are written as code rather than recorded or drawn, for the reason D88 gives for
the gradients: a sound defined as its partials and envelopes cannot have come from anywhere
else, and changing one is an edit and a rerun. The .oga files are checked in beside this
script, so that neither the extension's build nor a package build needs Python or ffmpeg.
They are CC-BY-SA-4.0, like the app's icon (D121).

**Ogg Vorbis, not Opus.** `spec/09` §4 says OGG/Opus, but GNOME Shell plays sounds through
Mutter's MetaSoundPlayer, which is libcanberra, and libcanberra reads WAV and Ogg Vorbis
only. An Opus file would be silent.

**The level.** The tonal cues are set to -16 LUFS (`spec/09` §4), measured the way
ffmpeg's `ebur128` measures a programme: ITU-R BS.1770's K-weighting over 400 ms blocks,
with the cue padded with a second of silence so that a cue shorter than one block still
fills one. The gate drops the silence. Yaru's "complete", a chime, measures -16.1 LUFS.

A click cannot get there. It is nearly all peak, and at -16 LUFS it would clip; the
freedesktop theme's own camera shutter is -23.9 LUFS with its peak at -0.4 dB. So nothing
passes -1 dB true peak, and the five shutters share one lower level, -20 LUFS. One level,
because Settings lets the user switch between the shutters, and a switch should change the
sound and not the volume. The countdown's tick is 4 LU under them: it repeats every second,
and it leads up to the shutter rather than competing with it. The clicks reach their level
with a little soft saturation, which lifts the body of a click against its first peak. The
subtle shutter is let sit lower, since being quieter is what it is for.

Needs ffmpeg with libvorbis. Deterministic: the noise is seeded, so a rerun writes the same
samples, and the same files if the encoder has not changed.
"""

import math
import os
import random
import re
import shutil
import struct
import subprocess
import sys
import tempfile

RATE = 48000
HERE = os.path.dirname(os.path.abspath(__file__))

TARGET_LUFS = -16.0
# The shutters' shared level, and the tick's under it: see the module's docstring.
CLICK_LUFS = -20.0
TICK_LUFS = -24.0
CEILING_DBTP = -1.0
# How far off target a cue may measure after encoding before `--check` fails.
TOLERANCE_LU = 0.5
# `spec/09` §4: "≤ 300 ms".
LONGEST_S = 0.300
VORBIS_QUALITY = '6'


# --- Building blocks -----------------------------------------------------------------------

def frames(seconds):
    return int(round(seconds * RATE))


def mix(dst, src, at=0.0, gain=1.0):
    """Adds `src` into `dst` starting `at` seconds in, growing `dst` as needed."""
    start = frames(at)
    end = start + len(src)
    if len(dst) < end:
        dst.extend([0.0] * (end - len(dst)))
    for i, s in enumerate(src):
        dst[start + i] += s * gain
    return dst


def attack(i, n):
    """A raised-cosine rise over `n` samples: a partial that starts at full level clicks."""
    if i >= n:
        return 1.0
    return 0.5 - 0.5 * math.cos(math.pi * i / n)


def mode(freq, decay, length, amp=1.0, rise=0.0008, phase=0.0):
    """One decaying partial: what a struck object is made of (modal synthesis).

    `decay` is the time constant, in seconds, of the exponential fall.
    """
    n = frames(length)
    a = max(1, frames(rise))
    w = 2 * math.pi * freq / RATE
    return [amp * attack(i, a) * math.exp(-i / (decay * RATE)) * math.sin(w * i + phase)
            for i in range(n)]


def sweep(f_from, f_to, settle, decay, length, amp=1.0, rise=0.0015, harmonics=((1, 1.0),)):
    """A partial whose pitch glides exponentially from `f_from` to `f_to`, `settle` being
    the glide's time constant. Phase is accumulated, so the glide has no seams."""
    n = frames(length)
    a = max(1, frames(rise))
    out = []
    phase = 0.0
    for i in range(n):
        t = i / RATE
        f = f_to + (f_from - f_to) * math.exp(-t / settle)
        phase += 2 * math.pi * f / RATE
        s = sum(h_amp * math.sin(k * phase) for k, h_amp in harmonics)
        out.append(amp * attack(i, a) * math.exp(-t / decay) * s)
    return out


def noise(length, decay, seed, amp=1.0, rise=0.0002):
    """A burst of white noise with an exponential fall: the contact in a click."""
    rng = random.Random(seed)
    n = frames(length)
    a = max(1, frames(rise))
    return [amp * attack(i, a) * math.exp(-i / (decay * RATE)) * rng.uniform(-1.0, 1.0)
            for i in range(n)]


def biquad(x, kind, f0, q=0.7071):
    """RBJ's cookbook filters. `bp` is the constant 0 dB peak gain band-pass."""
    w0 = 2 * math.pi * f0 / RATE
    cw, sw = math.cos(w0), math.sin(w0)
    alpha = sw / (2 * q)
    if kind == 'lp':
        b = [(1 - cw) / 2, 1 - cw, (1 - cw) / 2]
    elif kind == 'hp':
        b = [(1 + cw) / 2, -(1 + cw), (1 + cw) / 2]
    elif kind == 'bp':
        b = [alpha, 0.0, -alpha]
    else:
        raise ValueError(kind)
    a0, a1, a2 = 1 + alpha, -2 * cw, 1 - alpha
    b0, b1, b2 = (v / a0 for v in b)
    a1, a2 = a1 / a0, a2 / a0
    y = []
    x1 = x2 = y1 = y2 = 0.0
    for s in x:
        out = b0 * s + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2
        x2, x1 = x1, s
        y2, y1 = y1, out
        y.append(out)
    return y


def pulses(notes, duty=0.25, ceiling=10000.0, blend=0.002):
    """Pulse-wave notes, `(frequency, seconds)` each, built from their harmonics up to
    `ceiling` so that they do not alias.

    A naive square at 1.3 kHz has harmonics far past 24 kHz, and they fold back down as
    inharmonic whine. The ceiling also takes the edge off, which a sound heard many times
    a day wants.

    One note hands over to the next in a `blend`-second crossfade, each at its own pitch.
    Switching the pitch outright is a step in the wave, and a step is a click. So is easing
    one note's harmonics into the next note's pitch, which puts the lower note's top
    harmonics, for that moment, far above the ceiling.
    """
    out = []
    at = 0
    overlap = frames(blend)
    for n, (freq, length) in enumerate(notes):
        coeffs = [(k, 2.0 / (k * math.pi) * math.sin(k * math.pi * duty))
                  for k in range(1, int(ceiling // freq) + 1)]
        w = 2 * math.pi * freq / RATE
        last = n == len(notes) - 1
        count = frames(length) + (0 if last else overlap)
        wave = [sum(c * math.cos(k * w * i) for k, c in coeffs) for i in range(count)]
        for i in range(overlap):
            fade = 0.5 - 0.5 * math.cos(math.pi * i / overlap)
            if n > 0:
                wave[i] *= fade
            if not last:
                wave[count - overlap + i] *= 1.0 - fade
        mix(out, wave, at / RATE)
        at += frames(length)
    return out


def dc_blocked(x):
    """A 30 Hz high-pass: the saturation's asymmetries leave a little DC, which a
    speaker turns into a thump at the start and the end."""
    return biquad(x, 'hp', 30.0)


def saturate(x, drive):
    """Soft saturation, `tanh(drive * x)` on the cue scaled to a peak of 1.

    It leaves the peak where it is and lifts everything under it, by up to
    `drive / tanh(drive)`: a click's ring and thud come up against its first contact.
    """
    peak = max(abs(s) for s in x) or 1.0
    norm = math.tanh(drive)
    return [math.tanh(drive * s / peak) / norm for s in x]


def fade_out(x, length):
    n = min(len(x), frames(length))
    for j in range(n):
        i = len(x) - n + j
        x[i] *= 0.5 + 0.5 * math.cos(math.pi * j / n)
    return x


def trimmed(x, longest=LONGEST_S, fade=0.02, floor_db=-66.0):
    """Cuts the tail where it has fallen below `floor_db` of the peak, or at `longest`,
    and fades the last `fade` seconds so the cut does not click."""
    peak = max(abs(s) for s in x) or 1.0
    floor = peak * 10 ** (floor_db / 20)
    end = len(x)
    while end > 1 and abs(x[end - 1]) < floor:
        end -= 1
    end = min(end, frames(longest))
    return fade_out(x[:end], min(fade, end / RATE / 2))


# --- The cues ------------------------------------------------------------------------------

def shutter_click(at, seed, bright, body, strength, out):
    """One curtain of a mechanical shutter: a contact, the metal ringing, the body's thud.

    `bright` scales the metal partials' frequencies, and `body` the thud's.
    """
    contact = biquad(noise(0.010, 0.0014, seed, amp=1.0), 'bp', 2800 * bright, q=1.1)
    mix(out, contact, at, 0.9 * strength)
    for f, tau, amp in ((1900, 0.014, 0.34), (3500, 0.009, 0.24), (5600, 0.006, 0.14),
                        (7900, 0.004, 0.07)):
        mix(out, mode(f * bright, tau, 0.08, amp), at, strength)
    for f, tau, amp in ((150, 0.030, 0.42), (330, 0.020, 0.22)):
        mix(out, mode(f * body, tau, 0.20, amp), at, strength)


def classic():
    """"Classic camera": two curtains 78 ms apart, the first lighter and higher, with the
    spring's whirr between them.

    The curtains are saturated and the whirr is not: saturation lifts whatever is quiet,
    and a whirr lifted by 7 dB and spread over the whole band was a hiss.
    """
    clicks = []
    shutter_click(0.0, 11, bright=1.12, body=1.2, strength=0.62, out=clicks)
    shutter_click(0.078, 13, bright=0.95, body=1.0, strength=1.0, out=clicks)
    out = saturate(clicks, 2.2)
    # The spring and the gear train: noise in a narrow band, buzzing at the gear's rate,
    # swelling and dying between the curtains.
    whirr = biquad(biquad(noise(0.07, 0.05, 12), 'bp', 1200, q=1.4), 'bp', 1200, q=1.4)
    whirr = [s * math.sin(math.pi * i / len(whirr)) * (0.6 + 0.4 * math.sin(2 * math.pi * 150 * i / RATE))
             for i, s in enumerate(whirr)]
    mix(out, whirr, 0.008, 0.12)
    return trimmed(out, fade=0.04)


def pop():
    """"Soft pop": a bubble's rising bloop, with a breath of air at the front."""
    out = sweep(380, 1050, settle=0.008, decay=0.022, length=0.16,
                harmonics=((1, 1.0), (2, 0.12)))
    puff = biquad(noise(0.004, 0.0008, 21), 'lp', 2500)
    mix(out, puff, 0.0, 0.25)
    return trimmed(out, longest=0.13, fade=0.03)


def subtle():
    """"Subtle click": the smallest sound that still says the capture happened."""
    out = biquad(noise(0.004, 0.00035, 31), 'bp', 4200, q=1.5)
    out = [s * 0.8 for s in out]
    mix(out, mode(2600, 0.004, 0.04, 0.30))
    mix(out, mode(5300, 0.0025, 0.03, 0.15))
    return trimmed(out, longest=0.045, fade=0.01)


def eight_bit():
    """"8-bit blip": two pulse-wave notes with a stepped volume envelope, the way a
    console's sound chip clocks its envelope sixty times a second.

    Each step is eased over a millisecond. A chip's steps are instant, and through a
    modern speaker at this level each one is a click of its own.
    """
    frame = 1 / 60
    out = pulses([(880.0, 2 * frame), (1318.51, 5 * frame)])
    steps = [15, 15, 13, 11, 9, 6, 3]
    ease = frames(0.001)
    per = frames(frame)
    level = steps[0] / 15
    for i in range(len(out)):
        k = min(i // per, len(steps) - 1)
        target = steps[k] / 15
        if k > 0 and i % per < ease:
            before = steps[k - 1] / 15
            level = before + (target - before) * (i % per + 1) / ease
        else:
            level = target
        out[i] *= level
    return trimmed(out, fade=0.006)


def soft():
    """"Warm modern": a muted double tap with a tone in it, lower and rounder than the
    classic."""
    out = []
    for at, seed, f_body, strength in ((0.0, 41, 220, 0.7), (0.055, 42, 165, 1.0)):
        tap = biquad(noise(0.012, 0.0015, seed), 'lp', 1600)
        mix(out, tap, at, 0.55 * strength)
        mix(out, mode(f_body, 0.035, 0.22, 0.55), at, strength)
    mix(out, mode(660, 0.045, 0.2, 0.16), 0.055)
    mix(out, mode(990, 0.035, 0.2, 0.08), 0.055)
    return trimmed(out, longest=0.2, fade=0.04)


def tick():
    """The countdown's tick: a small wood block, not bright, since it repeats."""
    out = mode(1560, 0.014, 0.08, 0.6)
    mix(out, mode(3460, 0.007, 0.05, 0.22))
    mix(out, mode(820, 0.010, 0.06, 0.2))
    mix(out, biquad(noise(0.003, 0.0006, 51), 'lp', 5000), 0.0, 0.3)
    return trimmed(out, longest=0.07, fade=0.015)


def mallet(freq, decay):
    """A marimba-like note: the fundamental and its tuned fourth-harmonic overtone."""
    out = mode(freq, decay, 0.3, 1.0, rise=0.002)
    mix(out, mode(freq * 4, decay * 0.3, 0.2, 0.12, rise=0.001))
    return out


def two_notes(first, second, gap=0.09):
    out = mallet(first, 0.045)
    mix(out, mallet(second, 0.05), gap)
    return trimmed(out, fade=0.05)


def record_start():
    """Two notes up a fifth, G5 then D6: something began."""
    return two_notes(783.99, 1174.66)


def record_stop():
    """The same two notes down: it ended."""
    return two_notes(1174.66, 783.99)


def bell(freq, decay):
    """A small glass bell: the fundamental and two inharmonic partials that die fast."""
    out = mode(freq, decay, 0.3, 1.0, rise=0.001)
    mix(out, mode(freq * 2.76, decay * 0.3, 0.2, 0.2))
    mix(out, mode(freq * 5.40, decay * 0.13, 0.1, 0.08))
    return out


def text_copied():
    """Recognised text on the clipboard: a quick rising arpeggio, C6 E6 G6, in glass,
    so that it cannot be mistaken for the recording's mallets."""
    out = []
    for at, freq, decay in ((0.0, 1046.5, 0.05), (0.055, 1318.51, 0.05), (0.11, 1567.98, 0.08)):
        mix(out, bell(freq, decay), at, 0.8)
    return trimmed(out, fade=0.06)


def tink():
    """Copy and pin: a short high tink, a small metal bar's partials."""
    out = mode(2350, 0.045, 0.2, 0.6, rise=0.0005)
    mix(out, mode(2350 * 2.76, 0.018, 0.1, 0.22))
    mix(out, mode(2350 * 5.40, 0.008, 0.05, 0.05))
    mix(out, biquad(noise(0.002, 0.0004, 61), 'hp', 4000), 0.0, 0.2)
    return trimmed(out, longest=0.16, fade=0.05)


# The file names are what `extension/src/sound.ts` plays. Each cue: how to make it, the
# level it is set to, and how much saturation it gets on the way.
CUES = {
    'shutter-classic': (classic, CLICK_LUFS, 0.0),
    'shutter-pop': (pop, CLICK_LUFS, 0.0),
    'shutter-subtle': (subtle, CLICK_LUFS, 1.6),
    'shutter-8bit': (eight_bit, CLICK_LUFS, 0.0),
    'shutter-soft': (soft, CLICK_LUFS, 0.8),
    'countdown-tick': (tick, TICK_LUFS, 1.4),
    'record-start': (record_start, TARGET_LUFS, 0.0),
    'record-stop': (record_stop, TARGET_LUFS, 0.0),
    'text-copied': (text_copied, TARGET_LUFS, 0.0),
    'tink': (tink, TARGET_LUFS, 0.0),
}


# --- Measuring and writing -----------------------------------------------------------------

def write_float_wav(path, samples):
    """A mono 32-bit float WAV: headroom for the gain that follows the measurement."""
    data = struct.pack('<%df' % len(samples), *samples)
    with open(path, 'wb') as f:
        f.write(b'RIFF' + struct.pack('<I', 36 + len(data)) + b'WAVE')
        f.write(b'fmt ' + struct.pack('<IHHIIHH', 16, 3, 1, RATE, RATE * 4, 4, 32))
        f.write(b'data' + struct.pack('<I', len(data)) + data)


def measure(path):
    """(integrated LUFS, true peak dBTP, seconds) of `path`, by ffmpeg's `ebur128`."""
    run = subprocess.run(
        ['ffmpeg', '-hide_banner', '-nostats', '-i', path,
         '-af', 'apad=pad_dur=1,ebur128=peak=true', '-f', 'null', '-'],
        capture_output=True, text=True, check=True)
    summary = run.stderr[run.stderr.rfind('Summary:'):]
    lufs = float(re.search(r'I:\s*(-?[\d.]+) LUFS', summary).group(1))
    peak = float(re.search(r'Peak:\s*(-?[\d.]+|-inf) dBFS', summary).group(1))
    probe = subprocess.run(
        ['ffprobe', '-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', path],
        capture_output=True, text=True, check=True)
    return lufs, peak, float(probe.stdout.strip())


def encode(wav, oga):
    subprocess.run(
        ['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-i', wav,
         '-c:a', 'libvorbis', '-q:a', VORBIS_QUALITY, '-map_metadata', '-1',
         '-fflags', '+bitexact', '-flags:a', '+bitexact', oga],
        check=True)


def gain_for(target, lufs, peak):
    """The gain, in dB, that brings a cue to its target or to the ceiling, whichever first."""
    return min(target - lufs, CEILING_DBTP - peak)


def build(name, scratch, keep):
    make, target, drive = CUES[name]
    samples = make()
    if drive > 0:
        samples = saturate(samples, drive)
    samples = dc_blocked(samples)
    wav = os.path.join(scratch, name + '.wav')
    write_float_wav(wav, samples)
    lufs, peak, _ = measure(wav)
    gain = 10 ** (gain_for(target, lufs, peak) / 20)
    oga = os.path.join(HERE, name + '.oga')
    # Twice at most: Vorbis moves the true peak by a few tenths, which the second pass
    # takes back off.
    for _ in range(2):
        write_float_wav(wav, [s * gain for s in samples])
        encode(wav, oga)
        lufs, peak, seconds = measure(oga)
        if peak <= CEILING_DBTP + 0.05:
            break
        gain *= 10 ** ((CEILING_DBTP - peak - 0.05) / 20)
    if keep:
        shutil.move(wav, os.path.join(keep, name + ".wav"))
    return lufs, peak, seconds


def report(name, lufs, peak, seconds):
    target = CUES[name][1]
    limit = 'peak' if peak >= CEILING_DBTP - 0.3 and lufs < target - TOLERANCE_LU else 'level'
    print(f'{name:16} {seconds * 1000:5.0f} ms  {lufs:6.1f} LUFS  {peak:5.1f} dBTP  '
          f'{os.path.getsize(os.path.join(HERE, name + ".oga")):6d} B  set by {limit}')


def check():
    """Every cue on target or at the ceiling, and none too long."""
    bad = []
    for name, (_, target, _) in CUES.items():
        path = os.path.join(HERE, name + '.oga')
        if not os.path.exists(path):
            bad.append(f'{name}: missing')
            continue
        lufs, peak, seconds = measure(path)
        report(name, lufs, peak, seconds)
        if peak > CEILING_DBTP + 0.1:
            bad.append(f'{name}: true peak {peak} dBTP')
        if lufs > target + TOLERANCE_LU:
            bad.append(f'{name}: {lufs} LUFS is louder than {target}')
        if lufs < target - TOLERANCE_LU and peak < CEILING_DBTP - 0.3:
            bad.append(f'{name}: {lufs} LUFS is quiet with room to spare ({peak} dBTP)')
        if seconds > LONGEST_S + 0.005:
            bad.append(f'{name}: {seconds:.3f} s is longer than {LONGEST_S} s')
    for line in bad:
        print('OFF   ' + line)
    return not bad


def main(argv):
    if '--check' in argv:
        return 0 if check() else 1
    keep = None
    if '--wav' in argv:
        keep = argv[argv.index('--wav') + 1]
        os.makedirs(keep, exist_ok=True)
    with tempfile.TemporaryDirectory() as scratch:
        for name in CUES:
            report(name, *build(name, scratch, keep))
    return 0 if check() else 1


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
