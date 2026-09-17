// Concern: declares the Env a term reads node and parameter types through | Non-concern: holding a graph (sva-engine) | IO: (NodeId) -> Ty

use crate::ty::Ty;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamId(pub u32);

/// Total on both lookups: the graph resolves every ref before a closed form reaches this crate, so a
/// missing id is the caller's contract violation, not a case to type around.
pub trait Env {
    fn node(&self, id: NodeId) -> Ty;
    fn param(&self, id: ParamId) -> Ty;
}
