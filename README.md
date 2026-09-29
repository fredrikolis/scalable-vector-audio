<!-- Concern: introduces SVA to a stranger arriving on GitHub | Non-concern: contributor workflow and internal tooling, which CONTRIBUTING.md would own | IO: none -->

# SVA: Scalable Vector Audio

[![Release](https://github.com/fredrikolis/scalable-vector-audio/actions/workflows/release.yml/badge.svg)](https://github.com/fredrikolis/scalable-vector-audio/actions/workflows/release.yml) [![crates.io](https://img.shields.io/crates/v/sva-cli)](https://crates.io/crates/sva-cli) [![npm sva-cli](https://img.shields.io/npm/v/@scalable-vector-audio/sva-cli?label=npm%20sva-cli)](https://www.npmjs.com/package/@scalable-vector-audio/sva-cli) [![npm sva-wasm](https://img.shields.io/npm/v/@scalable-vector-audio/sva-wasm?label=npm%20sva-wasm)](https://www.npmjs.com/package/@scalable-vector-audio/sva-wasm)

Sounds written as equations. $`\sin(2\pi \cdot 440\,t)`$ is a 440 Hz tone. Rendering samples an
equation at any rate, so the sound is a scalable vector.

## The equation for a chord

### In the time domain

$`\cos(2\pi\,\mathbf{C_4}\,t) + \cos(2\pi\,\mathbf{E_4}\,t) + \cos(2\pi\,\mathbf{G_4}\,t)`$

$t$ is seconds, the value is amplitude, and $`\mathbf{C_4}`$ is the note C4.

### In the frequency domain

$`\tfrac{1}{2}\delta(f - \mathbf{C_4}) + \tfrac{1}{2}\delta(f + \mathbf{C_4}) + \tfrac{1}{2}\delta(f - \mathbf{E_4}) + \tfrac{1}{2}\delta(f + \mathbf{E_4}) + \tfrac{1}{2}\delta(f - \mathbf{G_4}) + \tfrac{1}{2}\delta(f + \mathbf{G_4})`$

Each $\delta$ is a spectral line at half the amplitude, paired with its conjugate at the negative
frequency, which is what a cosine is; $\delta$ is the builtin `delta`. `ifourier` of that node is
the cosine sum, `fourier` crosses a term from `t` to `f`, and an expression holding both refuses.

## Each equation is stored in a file

A file holds one equation spelled in ASCII: one `;` comment stating what it models and
neglects, then `name = value` defaults, then the expression. The 440 Hz tone from the opening,
as a file:

```
; Models: one steady 440 Hz tone | Neglects: an envelope, a rate, and every other voice | IO: (t) -> amplitude | Tags: tone
sin(2*pi*440*t)
```

## A composition is a directory of these files

```
my-composition/
├── voice
└── chord
```

`voice` might contain:

```
; Models: one voice, a tone under a decay | Neglects: the pitch it is played at, which its caller binds | IO: (t, f0) -> amplitude | Tags: voice
f0 = C4
crop(exp(-t/0.25s), 0s, 0.5s, rise=0.005s, fall=0.05s) * sin(2*pi*f0*t)
```

`chord` might contain:

```
; Models: the triad, three voices at once | Neglects: rhythm, and every note after the first | IO: (t) -> amplitude | Tags: chord
@voice(t, f0=C4) + @voice(t, f0=E4) + @voice(t, f0=G4)
```

`@path(t, name=value)` substitutes another file's expression and binds its defaults at the call.

## A master, with fx

A third file in `my-composition/`, `master`, masters `chord`: the triad fed back one sample $T$ later, over $0 \le t < 2$,
$y(t) = \tfrac{1}{2}(\mathrm{chord}(t) + 0.3 y(t - T))$:

```
; Models: the triad through one short feedback delay | Neglects: a second bar, and any mix beside the gain | IO: (t) -> amplitude | Tags: master
crop(0.5*(sample(@chord(t)) + 0.3*self[idx(t) - 1]), 0s, 2s)
```

`sample(...)` is the one crossing from algebra to a buffer: above it every term is exact and
rate-free, below it there is a rate, and a feedback term reading what it wrote sits under it.

## Literals

`C4` is a note name, `440hz` a frequency, `0.25s` a duration. `4st` and `50ct` are a semitone
and a cent as ratios, so `C4*4st` is `E4` and a chord written that way transposes with its
root. `1b` is a bar against the composition's own bpm and meter, and `1sp` is one step of the
rate a render samples at: 1/44100 s by default, 1/48000 s at `--rate 48000`. The rest are
`ms`, `m`, `h`, `khz` and `db`.

## Reading a sample by index

| Written | Reads |
| ------- | ----- |
| `@x(e)` | `x` at the instant `e`: exact anywhere where `x` is a closed form; where `x` is a filter, loop or solver, only on its samples, so at 128 bpm and 44.1 kHz `@x(t - 0.5b)`, 41343.75 samples back, refuses |
| `@x[i]` | `x`'s stored sample at index `i`; `i` is a whole number, `idx(...)`, or `+`, `-` and `*` over those, so `@x[t - 0.5b]` refuses |
| `idx(e)` | the sample index nearest `e`, ties to even: `@x[idx(t - 0.5b)]` reads sample 41344 at 44.1 kHz, and `self[idx(t) - 1]` is a loop's sample before this one at any rate; `idx(e, floor)` and `idx(e, ceil)` round down and up |
| `self(t - d)`, `self[i]` | a loop's own past. A continuous loop, one constant delay at a gain under 1 over a closed form as in `x + 0.5*self(t - 17ms)`, reads `self(t - d)` and is its exact series at any rate. A `sample`, filter, nonlinearity, `sp` step, second delay or moving delay in a loop makes it discrete, and it reads only `self[i]`; `self(t - d)` there refuses before rendering, naming what made it discrete and the index read to write |

## sva-cli

One Rust engine, JSON on stdout, one `--help` page listing every flag beside its default.
`npm install -g @scalable-vector-audio/sva-cli` installs a prebuilt binary, `cargo install sva-cli` builds one. Put
the three files above in a directory and run it there.

```
sva-cli lint
```

`lint` checks every file on its own without rendering a sample, and every finding carries a
severity. Warnings exit 0. Seven checks exit non-zero: five about a node's doc comment or its
length, one about a rate written as a number, and `quiet-tail`, a node proven under the 24-bit
resolution more than a second before its extent ends, which a crop fixes. `sva-cli lint
'@master'` checks what `master` reaches, and prints the interval a render of it reads.

```
sva-cli trace master
```

prints what `master` reads one hop down, everything that reads it up to an entry point, and
the node that made it discrete:

```
"ty": "samples", "discrete": "sample(chord)", "down": ["chord", "master"]
```

`master` is under `down` because `self` reads it.

```
sva-cli render '@master' --representation flops
sva-cli render '@master' --representation samples=/tmp/song.wav --rate 48000
```

`flops` counts the render before running it, 1,243,620 operations here against the profile's
1e10 budget; a render over that budget refuses, naming the node that dominates. The second
line writes 48 kHz float over the two seconds `master`'s crop holds. `'@master([0, 1s])'`
names an interval; with none, a render ends where `master`'s support does.
`--until 'envelope(t) < -60db'` stops the render at the first frame under -60 dB.
`ledger` prints rms, peak and clipped per node.

Every reading says whether it is `exact` or `measured`, under which profile and at which
rate. `--rate` is the one rate a render samples at: the target is read at its instants, and
each node at the instants its reader asks for. A closed form is exact at any instant; a filter,
discrete loop or solver steps at that rate, and a read between two of its samples refuses by code.
`--rate` is legal with every representation.

`sva-cli builtins` prints every builtin with its arity and named arguments, the unit suffixes
and the note-name grammar; a name outside that closed vocabulary does not parse. `sva-cli new
my-song` writes eleven files with a grid, a noise and a tempo, and nine commands to run in order.

`sva-wasm` runs the same compositions in a browser: `npm install @scalable-vector-audio/sva-wasm`.
MIT. See `LICENSE`.

## Credit

Reddit user Rudxain [asked in 2022](https://www.reddit.com/r/AskProgramming/comments/ue4nzb/is_there_an_audio_equivalent_of_svg/), "Is there an audio equivalent of SVG?", proposed the
name Scalable Vector Audio, and sketched a 300 Hz sine in XML. This repository implements
that: a closed-form audio format, and an engine that renders it at any rate.
