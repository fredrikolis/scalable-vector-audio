<!-- Concern: introduces SVA to a stranger arriving on GitHub | Non-concern: contributor workflow and internal tooling, which CONTRIBUTING.md would own | IO: none -->

# SVA: Scalable Vector Audio

[![Release](https://github.com/fredrikolis/scalable-vector-audio/actions/workflows/release.yml/badge.svg)](https://github.com/fredrikolis/scalable-vector-audio/actions/workflows/release.yml)
[![crates.io](https://img.shields.io/crates/v/sva-cli)](https://crates.io/crates/sva-cli)
[![npm sva-cli](https://img.shields.io/npm/v/@scalable-vector-audio/sva-cli?label=npm%20sva-cli)](https://www.npmjs.com/package/@scalable-vector-audio/sva-cli)
[![npm sva-wasm](https://img.shields.io/npm/v/@scalable-vector-audio/sva-wasm?label=npm%20sva-wasm)](https://www.npmjs.com/package/@scalable-vector-audio/sva-wasm)

Sounds written as equations. `sin(2*pi*440*t)` is a 440 Hz tone, and a composition is a
directory of such equations that refer to each other. A render samples them at any rate (thus
producing a scalable vector).

## The equation for a chord

### In the time domain

```math
\cos(2\pi C_4 t) + \cos(2\pi E_4 t) + \cos(2\pi G_4 t)
```

$t$ is seconds, the value is amplitude, and $C_4$ is the literal `C4`, so a term is written `cos(2*pi*C4*t)`.

### In the frequency domain

```math
\tfrac{1}{2}\delta(f - C_4) + \tfrac{1}{2}\delta(f + C_4) + \tfrac{1}{2}\delta(f - E_4) + \tfrac{1}{2}\delta(f + E_4) + \tfrac{1}{2}\delta(f - G_4) + \tfrac{1}{2}\delta(f + G_4)
```

Each $\delta$ is a spectral line at half the amplitude, paired with its conjugate at the negative
frequency, which is what a cosine is, and $\delta$ is the builtin `delta`. `ifourier` of that
node is the cosine sum above, and `fourier` crosses a term from `t` to `f`. A node is a function
of one variable: an expression holding both `t` and `f` refuses as `type.domain_mismatch`.

## A note

```math
e^{-t/0.25}\, w(t)\, \sin(2\pi C_4 t)
```
```
crop(exp(-t/0.25s), 0s, 0.5s, rise=0.005s, fall=0.05s) * sin(2*pi*C4*t)
```

$w$ is the `crop` window, zero outside $0 \le t < 0.5$, and `rise` and `fall` are raised-cosine
fades inside it, here 5 ms in and 50 ms out. `min` and `max` are builtins too, but neither has a
finite atom sum, so a term under one reads only through `sample`.

## A composition is a directory of these

A node file is one `;` comment stating what it models and neglects, then `name = value`
defaults, then one expression. One file is a composition, `master`:

```
; Models: one steady 440 Hz tone | Neglects: an envelope, a rate, and every other voice | IO: (t) -> amplitude | Tags: tone
sin(2*pi*440*t)
```

`@path(t, name=value)` substitutes another file's expression here and binds its defaults at
the call. `voice`:

```
; Models: one voice, a tone under a decay | Neglects: the pitch it is played at, which its caller binds | IO: (t, f0) -> amplitude | Tags: voice
f0 = C4
crop(exp(-t/0.25s), 0s, 0.5s, rise=0.005s, fall=0.05s) * sin(2*pi*f0*t)
```

`chord`:

```
; Models: the triad, three voices at once | Neglects: rhythm, and every note after the first | IO: (t) -> amplitude | Tags: chord
@voice(t, f0=C4) + @voice(t, f0=E4) + @voice(t, f0=G4)
```

## A master, with fx

Replace `master` with the triad fed back one sample $T$ later, over $0 \le t < 2$,
$y(t) = \tfrac{1}{2}(\mathrm{chord}(t) + 0.3 y(t - T))$:

```
; Models: the triad through one short feedback delay | Neglects: a second bar, and any mix beside the gain | IO: (t) -> amplitude | Tags: master
crop(0.5*(sample(@chord(t)) + 0.3*self(t - 1sp)), 0s, 2s)
```

`sample(...)` is the one crossing from algebra to a buffer. Above it every term is exact and
rate-free; below it there is a rate. `self` reads only what `sample` has already written, so
a feedback term sits under a `sample` and comes last in that chain.

## Literals

`C4` is a note name, `440hz` a frequency, `0.25s` a duration. `4st` and `50ct` are a semitone
and a cent as ratios, so `C4*4st` is `E4` and a chord written that way transposes with its
root. `1b` is a bar against the composition's own bpm and meter, and `1sp` is one sample at
the render's rate. The rest are `ms`, `m`, `h`, `khz` and `db`.

## sva-cli

One Rust engine, JSON on stdout, one `--help` page listing every flag beside its default.
`npm install -g @scalable-vector-audio/sva-cli` installs a prebuilt binary, `cargo install sva-cli` builds one. Put
the three files above in a directory and run it there.

```
sva-cli lint
```

`lint` checks binding, ref and tempo resolution without rendering a sample, and every
finding carries a severity. Advice and warnings exit 0. Five checks exit non-zero, all of
them about a node's doc comment or its length.

```
sva-cli trace master
```

prints what `master` reads one hop down, everything that reads it up to an entry point, and
the node that made it discrete:

```
"ty": "samples", "discrete": "sample(chord)", "down": ["chord", "master"]
```

`master` is under `down` because `self` reads it. `sva-cli render chord --as lines` refuses
and says why: the envelope's window widens every line, so `chord` has no line list. `--as
atoms` prints its terms as they stand, off the expression, with no buffer allocated.

```
sva-cli render master --as flops
sva-cli render master --as samples=/tmp/song.wav --sample-rate 48000
```

`flops` counts the render before running it, 1,896,300 operations here against the profile's
1e10 budget; a render over that budget refuses, naming the node that dominates. The second
line writes two seconds of 48 kHz float. `--as ledger` prints rms, peak and clipped per node.

Every reading says whether it is `exact` or `measured`, under which profile and at which
rate. No expression can read that rate as a number: `1sp` is the one literal measured in it,
and a node that writes one is discrete. `--sample-rate` is legal with every `--as`.

`sva-cli builtins` prints every builtin with its arity and named arguments, the unit suffixes
and the note-name grammar; the vocabulary is closed, so a name outside it does not parse.
`sva-cli new my-song` writes a larger composition, eleven files with a grid, a noise and a
tempo, and prints nine commands in the order to run them.

`sva-wasm` runs the same compositions in a browser: `npm install @scalable-vector-audio/sva-wasm`.

MIT. See `LICENSE`.

## Credit

Reddit user Rudxain [asked in 2022](https://www.reddit.com/r/AskProgramming/comments/ue4nzb/is_there_an_audio_equivalent_of_svg/), "Is there an audio equivalent of SVG?", proposed the
name Scalable Vector Audio, and sketched a 300 Hz sine in XML. This repository implements
that: a closed-form audio format, and an engine that renders it at any rate.
