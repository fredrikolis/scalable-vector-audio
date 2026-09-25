// Concern: declares every refusal the engine reaches and the diagnostic shape each is written in | Non-concern: sva-ast's parse-time codes, sva-formula's own type codes | IO: none

use std::fmt;

use sva_ast::ByteSpan;
use sva_formula::Refusal;

/// What went wrong binding one invocation's arguments to one file's free variables.
#[derive(Clone, Debug, PartialEq)]
pub enum BindingFault {
    Unbound(String, String),
    Unused(String, String),
    Reserved(String),
    Duplicate(String),
    SelfInArgument(String),
    ShiftedRead(String, String),
    TooManyInstances(usize),
    DefaultNamesNoNumber(String, String),
}

impl BindingFault {
    pub fn code(&self) -> &'static str {
        match self {
            BindingFault::Unbound(..) => "engine.unbound_variable",
            BindingFault::Unused(..) => "engine.unused_argument",
            BindingFault::Reserved(_) => "engine.reserved_parameter",
            BindingFault::Duplicate(_) => "engine.duplicate_argument",
            BindingFault::SelfInArgument(_) => "engine.self_in_argument",
            BindingFault::ShiftedRead(..) => "engine.shifted_parameter_read",
            BindingFault::TooManyInstances(_) => "engine.too_many_instances",
            // The code an agent branches on stays whatever it was first published as.
            BindingFault::DefaultNamesNoNumber(..) => "engine.default_reads_buffer",
        }
    }
}

impl fmt::Display for BindingFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindingFault::Unbound(callee, v) => write!(
                f,
                "`{v}` is free in `{callee}` and this invocation binds no `{v}=`"
            ),
            BindingFault::Unused(callee, v) => {
                write!(f, "`{v}=` binds nothing — `{callee}` leaves no `{v}` free")
            }
            BindingFault::Reserved(v) => {
                write!(
                    f,
                    "`{v}` is already bound by the language and cannot be a parameter"
                )
            }
            BindingFault::Duplicate(v) => {
                write!(f, "`{v}=` is given twice in one invocation")
            }
            BindingFault::SelfInArgument(v) => write!(
                f,
                "`{v}=` passes a `self(...)`, which would read the callee's own output, not \
                 this node's; give it a file to reference instead"
            ),
            BindingFault::ShiftedRead(v, what) => write!(
                f,
                "`{v}` is read at a shifted time but its argument holds `{what}`, whose value \
                 depends on the sample being written rather than on that time; pass a file"
            ),
            BindingFault::TooManyInstances(n) => {
                write!(f, "this composition expands past {n} function invocations")
            }
            BindingFault::DefaultNamesNoNumber(file, what) => write!(
                f,
                "`{file}`'s default names `{what}`, which settles to no one number; a default \
                 resolves with no invocation behind it, so a signal is what a caller passes"
            ),
        }
    }
}

/// Which node a refusal points at, and where inside its file. A graph holds no source text,
/// so a byte span is as far as the engine can place a subterm.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Located {
    pub node: String,
    pub span: Option<ByteSpan>,
}

impl Located {
    pub fn at(node: impl Into<String>, span: Option<ByteSpan>) -> Located {
        Located {
            node: node.into(),
            span,
        }
    }
}

impl fmt::Display for Located {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.span {
            Some(span) => write!(f, "{}:{}", self.node, span.start),
            None => write!(f, "{}", self.node),
        }
    }
}

/// The three parts of FORMAT 7.1 and 8.3, which every engine refusal is written in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub location: Located,
    pub help: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}). {}", self.message, self.location, self.help)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineError {
    Binding {
        node: String,
        span: Option<ByteSpan>,
        fault: BindingFault,
    },
    UnknownNode(String),
    AmbiguousNode(String, Vec<String>),
    RefAboveRoot(String, String),
    UnknownBuiltin(String),
    BadArity(String),
    /// A type, cast or ref refusal, already written in the shape section 16 states.
    Refused(Box<Diagnostic>),
    /// A bar is a duration this graph never resolved; the tempo pass runs above the engine.
    UnresolvedBars(Located),
}

impl EngineError {
    pub fn code(&self) -> &str {
        match self {
            EngineError::Binding { fault, .. } => fault.code(),
            EngineError::UnknownNode(_) => "engine.unknown_node",
            EngineError::AmbiguousNode(..) => "engine.ambiguous_node",
            EngineError::RefAboveRoot(..) => "engine.ref_above_root",
            EngineError::UnknownBuiltin(_) => "engine.unknown_builtin",
            EngineError::BadArity(_) => "engine.bad_arity",
            EngineError::Refused(d) => &d.code,
            EngineError::UnresolvedBars(_) => "engine.unresolved_bar_literal",
        }
    }

    /// Which node a refusal points at, where one is known.
    pub fn at(&self) -> Option<&Located> {
        match self {
            EngineError::Refused(d) => Some(&d.location),
            EngineError::UnresolvedBars(at) => Some(at),
            _ => None,
        }
    }

    pub fn refused(d: Diagnostic) -> EngineError {
        EngineError::Refused(Box::new(d))
    }

    /// A closed form refusal already names its blocking subterm by `Origin`; the caller resolves that
    /// to a node and a span before it reaches here.
    pub fn of_closed_form(r: &Refusal, at: Located, help: impl Into<String>) -> EngineError {
        EngineError::refused(Diagnostic {
            code: r.code.as_str().to_string(),
            message: r.message.clone(),
            location: at,
            help: help.into(),
        })
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Binding { node, fault, .. } => write!(f, "`{node}`: {fault}"),
            EngineError::UnknownNode(p) => write!(f, "no such node: `{p}`"),
            EngineError::AmbiguousNode(file, names) => write!(
                f,
                "`{file}` is a file with {} instances; name one: {}",
                names.len(),
                names.join(", ")
            ),
            EngineError::RefAboveRoot(node, path) => {
                write!(f, "`@{path}` in `{node}` walks above the composition root")
            }
            EngineError::UnknownBuiltin(n) => write!(
                f,
                "unresolved function `{n}` (no builtin or sugar implementation)"
            ),
            EngineError::BadArity(n) => write!(f, "wrong argument shape for `{n}`"),
            EngineError::Refused(d) => write!(f, "{d}"),
            EngineError::UnresolvedBars(at) => write!(
                f,
                "`{at}` holds a bar literal and this composition declares no bpm/meter"
            ),
        }
    }
}

impl std::error::Error for EngineError {}

/// Every code a judgment in this engine can refuse under, with what trips it. The prefix
/// says which judgment refused, per FORMAT 16.
pub static REGISTRY: [(&str, &str); 77] = [
    (
        "type.no_overload",
        "a builtin applied to operand types no row names",
    ),
    ("type.domain_mismatch", "a closed form in t meets one in f"),
    (
        "type.samples_in_closed_form",
        "a closed form meets samples or frames",
    ),
    (
        "type.rate_conflict",
        "two sampled operands at different rates",
    ),
    (
        "type.frame_mismatch",
        "two framed operands at different windows or hops",
    ),
    ("type.width_mismatch", "widths differ and neither is one"),
    (
        "type.filter_needs_a_dual",
        "a filter needs a closed form with a dual, and this one has none",
    ),
    (
        "type.non_affine_singular",
        "delta or pv with a non-affine argument",
    ),
    (
        "type.series_not_summable",
        "a sum to inf whose dual does not converge",
    ),
    (
        "type.self_gain_unbounded",
        "a linear self whose loop gain does not settle",
    ),
    (
        "type.bars_in_frequency",
        "a bar unit inside a closed form in f",
    ),
    (
        "type.non_integer_power",
        "a real exponent over a base that is not a positive constant",
    ),
    ("cast.left_algebra", "fourier or ifourier on a non-pair"),
    (
        "cast.samples_are_terminal",
        "fourier or ifourier on samples or frames",
    ),
    ("cast.stft_needs_samples", "stft on anything but samples"),
    ("cast.istft_needs_frames", "istft on anything but frames"),
    ("cast.missing_window", "stft without window= or hop="),
    (
        "collapse.no_horizon",
        "a collapse with no window to run against",
    ),
    (
        "collapse.empty_band",
        "every line sits at or above the ceiling",
    ),
    (
        "collapse.singular_in_ct",
        "a delta or pv atom reaches point sampling",
    ),
    (
        "collapse.not_evaluable",
        "a subterm with no value at a point",
    ),
    (
        "collapse.series_nesting",
        "series nested past what one instant expands",
    ),
    (
        "collapse.no_block_row",
        "a closed form in f read span by span, which one transform over a horizon places",
    ),
    (
        "collapse.over_budget",
        "a render counting more operations than the caller allowed",
    ),
    (
        "samples.zero_delay_loop",
        "a sampled loop reads no written sample",
    ),
    (
        "samples.hop_outside_cola",
        "an stft hop the window does not overlap-add across",
    ),
    (
        "samples.window_not_power_of_two",
        "an stft window that is not a power of two",
    ),
    (
        "samples.channel_out_of_range",
        "a component past the width of the value it was asked of",
    ),
    (
        "samples.grid_too_large",
        "a finite-difference grid past what one call may hold at this rate",
    ),
    (
        "samples.string_past_rate",
        "a string whose first two partials no stable grid rings at this rate",
    ),
    (
        "samples.bridge_unstable",
        "a unison whose bridge leaves its scheme not provably stable at this rate",
    ),
    (
        "samples.contact_unsettled",
        "a contact force whose solve reached no value at some sample",
    ),
    (
        "samples.state_mismatch",
        "a held state handed to call sites other than the ones it was held for",
    ),
    (
        "grammar.unknown_name",
        "an identifier that is no builtin, unit or note name",
    ),
    ("grammar.arity", "a call with the wrong argument count"),
    (
        "grammar.unknown_named_argument",
        "a named argument the builtin does not read",
    ),
    ("grammar.inf_out_of_place", "inf outside a sum bound"),
    (
        "ref.fractional_shift_on_samples",
        "a sampled read offset that is not a whole sample",
    ),
    (
        "engine.unknown_node",
        "a target no node of this composition answers for",
    ),
    (
        "engine.ambiguous_node",
        "a path more than one node answers for",
    ),
    (
        "engine.ref_above_root",
        "a ref reaching outside the composition",
    ),
    ("engine.unknown_builtin", "a call no builtin table names"),
    (
        "engine.physics_out_of_range",
        "a finite-difference argument outside the range its model admits",
    ),
    (
        "engine.bad_arity",
        "a builtin whose own lowering wanted other arguments",
    ),
    (
        "engine.unresolved_bar_literal",
        "a bar literal with no bpm and meter behind it",
    ),
    (
        "engine.unbound_variable",
        "a body reading a name no invocation bound",
    ),
    ("engine.unused_argument", "an argument the body never reads"),
    (
        "engine.reserved_parameter",
        "a parameter named t, f, pi, i or self",
    ),
    ("engine.duplicate_argument", "one argument given twice"),
    ("engine.self_in_argument", "self written inside an argument"),
    (
        "engine.shifted_parameter_read",
        "a parameter read at a shifted time",
    ),
    (
        "engine.too_many_instances",
        "more argument tuples than one node may hold",
    ),
    (
        "engine.default_reads_buffer",
        "a default naming what settles to no one number",
    ),
    (
        "engine.cyclic_substitution",
        "a closed form reaching itself around a loop of refs",
    ),
    (
        "read.no_spectral_sum",
        "an exact reading of a term no atom sum reaches",
    ),
    (
        "read.series_not_enumerable",
        "a line reading of a series whose term is no line",
    ),
    (
        "read.lines_need_unwindowed_lines",
        "a line reading of a term a window gave a width",
    ),
    (
        "engine.bad_crop_shoulder",
        "a crop shoulder outside the window it belongs to",
    ),
    (
        "engine.non_constant_argument",
        "a number-valued argument that moves",
    ),
    (
        "engine.varying_delay",
        "a delay with no whole-sample offset on the grid",
    ),
    (
        "engine.unreadable_shift",
        "a ref read at a time this engine cannot settle",
    ),
    (
        "engine.per_lane_read_on_samples",
        "a buffer read at one time per component",
    ),
    (
        "engine.forward_self_read",
        "a self-reference reaching forward in time",
    ),
    (
        "engine.series_body_not_inlinable",
        "a closed loop whose body holds a value no term carries",
    ),
    (
        "engine.unshadered_operation",
        "an operation the shader builder has no arm for",
    ),
    (
        "engine.not_materialized",
        "a reading off samples this render never held",
    ),
    (
        "engine.observation_needs_samples",
        "a sample-consuming reading asked of a closed form",
    ),
    (
        "engine.observation_needs_a_graph",
        "a reading asked of a buffer with no node behind it",
    ),
    (
        "engine.observation_not_wired",
        "a reading no buffer answers",
    ),
    (
        "engine.alias_needs_a_closed_form",
        "an alias score asked of samples with no closed form behind them",
    ),
    (
        "engine.release_not_causal",
        "a sample before key-up that would depend on release",
    ),
    (
        "engine.no_stream",
        "a stream over a node no block reads alone, or a target or binding it cannot open",
    ),
    (
        "engine.binding_not_causal",
        "a binding moved at a checkpoint where a sample before it could hear the move",
    ),
    (
        "engine.checkpoint_mismatch",
        "a checkpoint resumed on a stream or node other than the one it was taken of",
    ),
    (
        "engine.never_silent",
        "a render until silent of a node that holds a level forever",
    ),
    (
        "engine.not_silent_by",
        "a render until silent whose bound is not under the floor by the latest time",
    ),
    (
        "engine.no_tail_bound",
        "a render until silent through a node class no tail bound is derived for",
    ),
];
