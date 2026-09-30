// Concern: the `lint` codes a caller branches on, and the remediation each names | Non-concern: finding a violation (sva-cli lint.rs), framing one (cli_error.rs) | IO: (LintCode) -> tag, help

/// Every shape `lint` reports. A caller dispatches on the tag, so the tag is the contract and
/// the variant is how this workspace spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LintCode {
    GridRowsPerBar,
    MissingComment,
    MalformedComment,
    MultilineComment,
    LongCommentBlock,
    LongExpressionBody,
    LiteralSampleRate,
    Arity,
    KeyIsNotAPitch,
}

impl LintCode {
    pub const ALL: &'static [LintCode] = &[
        LintCode::GridRowsPerBar,
        LintCode::MissingComment,
        LintCode::MalformedComment,
        LintCode::MultilineComment,
        LintCode::LongCommentBlock,
        LintCode::LongExpressionBody,
        LintCode::LiteralSampleRate,
        LintCode::Arity,
        LintCode::KeyIsNotAPitch,
    ];

    pub fn code_str(self) -> &'static str {
        match self {
            LintCode::GridRowsPerBar => "grid-rows-per-bar",
            LintCode::MissingComment => "missing-comment",
            LintCode::MalformedComment => "malformed-comment",
            LintCode::MultilineComment => "multiline-comment",
            LintCode::LongCommentBlock => "long-comment-block",
            LintCode::LongExpressionBody => "long-expression-body",
            LintCode::LiteralSampleRate => "literal-sample-rate",
            LintCode::Arity => "arity",
            LintCode::KeyIsNotAPitch => "key-is-not-a-pitch",
        }
    }

    /// Exhaustive, so a code cannot be added without saying what to write instead of it.
    pub fn help(self) -> &'static str {
        match self {
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
            LintCode::LiteralSampleRate => {
                "write the sample period as `sp` and let the render choose its rate"
            }
            LintCode::Arity => "give the call what its builtin reads, as `sva-cli builtins` lists",
            LintCode::KeyIsNotAPitch => {
                "hold one note name or a number of hertz in `variables/key`"
            }
        }
    }
}
