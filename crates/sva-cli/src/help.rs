// Concern: the `--help` page, each flag's default printed from its own constant | Non-concern: parsing those flags (args/), the JSON a subcommand answers (output.rs) | IO: () -> the page

use sva_core::{
    DEFAULT_LEDGER_DEPTH, DEFAULT_MAX_PEAKS, DEFAULT_OVERSAMPLE, DEFAULT_PROOF_LIMIT_SECS,
};
use sva_engine::{DEFAULT_FRAME_SECS, DEFAULT_SAMPLE_RATE, PSYCHOACOUSTIC_V1};

/// Read off the constants the parser itself defaults to, so a printed default cannot drift
/// from the one a render actually uses.
pub fn help_text() -> String {
    let budget = PSYCHOACOUSTIC_V1.flop_budget;
    format!(
        r#"USAGE:
  sva-cli (render | analyze | lint | trace | builtins | outline | new) [arguments]

DESCRIPTION:
  A composition is a directory of node files, each one closed-form expression in
  `t` or `f`. sva-cli reads that composition and prints what it is and what it
  sounds like, as JSON on stdout. `render`, `lint` and `trace` read the current
  directory; `new` writes beside it and `analyze` reads a file.

RENDER:
  sva-cli render '<expression>' [--until '<condition>'] [--representation <list>]...
                 [--rate <hz>] [-c <key>=<value>]... [--confirm]

  Renders one expression, in the grammar a node file's body uses, and prints one
  reading per representation under `data.readings`. `@path` reads a node from
  the current directory and `@/abs/path` one anywhere; there is no default
  target. Every reading states its `source` (exact or measured), the
  conformance profile it ran under, and its rate.

  The target's own ref may read an interval: `@piano4([0, 2b], f0=C4,
  vel=0.5, release=1s)`. Its ends are the language's literals (`s`, `ms`, `b`,
  `sp`, or a bare `0`); `[a, inf)` and `[a,)` leave the end open; the other
  arguments are the node's own named ones. The start trims the output alone:
  history before it is still computed, so a loop or a filter carries the state
  it had there. With no interval the range starts at 0, earlier only where a
  crop reaches before it, and its end is open. A closed interval renders exactly
  its length; an open one ends where the target's support does (a crop's end, a
  release and its cropped tail), and refuses as `render.no_stop` where that
  support never ends and no `--until` is written.

  `--until '<condition>'` ends the render at the first sample the condition
  holds at, or at the interval's end, whichever is first. There is no default
  condition, and no proof runs unless one is written. A condition compares
  (`<`, `<=`, `>`, `>=`) `t`, `envelope(t)` (the RMS of the `envelope`
  representation's 50 ms frames), `max`/`min(envelope([a, b]))` and literals,
  joined by `and`/`or`. A range reaching `inf` is answered by the tail proof, a
  bound on every later sample. An open interval over an endless support that
  nothing proves an end for refuses, naming the condition: a node holding a
  level forever as `engine.never_silent`, one no bound is derived for yet (a
  physical solver other than chaigne_askenfelt, a filter whose coefficients
  move) as `engine.no_tail_bound`, one not quiet by `-c proof_limit` as
  `engine.not_silent_by`, and a condition no proof brings about as
  `render.no_stop`. A short-time transform reads its input whole, and refuses
  one with no end as `engine.unbounded_extent`.

  `--representation <r>[=<path>][,...]` takes a comma list and may repeat. An
  entry with `=<path>` writes a file instead, listed under `written`:
  `samples=<path>.wav` writes 32-bit float audio, or 16-bit PCM under
  `-c pcm16=true`; any other path takes the reading's JSON, uncapped. A path
  that already holds a file refuses unless `--confirm` is written. `lines`,
  `atoms`, `derivative`, `bindings`, `arguments`, and a closed form's
  `spectrum`, `envelope` and `pitch`, read the expression and ignore the range.

  `--rate <hz>` is the sample rate; no expression can read it. `-c` sets the
  rest, one `key=value` each: `flop_budget`, `proof_limit`, `node` (the
  instance a reading is taken of; `bindings` requires it), `depth`, `peaks`,
  `oversample`, `frame`, `brief`, `skim` and `pcm16`.

  `ledger` prints one row per node under the target. A row's `share` is the
  part of its reader's own energy that row accounts for, so one reader's refs
  sum to 1; a ref no addend isolates, such as one factor of a product, prints
  `null`. Each ref carrying a share is collapsed once on its own, so a ledger
  costs one collapse per attributed ref beyond the render, and `depth` bounds
  how many. `brief=true` keeps only the rows that clipped, `skim=true` drops
  the wider fields.

  `arguments` renders nothing: for every instance under the target it prints
  each builtin call's named arguments as the numbers the call was lowered
  with, a solver's whole parameter set with `written: false` on each default
  it filled in, and the operand each `min`/`max` `chosen` where a call folds
  one to a number: in named arguments and a solver's, modal bank's or
  `noise`'s positionals. One inside a filter's or cast's positional or
  `rand`'s key or seed is not listed. `at` spans are bytes of the instance's
  own body, as `sva-cli outline` counts them.

ANALYZE:
  sva-cli analyze <file.wav> [--representation <list>]... [-c <key>=<value>]...

  Runs the same readings over a whole external `.wav` at its own rate, never
  resampled. Only the readings a buffer answers alone apply; the rest need the
  graph behind it. `-c` takes `frame`, `peaks`, and `against=<file.wav>`, the
  second signal `masking` reads against.

LINT:
  sva-cli lint [<node|expression>] [--format <json|text>]

  Checks binding, ref and tempo resolution in the current directory without
  rendering a sample. With no target it checks the whole directory against
  `master`. With a target it checks
  only the nodes that target reaches, and `entry-point` does not run, since the
  target's own reach references every node in it.

  Every check prints one `data.diagnostics` item. `advice` and `warning` exit 0,
  `error` exits non-zero, so branch on the verdict and never on whether the array
  is empty. No flag downgrades an error.

  error    missing-comment       no `;` comment line
           multiline-comment     more than one
           malformed-comment     not four ` | ` fields, `Models:` `Neglects:`
                                 `IO: <in> -> <out>` `Tags: <tag>[, <tag>...]`
           long-comment-block    a `;` block over 1000 characters, the line-1
                                 doc comment's own run exempted
           long-expression-body  a body over 10000 characters, a backstop rather
                                 than a complexity budget
  warning  grid-rows-per-bar     a TSV grid's row count does not divide evenly
                                 into its filename's bar span
           key-is-not-a-pitch    `variables/key` holds neither a note name nor
                                 a number of hertz
           entry-point-refused   a node a whole-directory render reaches does
                                 not type
  advice   entry-point           nothing references this node
           no-default-root       the directory has no `master`
           tag-shape             a tag over 3 lowercase words or 24 characters
           window-inside-ramp    a window sits wholly inside a crop's shoulder
           literal-sample-rate   a written rate where `sp` belongs
           not-a-file            a socket, FIFO or device in the directory
           not-a-node            a filename no `@ref` can spell

TRACE:
  sva-cli trace <node|expression>

  Prints one node's position without rendering audio: what it reads (`down`, one
  hop), everything that reads it (`up`, transitively to an entry point), each
  beside the expression doing the reading, the node that made it discrete, and
  the feedback loop it sits in, if any.

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
  sva-cli render '@master([0, 8b])' --representation ledger,loudness
  sva-cli render '@voice/note([1s, inf), f0=C4, release=0.5s)' \
    --until 'max(envelope([t, inf))) < -96db' --representation samples=/tmp/note.wav
  sva-cli render '@chord/home' --representation lines --rate 48000
  sva-cli lint
  sva-cli trace grid/phrase-2b
  sva-cli builtins

OUTPUT:
  {{"status": "success", "data": {{"target": "@master([0, 8b])", "sample_rate":
  44100, "profile": "psychoacoustic-v1", "range": {{"start_secs": 0,
  "end_secs": 16}}, "label": {{...}}, "written": {{"items": [...]}}, "readings":
  {{"ledger": {{...}}}}, "diagnostics": {{"items": []}}}}, "meta": {{"request_id":
  "req_...", "timestamp": 1700000000}}}}

  `range` is null where no reading read samples. An error adds "error":
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
  -c flop_budget=<n>   the operation count paid before a render refuses.
                       Default {budget}, the `psychoacoustic-v1` profile's own.
  -c proof_limit=<s>   how far a proof looks for the condition where the
                       interval has no end. Default {DEFAULT_PROOF_LIMIT_SECS} seconds.
  -c depth=<n>         how deep below its target a `ledger` walks.
                       Default {DEFAULT_LEDGER_DEPTH}.
  -c peaks=<n>         peaks a `spectrum` keeps, notes a `pitch`, formants a
                       `formants`. Default {DEFAULT_MAX_PEAKS}.
  -c oversample=<n>    the multiple `alias` re-renders at to hear what folded.
                       Default {DEFAULT_OVERSAMPLE}.
  -c frame=<s>         the step a framed reading advances by, in seconds.
                       Default {DEFAULT_FRAME_SECS}, except `spectrum`, which
                       sizes its own transform to the range unless it is set.
  -c node=<path>       the instance a reading is taken of. Defaults to the
                       target itself; `bindings` requires it.
  -c against=<file>    the second signal `analyze`'s `masking` reads against.
                       No default: that one analysis requires it.
  -c brief, skim and pcm16 are `false` unless set `true`.
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
