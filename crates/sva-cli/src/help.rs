// Concern: the `--help` page, each flag's default printed from its own constant | Non-concern: parsing those flags (args/), the JSON a subcommand answers (output.rs) | IO: () -> the page

use sva_core::{DEFAULT_LEDGER_DEPTH, DEFAULT_MAX_PEAKS, DEFAULT_OVERSAMPLE, DEFAULT_STORE_BYTES};
use sva_engine::{DEFAULT_FRAME_SECS, DEFAULT_SAMPLE_RATE, PSYCHOACOUSTIC_V1};

/// Read off the constants the parser itself defaults to, so a printed default cannot drift
/// from the one a render actually uses.
pub fn help_text() -> String {
    let budget = PSYCHOACOUSTIC_V1.flop_budget;
    let bits = PSYCHOACOUSTIC_V1.precision_bits;
    let store_gb = DEFAULT_STORE_BYTES >> 30;
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
                 [--bits <n>] [--rate <hz>] [--flop-budget <n>] [--cache <path|none>]
                 [--confirm]

  Renders one expression, in the grammar a node file's body uses, and prints one
  reading per representation under `data.representations`. `@path` reads a node
  from the current directory and `@/abs/path` one anywhere; there is no default
  target. Every reading states its `source` (exact or measured), the
  conformance profile it ran under, and its rate.

  The target's own ref may read an interval: `@piano4([0, 2b], f0=C4,
  vel=0.5, release=1s)`, where `release` is the instrument's own parameter. Its ends are the language's literals (`s`, `ms`, `b`,
  `sp`, or a bare `0`); `[a, inf)` and `[a,)` leave the end open; the other
  arguments are the node's own named ones. The start trims the output alone:
  history before it is still computed, so a loop or a filter carries the state
  it had there. With no interval the render starts at 0, earlier only where a
  crop reaches before it, and its end is open. Inside an expression a window is
  a `crop`. A target that samples its one ref read, `sample(@piano4([0, 2b],
  f0=C4))`, reads that ref's interval and binds and the same node values.

  Every node is computed over its support met with what reads it, and nowhere
  else: outside its support a node is exactly zero. A closed interval renders
  exactly its length, nothing cut. An open one ends where the root's support
  does: a crop's end, the exact underflow of an `exp`, a clamp holding a factor
  at zero; or earlier, where a proven upper bound on the root's magnitude stays
  under the profile's silence threshold (-120 dBFS) from a sample on. A root
  with no such bound (a held sine, a loop) is never cut. The label's `pruned`
  states the threshold as `db` and the root's cut, `from` the sample the render
  is treated as silent from. A stream's term leaves `@notes` where its support
  ends. An open interval over a root whose support never ends and is never cut
  (a held sine, a physical solver) refuses as `render.no_end`. A short-time
  transform reads its input whole, and refuses one with no end as
  `engine.unbounded_extent`. A form in `f` with no dual in `t` has no samples
  and refuses as its dual does, `cast.left_algebra`.

  A render is a stream pulled to its end: a closed form, a short-time
  transform and what either reads are computed over their whole extent first,
  and every other node block by block. `--until '<condition>'` stops the render
  at the first sample the condition holds at, or at the interval's end,
  whichever is first. It is checked as each block is pulled, so no node driven
  block by block runs past the block it holds in. A condition compares (`<`,
  `<=`, `>`, `>=`) `t`, `envelope(t)` (the RMS over every channel of the
  `envelope` representation's frame holding `t`, framed by its own `frame`
  where one is asked) and literals, joined by `and`/`or`.

  `--representation <list>` takes a comma list of readings and may repeat. Each
  is a call in the language's own syntax, its options its named arguments:
  `spectrum(peaks=8, frame=50ms)`, `ledger(depth=3, brief=1)`. An entry
  followed by `=<path>` writes a file instead, listed under `written`:
  `samples=<path>.wav` writes audio, encoded as `--bits` says; any other path
  takes the reading's JSON, uncapped. A path that already holds a file refuses
  unless `--confirm` is written. `lines`, `atoms`, `derivative`, `bindings`,
  `arguments`, and a closed form's `spectrum`, `envelope` and `pitch`, read the
  expression and ignore the range.

  A closed form's `spectrum` and `pitch` are its exact lines, so a term under a
  crop or an envelope, which has a width and is no line, refuses them; so does
  `spectrum(frame=)`, as a closed form has no frames. To measure either frame
  by frame, a reader samples the target's ref, `sample(@x([0, 1s]))`, and an
  author writes `sample(...)` inside the node. A measured `spectrum` is one
  spectrum: every `frame`-long window across the range, averaged. `pitch` is framed, one entry
  per `frame`; for the spectrum at one instant, read a range one frame long.
  A measured `envelope` is framed too, and reads every channel: a frame's
  `rms` is the root mean square of all its channels' samples together, its
  `peak` the largest magnitude in any channel.

  `--bits <n>` is the precision every sample is written to, from 2 to 52: the
  point where a series is truncated, and the encoding of a `.wav`, integer PCM
  at n bits up to 16 and 32-bit float above. `--rate <hz>` (default
  {DEFAULT_SAMPLE_RATE}) is the one rate a render samples at: the target is read
  at its instants, and every node at the instants its reader asks for. A
  closed form is exact at any instant, and so is a continuous loop, one
  constant delay at a gain under 1 over a closed form such as
  `x + 0.5*self(t - 17ms)`, which is its series. A filter, a discrete loop, a
  solver and `rand` step at the rate asked for, where `1sp` is one step, so an
  `sp` count or `self[idx(t) - 1]` means one sample at whatever rate is asked.
  `rand` draws once per step, keyed by that step's index; a key between steps
  reads the step nearest it, ties to even.
  A discrete loop reads its own past only by index; `self(t - d)` in one
  refuses as `type.discrete_self_at_time`, naming what made it discrete. A
  read `@x(k*t - d)` steps `x` at `k` times the step, every input it reads,
  `sp` and `idx` with it, and rounds `d` to the nearest sample of that step,
  ties to even: shifts are rounded to the nearest sample so placements share
  one cached value; timing is exact to half a sample, and the label's
  `moved_s` states the most any read moved. A node is computed once
  per step however many reads ask for it. A node that holds state read at a
  time that moves refuses as `type.stateful_warp`, naming what
  holds its state; `@x[idx(...)]` reads its nearest step instead, as `p[idx(...)]`
  does a signal passed in as parameter `p`, and any `idx`, as
  `idx(t - 5ms - 2ms*sin(2*pi*t))`, is read sample by sample. An edit to a
  stream plays from the next sample on.
  `--flop-budget <n>` is the operation count paid before a render refuses.

  Every node's value a render computes is kept in a store on disk, named by
  what it computes: never its file's name, directory, comments, spacing, or
  the order its named arguments are written in, so a renamed or moved copy, a
  constant lifted into a default, an expression split into a file of its own or
  written back inline, and two addends swapped all read one entry. The next
  render of anything that reads the same value reads it back, bit for bit, and
  computes nothing under it. The store is on by default, at
  `$XDG_CACHE_HOME/sva`, else `~/.cache/sva`; `--cache <path>` moves it (a
  `/dev/shm` path keeps it in memory) and `--cache none` turns it off. It holds
  at most {store_gb} GB, the least recently used values going first, and a
  store another build of sva-cli wrote is emptied when opened. A render stages
  what it computes beside the store as it goes, and the store itself is written
  once, when the render ends, fails, or is stopped by SIGINT or SIGTERM.

  `ledger` prints one row per node under the target. A row's `share` is the
  part of its reader's own energy that row accounts for, so one reader's refs
  sum to 1; a ref no addend isolates, such as one factor of a product, prints
  `null`. Each ref carrying a share is collapsed once on its own, so a ledger
  costs one collapse per attributed ref beyond the render, and `depth` bounds
  how many, counted in refs from one file to another. Every row is computed
  whole over the range, however late a reader reads it. `brief=1` keeps only
  the rows that clipped, `skim=1` drops the wider fields.

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
  graph behind it.

LINT:
  sva-cli lint ['<expression>'] [--format <json|text>]

  Checks the current directory without rendering a sample. With no target it
  checks every file under its own rules, and types every entry point, which
  types every file it reaches. With a target, in render's grammar, it checks
  the files that target reaches, and prints the interval a render of it reads.
  Either refuses a type error as `render` does.

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
           arity                 a builtin called without an argument it reads,
                                 or with one it does not, as `rand(seed=k)`
  warning  grid-rows-per-bar     a TSV grid's row count does not divide evenly
                                 into its filename's bar span
           key-is-not-a-pitch    `variables/key` holds neither a note name nor
                                 a number of hertz
           parameter-has-no-default
                                 a node reads a parameter no `name = value` line
                                 binds, so a bare `@node` cannot play
           support-never-ends    a node's support, cut where its bound falls
                                 under the silence threshold, has no end for a
                                 bare render to stop at

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
  `args`, operators by `op`, refs by `path` with their `binds`, refs and `self`
  by how they `read`, `time` or `index`, literals by `value` and `unit`, names
  by `name`. A node the parser supplies itself, the `0` of a prefix minus or the
  `t` of a bare `@ref`, has `written: false`. Reads no composition.

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
  sva-cli render '@voice/note([0, inf), f0=C4, len=2s)' \
    --until 'envelope(t) < -60db and t > 1s' --representation samples=/tmp/note.wav
  sva-cli render 'sample(@chord/home)' --representation 'spectrum(peaks=8)' --rate 48000
  sva-cli render 'sample(@voice/note([0, 2s], f0=C4))' --representation pitch
  sva-cli lint
  sva-cli lint '@master'
  sva-cli trace grid/phrase-2b
  sva-cli builtins

OUTPUT:
  {{"status": "success", "data": {{"target": "@master([0, 8b])", "sample_rate":
  {DEFAULT_SAMPLE_RATE}, "bits": {bits}, "profile": "psychoacoustic-v1",
  "interval": {{"start_secs":
  0, "end_secs": 16}}, "label": {{...}}, "written": {{"items": [...]}},
  "representations": {{"ledger": {{...}}}}, "diagnostics": {{"items": []}}}},
  "meta": {{"request_id": "req_...", "timestamp": 1700000000}}}}

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
  --rate <hz>          the one rate a render samples at, and `1sp`.
                       Default {DEFAULT_SAMPLE_RATE}.
  --bits <n>           the precision every sample is written to. Default {bits},
                       the `psychoacoustic-v1` profile's own.
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
  --cache <path|none>  where the store lives. Default `$XDG_CACHE_HOME/sva`, else
                       `~/.cache/sva`, holding at most {store_gb} GB; `none` keeps
                       no store. A store that fails fails no render: it answers
                       as without one, warned why in `diagnostics`.
  --confirm            replaces a destination that already holds a file. Without
                       it a path already taken refuses as `conflict` and nothing
                       is written.

ENVIRONMENT:
  SVA_LOG=debug        `render` logs, once it ends, what it asked of each value
                       to stderr; stdout and the samples are unchanged. Any
                       other value, or none, logs nothing. One line per second
                       of output, then one per node and a total:
                         sva-cache pass hit=0 miss=190 ... reused=0 cum-hit=0.0%
                         sva-cache t=1.022s hit=1 miss=2 ... cum-hit=0.5%
                         sva-cache node hit=1 miss=0 ... reused=3 <node>
                         sva-cache total hit=1 miss=198 ... hit-rate=0.5% ...
                       `pass` is what was looked up before the first block.
                       Each value is looked up once: a `hit` was answered by
                       the store, `prefix` found a stored run up to a switch,
                       `miss` computed it, `new` kept it for the store. Each
                       read of a value past its first is `reused`.

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
