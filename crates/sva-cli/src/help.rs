// Concern: the `--help` page, each flag's default printed from its own constant | Non-concern: parsing those flags (args/), the JSON a subcommand answers (output.rs) | IO: () -> the page

use sva_core::{DEFAULT_LEDGER_DEPTH, DEFAULT_MAX_PEAKS, DEFAULT_OVERSAMPLE};
use sva_engine::{DEFAULT_FRAME_SECS, DEFAULT_SAMPLE_RATE, PSYCHOACOUSTIC_V1};

/// Read off the constants the parser itself defaults to, so a printed default cannot drift
/// from the one a render actually uses.
pub fn help_text() -> String {
    let budget = PSYCHOACOUSTIC_V1.flop_budget;
    let bits = PSYCHOACOUSTIC_V1.precision_bits;
    let resolution = 20.0 * PSYCHOACOUSTIC_V1.half_lsb().log10();
    format!(
        r#"USAGE:
  sva-cli (render | analyze | lint | trace | builtins | outline | new) [arguments]

DESCRIPTION:
  A composition is a directory of node files, each one closed-form expression in
  `t` or `f`. sva-cli reads that composition and prints what it is and what it
  sounds like, as JSON on stdout. `render`, `lint` and `trace` read the current
  directory; `new` writes beside it and `analyze` reads a file.

RENDER:
  sva-cli render '<expression>' --representation <list> [--until '<condition>']
                 [--bits <n>] [--decay-floor <db>] [--rate <hz>]
                 [--flop-budget <n>] [--confirm]

  Renders one expression, in the grammar a node file's body uses, and prints one
  reading per representation under `data.representations`. `@path` reads a node
  from the current directory and `@/abs/path` one anywhere; there is no default
  target. Every reading states its `source` (exact or measured), the
  conformance profile it ran under, and its rate.

  The target's own ref may read an interval: `@piano4([0, 2b], f0=C4,
  vel=0.5, release=1s)`. Its ends are the language's literals (`s`, `ms`, `b`,
  `sp`, or a bare `0`); `[a, inf)` and `[a,)` leave the end open; the other
  arguments are the node's own named ones. The start trims the output alone:
  history before it is still computed, so a loop or a filter carries the state
  it had there. With no interval the render starts at 0, earlier only where a
  crop reaches before it, and its end is open. Inside an expression a window is
  a `crop`.

  Every node computed is cut before any sample is: where its bound times its
  gain to the output stays under the decay floor's share, it computes nothing
  more, and every cut together moves the output by the floor at most. A node
  no bound or no gain is derived for is not cut. A closed interval renders
  exactly its length, a cut region as exact zeros; an open one ends where the
  root is cut or its support ends (a crop's end, or a release and the crop
  after it). An open interval over a root never cut refuses: one no bound is
  derived for (a physical solver other than chaigne_askenfelt, a filter whose
  coefficients move) as `render.no_bound`, one that returns to a level over the
  floor forever as `render.never_ends`, and one still over it where the budget
  ends the search as `render.no_end`. A short-time transform reads its input
  whole, and refuses one with no end as `engine.unbounded_extent`.
  `data.cuts` lists each node cut and the second its extent ends at;
  `data.uncut` each node left uncut, and whether its `bound` or its `gain` is
  missing.

  `--until '<condition>'` stops the render at the first sample the condition
  holds at, or at the interval's end, whichever is first. It renders the first
  second, then twice as far each time, until the condition holds, and never
  past the interval. A condition compares (`<`, `<=`, `>`, `>=`) `t`,
  `envelope(t)` (the RMS of the `envelope` representation's frame holding `t`,
  framed by its own `frame` where one is asked) and literals, joined by
  `and`/`or`.

  `--representation <list>` takes a comma list of readings and may repeat. Each
  is a call in the language's own syntax, its options its named arguments:
  `spectrum(peaks=8, frame=50ms)`, `ledger(depth=3, brief=1)`. An entry
  followed by `=<path>` writes a file instead, listed under `written`:
  `samples=<path>.wav` writes audio, encoded as `--bits` says; any other path
  takes the reading's JSON, uncapped. A path that already holds a file refuses
  unless `--confirm` is written. `lines`, `atoms`, `derivative`, `bindings`,
  `arguments`, and a closed form's `spectrum`, `envelope` and `pitch`, read the
  expression and ignore the range.

  `--bits <n>` is the precision every sample is written to, from 2 to 52: the
  point where a series is truncated, and the encoding of a `.wav`, integer PCM
  at n bits up to 16 and 32-bit float above. `--decay-floor <db>` is the level
  decaying nodes are cut under, never below the resolution `--bits` writes;
  above it is a declared loss, decays cut early at full precision. `--rate <hz>`
  is the sample rate; no expression can read it. `--flop-budget <n>` is the
  operation count paid before a render refuses, the cut's own search included.

  `ledger` prints one row per node under the target. A row's `share` is the
  part of its reader's own energy that row accounts for, so one reader's refs
  sum to 1; a ref no addend isolates, such as one factor of a product, prints
  `null`. Each ref carrying a share is collapsed once on its own, so a ledger
  costs one collapse per attributed ref beyond the render, and `depth` bounds
  how many. `brief=1` keeps only the rows that clipped, `skim=1` drops the
  wider fields.

  `arguments` renders nothing: for every instance under the target it prints
  each builtin call's named arguments as the numbers the call was lowered
  with, a solver's whole parameter set with `written: false` on each default
  it filled in, and the operand each `min`/`max` `chosen` where a call folds
  one to a number: in named arguments and a solver's, modal bank's or
  `noise`'s positionals. One inside a filter's or cast's positional or
  `rand`'s key or seed is not listed. `at` spans are bytes of the instance's
  own body, as `sva-cli outline` counts them.

ANALYZE:
  sva-cli analyze <file.wav> --representation <list> [--confirm]

  Runs the same readings over a whole external `.wav` at its own rate, never
  resampled. Only the readings a buffer answers alone apply; the rest need the
  graph behind it. `masking(against=<file.wav>)` names the second signal
  masking reads against.

LINT:
  sva-cli lint ['<expression>' [--bits <n>] [--decay-floor <db>] [--rate <hz>]
               [--flop-budget <n>]] [--format <json|text>]

  Checks the current directory without rendering a sample. With no target it
  checks every file under its own rules. With a target, in render's grammar,
  it checks the files that target reaches, and decides the range, the cuts and
  what is left uncut as a render of it would, by the same function, reporting
  them as a render does.

  Every check prints one `data.diagnostics` item. `warning` exits 0, `error`
  exits non-zero, so branch on the verdict and never on whether the array is
  empty. No flag downgrades an error.

  error    missing-comment       no `;` comment line
           multiline-comment     more than one
           malformed-comment     not four ` | ` fields, `Models:` `Neglects:`
                                 `IO: <in> -> <out>` `Tags: <tag>[, <tag>...]`
           long-comment-block    a `;` block over 1000 characters, the line-1
                                 doc comment's own run exempted
           long-expression-body  a body over 10000 characters, a backstop rather
                                 than a complexity budget
           literal-sample-rate   a written rate where `sp` belongs
  warning  grid-rows-per-bar     a TSV grid's row count does not divide evenly
                                 into its filename's bar span
           key-is-not-a-pitch    `variables/key` holds neither a note name nor
                                 a number of hertz

TRACE:
  sva-cli trace <node|expression>

  Prints one node's position without rendering audio: what it reads (`down`, one
  hop), everything that reads it (`up`, transitively to an entry point), each
  beside the expression doing the reading, the node that made it discrete, and
  the feedback loop it sits in, if any. An interval the target reads is ignored.

BUILTINS:
  sva-cli builtins

  Prints the whole callable and syntactic vocabulary: every builtin with its
  arity and named arguments, each argument's `meaning`, `unit`, the model
  `part` it sets and whether it `moves` with `t` (a filter's cutoff, q and gain;
  every other named argument is one number, refused when it names none), each
  positional's meaning where the model states one, unit suffixes, the note-name
  grammar, reserved identifiers, special call shapes, and what has no operator
  at all.

OUTLINE:
  sva-cli outline <expression>

  Prints the parse tree the engine builds from one expression, each node with
  the byte `span` it was written in: calls by `name` with positional and named
  `args`, operators by `op`, refs by `path` with their `binds`, literals by
  `value` and `unit`, names by `name`. A node the parser supplies itself, the
  `0` of a prefix minus or the `t` of a bare `@ref`, has `written: false`.
  Reads no composition.

NEW:
  sva-cli new <name> [--idempotency-key <key>]

  Writes a starter composition at ./<name>, and refuses if that directory
  exists. `--idempotency-key <key>` records the key beside the composition, so a
  retry under the same key succeeds identically while the tree still holds what
  was written. Any other key, or an edited tree, refuses.

EXAMPLES:
  sva-cli new song1 && cd song1
  sva-cli render '@master' --representation samples=/tmp/song1.wav
  sva-cli render '@master([0, 8b])' --representation 'ledger(depth=2),loudness'
  sva-cli render '@voice/note([1s, inf), f0=C4, release=0.5s)' --decay-floor -96 \
    --until 'envelope(t) < -60db and t > 2s' --representation samples=/tmp/note.wav
  sva-cli render '@chord/home' --representation 'spectrum(peaks=8)' --rate 48000
  sva-cli lint
  sva-cli lint '@master' --bits 16
  sva-cli trace grid/phrase-2b
  sva-cli builtins

OUTPUT:
  {{"status": "success", "data": {{"target": "@master([0, 8b])", "sample_rate":
  44100, "bits": 24, "decay_floor_db": -144.5, "cuts": {{"items": [{{"node",
  "at_secs"}}]}}, "uncut": {{"items": [{{"node", "missing", "why"}}]}},
  "profile": "psychoacoustic-v1", "interval": {{"start_secs": 0, "end_secs":
  16}}, "label": {{...}}, "written": {{"items": [...]}}, "representations":
  {{"ledger": {{...}}}}, "diagnostics": {{"items": []}}}}, "meta": {{"request_id":
  "req_...", "timestamp": 1700000000}}}}

  `interval` is null where no reading read samples. An error adds "error":
  {{"code", "message", "details": {{"count", "codes"}}}}. Success or error,
  every response carries every finding in full at "data": {{"diagnostics":
  {{"items": [{{"code", "severity", "message", "location", "help"}}],
  "pagination": {{"count", "has_more", "next_cursor"}}}}}}, empty where it found
  none.

  Every collection carries that same {{items, pagination}} pair. `count` is the
  whole reading's, `has_more` says `items` holds less than that, and
  `next_cursor` is the interval start, as `0.5s`, the rest is read from. A
  reading written to a file is written whole, uncapped. This page is an
  envelope of its own, at "data": {{"help"}}.

DEFAULTS:
  --rate <hz>          the sample rate a render lays its seconds on.
                       Default {DEFAULT_SAMPLE_RATE}.
  --bits <n>           the precision every sample is written to. Default {bits},
                       the `psychoacoustic-v1` profile's own.
  --decay-floor <db>   the level decaying nodes are cut under. Default the
                       resolution `--bits` writes, {resolution:.1} dB at {bits}.
  --flop-budget <n>    the operation count paid before a render refuses.
                       Default {budget}, the `psychoacoustic-v1` profile's own.
  ledger(depth=<n>)    how deep below its target a `ledger` walks.
                       Default {DEFAULT_LEDGER_DEPTH}.
  (peaks=<n>)          peaks a `spectrum` keeps, notes a `pitch`, formants a
                       `formants`. Default {DEFAULT_MAX_PEAKS}.
  alias(oversample=<n>) the multiple `alias` re-renders at to hear what folded.
                       Default {DEFAULT_OVERSAMPLE}.
  (frame=<t>)          the step a framed reading advances by, in seconds.
                       Default {DEFAULT_FRAME_SECS}, except `spectrum`, which sizes
                       its own transform to the range unless it is set.
  bindings(node=@<path>) the instance whose bindings are read; required.
  ledger(brief, skim)  `0` unless set `1`.
  --format <json|text> how `lint` prints its findings: the envelope, or one
                       terminal line each, colored where stdout is a terminal.
                       The same objects either way. Default json.
  --confirm            replaces a destination that already holds a file. Without
                       it a path already taken refuses as `conflict` and nothing
                       is written.

EXIT CODES:
  0  success (error.code absent)
  1  internal_error (a destination could not be written)
  3  validation_error (bad arguments, a composition that failed to parse, or a
     render the engine refused)
  4  conflict (a name `new` would overwrite, or a destination already holding a
     file, without `--confirm`)
  24 not_found (a `.wav` file, or a node this composition does not define)

VERB ALIASES:
  validate = lint, list = builtins, create = new, show = trace, from the `cli`
  standard's own verb list. `render` and `analyze` take a reading, which that
  list has no word for, so they keep their own names.

SEE ALSO:
  sva-cli --version    Show version information"#
    )
}
