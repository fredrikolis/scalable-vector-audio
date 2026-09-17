// Concern: the `lint` codes a caller branches on, and the remediation each names | Non-concern: finding a violation (sva-cli lint.rs), framing one (cli_error.rs) | IO: (LintCode) -> tag, help

/// Every shape `lint` reports. A caller dispatches on the tag, so the tag is the contract and
/// the variant is how this workspace spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LintCode {
    NotANode,
    NotAFile,
    NoDefaultRoot,
    EntryPoint,
    EntryPointRefused,
    GridRowsPerBar,
    MissingComment,
    MalformedComment,
    MultilineComment,
    LongCommentBlock,
    LongExpressionBody,
    TagShape,
    LiteralSampleRate,
    KeyIsNotAPitch,
    WindowInsideRamp,
}

impl LintCode {
    pub const ALL: &'static [LintCode] = &[
        LintCode::NotANode,
        LintCode::NotAFile,
        LintCode::NoDefaultRoot,
        LintCode::EntryPoint,
        LintCode::EntryPointRefused,
        LintCode::GridRowsPerBar,
        LintCode::MissingComment,
        LintCode::MalformedComment,
        LintCode::MultilineComment,
        LintCode::LongCommentBlock,
        LintCode::LongExpressionBody,
        LintCode::TagShape,
        LintCode::LiteralSampleRate,
        LintCode::KeyIsNotAPitch,
        LintCode::WindowInsideRamp,
    ];

    pub fn code_str(self) -> &'static str {
        match self {
            LintCode::NotANode => "not-a-node",
            LintCode::NotAFile => "not-a-file",
            LintCode::NoDefaultRoot => "no-default-root",
            LintCode::EntryPoint => "entry-point",
            LintCode::EntryPointRefused => "entry-point-refused",
            LintCode::GridRowsPerBar => "grid-rows-per-bar",
            LintCode::MissingComment => "missing-comment",
            LintCode::MalformedComment => "malformed-comment",
            LintCode::MultilineComment => "multiline-comment",
            LintCode::LongCommentBlock => "long-comment-block",
            LintCode::LongExpressionBody => "long-expression-body",
            LintCode::TagShape => "tag-shape",
            LintCode::LiteralSampleRate => "literal-sample-rate",
            LintCode::KeyIsNotAPitch => "key-is-not-a-pitch",
            LintCode::WindowInsideRamp => "window-inside-ramp",
        }
    }

    /// Exhaustive, so a code cannot be added without saying what to write instead of it.
    pub fn help(self) -> &'static str {
        match self {
            LintCode::NotANode => {
                "rename it to what a `@ref` can spell — letters, digits, `_`, `-`, `/` — if it \
                 is meant to be read as a node"
            }
            LintCode::NotAFile => {
                "move the socket, FIFO or device out of the composition directory; a node is \
                 one regular file's whole text"
            }
            LintCode::NoDefaultRoot => {
                "add a `master` node, or name the node every render should target"
            }
            LintCode::EntryPoint => {
                "ref the node from something above it, or delete it if the name is a typo"
            }
            LintCode::EntryPointRefused => {
                "fix the node the refusal names, or delete it: a whole-directory render reaches it"
            }
            LintCode::GridRowsPerBar => {
                "make the row count a whole multiple of the filename's bar span, or restate the span"
            }
            LintCode::MissingComment | LintCode::MalformedComment => {
                "give the node one `;`-comment line: `; Models: ... | Neglects: ... | IO: ... -> ... \
                 | Tags: ...`"
            }
            LintCode::MultilineComment => "fold the `;`-comment onto one line",
            LintCode::LongCommentBlock => {
                "move the prose out of the node and keep the one `;`-comment line"
            }
            LintCode::LongExpressionBody => "split the expression across nodes and `@ref` them",
            LintCode::TagShape => {
                "write `Tags:` as the one-line, comma-separated field the others use"
            }
            LintCode::LiteralSampleRate => {
                "write the sample period as `sp` and let the render choose its rate"
            }
            LintCode::KeyIsNotAPitch => {
                "hold one note name or a number of hertz in `variables/key`"
            }
            LintCode::WindowInsideRamp => {
                "widen the window past the ramp, or shorten the shoulder it sits inside"
            }
        }
    }
}
