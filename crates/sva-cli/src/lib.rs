// Concern: wires this CLI's subcommand modules atop sva-core's pipeline | Non-concern: the pipeline itself (sva-core), JS bindings (sva-wasm) | IO: none

mod args;
mod composition;
mod destination;
mod help;
mod lint;
mod new;
mod output;
mod panic;
mod rates;
mod render;
mod terminal;
mod trace;
mod variables;
mod wav;
mod windows;

pub use args::{
    ANALYZE_REPRESENTATIONS, AnalyzeArgs, Command, Format, RenderArgs, USAGE, parse_args,
};
pub use composition::composition;
pub use destination::{Framing, refuse_inside, write as write_destination, write_analysis};
pub use help::help_text;
pub use lint::{Finding, LintReport, entry_points, lint};
pub use new::{FILES, NEXT, Scaffolded, scaffold};
pub use output::{help_data, lint_data, new_data, trace_data, version_data};
pub use panic::{Stopped, caught, stopped};
pub use render::{analyze, render};
pub use sva_core::{
    Builtins, Callable, CliError, builtins, builtins_data, cwd, error_envelope, success_envelope,
};
pub use terminal::{colored, diagnostics_text};
pub use trace::{Traceable, trace};
pub use wav::{SampleEncoding, read_channels, read_wav, write_channels, write_wav};

pub const NAME: &str = "sva-cli";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
