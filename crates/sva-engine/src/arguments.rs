// Concern: the numbers one instance's builtin calls were lowered with, and the operand each constant min/max chose | Non-concern: folding them (loops.rs), what a builtin does with them | IO: none

use sva_ast::ByteSpan;

/// Spans are bytes of the instance's own body text, as `outline` of that text reports them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arguments {
    pub node: String,
    pub calls: Vec<Called>,
    pub chosen: Vec<Chosen>,
}

/// One builtin call and the number each named argument came to where it was lowered. A
/// finite-difference solver answers its whole parameter set as the solver was handed it,
/// `written` false for a default it filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct Called {
    pub name: String,
    pub at: ByteSpan,
    pub arguments: Vec<Argument>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Argument {
    pub name: String,
    pub value: f64,
    pub written: bool,
}

/// A `min` or `max` of numbers, which operand it answered with.
#[derive(Clone, Debug, PartialEq)]
pub struct Chosen {
    pub name: String,
    pub at: ByteSpan,
    pub operands: Vec<f64>,
    pub chosen: usize,
}
