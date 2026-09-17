// Concern: one instance's resolved parameters as the composer wrote them | Non-concern: resolving them (render/mod.rs), which instances exist (instantiate.rs) | IO: none

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub name: String,
    pub source: String,
}
