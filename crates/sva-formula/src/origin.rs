// Concern: declares Origin, the opaque token a refusal names a subterm by | Non-concern: resolving one to file:line:col (sva-engine) | IO: none

/// Excluded from every hash and every sort key: two spellings written in different files
/// are one value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Origin(u32);

impl Origin {
    pub const UNKNOWN: Origin = Origin(u32::MAX);

    pub fn new(token: u32) -> Origin {
        Origin(token)
    }

    pub fn token(self) -> u32 {
        self.0
    }
}
