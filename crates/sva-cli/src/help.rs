// Concern: the `--help` page, each flag's default printed from its own constant | Non-concern: parsing those flags (args/), the JSON a subcommand answers (output.rs) | IO: () -> the page

use sva_core::{DEFAULT_LEDGER_DEPTH, DEFAULT_MAX_PEAKS, DEFAULT_OVERSAMPLE};
use sva_engine::{DEFAULT_FRAME_SECS, DEFAULT_SAMPLE_RATE, PSYCHOACOUSTIC_V1};

/// Read off the constants the parser itself defaults to, so a printed default cannot drift
/// from the one a render actually uses.
pub fn help_text() -> String {
    let budget = PSYCHOACOUSTIC_V1.flop_budget;
    format!(
        r#"USAGE:
  sva-cli (render | analyze | lint | trace | builtins | new) [arguments]

DESCRIPTION:
  A composition is a directory of node files, each one closed-form expression in
  `t` or `f`. sva-cli reads that composition and prints what it is and what it
  sounds like, as JSON on stdout.

  `--in <dir>` picks the composition for render, lint and trace, wherever in the
  arguments it is written; without it they read the current directory. `new`
  writes beside the current directory and `analyze` reads a file, so neither
  takes `--in`.

RENDER:
  sva-cli render [<node|expression>] [query options]

  Renders a node, `master` by default, and prints one reading per `--as`. The
  argument may be an expression instead, in the grammar a node file's body uses.
  Every reading states its `source` (exact or measured), the conformance profile
  it ran under, and its rate.

  `--as lines` and `--as atoms` read the closed form and allocate no buffer.
  `--as samples=<path>.wav` writes 32-bit float audio, or 16-bit PCM under
  `--pcm16`. Any other destination path takes the same JSON, uncapped. A path
  that already holds a file refuses unless `--confirm` is written.

  `--from`/`--to` bound the window a collapse runs over. `--sample-rate <hz>` is
  the observation rate and is legal with every `--as`: no expression can read it.
  `--no-cache` skips the disk store.

  `--as ledger` prints one row per node under the target. A row's `share` is the
  part of its reader's own energy that row accounts for, so one reader's refs sum
  to 1; a ref no addend isolates, such as one factor of a product, prints `null`.
  Each ref carrying a share is collapsed once on its own, so a ledger costs one
  collapse per attributed ref beyond the render, and `--depth` bounds how many.
  `--brief` keeps only the rows that clipped, `--skim` drops the wider fields.

ANALYZE:
  sva-cli analyze <file.wav> [--as <representation>[=<destination>]]...

  Runs the same readings over an external `.wav` at its own rate, never
  resampled. Only the readings a buffer answers alone apply; the rest need the
  graph behind it.

LINT:
  sva-cli lint [<node|expression>] [--in <dir>] [--format <json|text>]

  Checks binding, ref and tempo resolution without rendering a sample. With no
  target it checks the whole directory against `master`. With a target it checks
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
  sva-cli trace <node|expression> [--in <dir>]

  Prints one node's position without rendering audio: what it reads (`down`, one
  hop), everything that reads it (`up`, transitively to an entry point), each
  beside the expression doing the reading, the node that made it discrete, and
  the feedback loop it sits in, if any.

BUILTINS:
  sva-cli builtins

  Prints the whole callable and syntactic vocabulary: every builtin with its
  arity and named arguments, unit suffixes, the note-name grammar, reserved
  identifiers, special call shapes, and what has no operator at all.

NEW:
  sva-cli new <name> [--idempotency-key <key>]

  Writes a starter composition at ./<name>, and refuses if that directory
  exists. `--idempotency-key <key>` records the key beside the composition, so a
  retry under the same key succeeds identically while the tree still holds what
  was written. Any other key, or an edited tree, refuses.

EXAMPLES:
  sva-cli new song1 && cd song1
  cd ./song1 && sva-cli render --as samples=/tmp/song1.wav
  sva-cli render master --in ./song1 --as ledger --as loudness
  cd ./song1 && sva-cli render chord/home --as lines
  sva-cli lint --in ./song1
  cd ./song1 && sva-cli lint voice/note
  cd ./song1 && sva-cli trace grid/phrase-2b
  sva-cli builtins

OUTPUT:
  {{"status": "success", "data": {{"node": "master", "down": {{"items": [...]}},
  "diagnostics": {{"items": []}}}},
   "meta": {{"request_id": "req_...", "timestamp": 1700000000}}}}

  An error adds "error": {{"code", "message", "details": {{"count", "codes"}}}}.
  Success or error, every response carries every finding in full at
  "data": {{"diagnostics": {{"items": [{{"code", "severity", "message",
  "location", "help"}}], "pagination": {{"count", "has_more", "next_cursor"}}}}}},
  empty where it found none.

  Every collection carries that same {{items, pagination}} pair. `count` is the
  whole reading's, `has_more` says `items` holds less than that, and
  `next_cursor` is a `--from` value to pass back verbatim for the rest. A framed
  measurement restarts its state at that instant, so a second page is a second
  reading rather than a continuation. `--as <name>=<path>` writes the whole
  reading to a file instead, uncapped. This page is an envelope of its own, at
  "data": {{"help"}}.

DEFAULTS:
  --depth <n>          how deep below its target a `ledger` walks.
                       Default {DEFAULT_LEDGER_DEPTH}.
  --peaks <n>          peaks a `spectrum` keeps, notes a `pitch`, formants a
                       `formants`. Default {DEFAULT_MAX_PEAKS}.
  --oversample <n>     the multiple `alias` re-renders at to hear what folded.
                       Default {DEFAULT_OVERSAMPLE}.
  --frame <secs>       the step a framed reading advances by, in seconds.
                       Default {DEFAULT_FRAME_SECS}, except `spectrum`, which
                       sizes its own transform to the window unless this flag is
                       given.
  --sample-rate <hz>   the observation rate a render lays its seconds on.
                       Default {DEFAULT_SAMPLE_RATE}.
  --flop-budget <n>    the operation count paid before a render refuses.
                       Default {budget}, the `psychoacoustic-v1` profile's own.
  --format <json|text> how `lint` prints its findings: the envelope, or one
                       terminal line each, colored where stdout is a terminal.
                       The same objects either way. Default json.
  --in <dir>           the composition `render`, `lint` and `trace` read.
                       Default: the directory the process runs in.
  --node <path>        the instance a reading is taken of. Defaults to the
                       target itself; `--as bindings` requires it.
  --against <file.wav> the second signal `--as masking` reads against. No
                       default: that one analysis requires it.
  --from <time>        default 0s; `--to <time>` defaults to the node's extent.
  --confirm            replaces a destination that already holds a file. Without
                       it a path already taken refuses as `conflict` and nothing
                       is written.
  --no-cache, --brief, --skim and --pcm16 are all off unless written.

EXIT CODES:
  0  success (error.code absent)
  1  internal_error (a destination could not be written)
  3  validation_error (bad arguments, or a composition that failed to parse)
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
