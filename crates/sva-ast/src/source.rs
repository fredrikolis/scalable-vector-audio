// Concern: declares where a composition's node texts come from, and holds one in memory | Non-concern: reading a directory (dir.rs), what a text MEANS (graph.rs) | IO: (path) -> node text

use std::borrow::Cow;
use std::collections::BTreeMap;

/// An `unreadable` path holds no text, so `get` is never asked for it — only reported.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub found: Vec<String>,
    pub unreadable: Vec<String>,
}

impl Listing {
    pub fn of(found: Vec<String>) -> Listing {
        Listing {
            found,
            unreadable: Vec::new(),
        }
    }
}

/// `get` is the whole reading surface, so a loader pulls: what nothing reaches is never read.
pub trait Source {
    fn paths(&self) -> Result<Listing, String>;

    /// `Ok(None)` is absent; `Err` is why a path this source names could not be read.
    fn get(&self, path: &str) -> Result<Option<Cow<'_, str>>, String>;

    fn name(&self) -> String {
        UNNAMED.to_string()
    }
}

const UNNAMED: &str = "this composition";

/// The deserialization target, and the builder: a flat path-to-text map is what a JSON object
/// decodes into, so a producer that is not a directory needs no loader of its own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Composition {
    nodes: BTreeMap<String, String>,
    name: Option<String>,
}

impl Composition {
    pub fn new() -> Composition {
        Composition::default()
    }

    pub fn insert(&mut self, path: impl Into<String>, text: impl Into<String>) -> &mut Composition {
        self.nodes.insert(path.into(), text.into());
        self
    }

    pub fn named(mut self, name: impl Into<String>) -> Composition {
        self.name = Some(name.into());
        self
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Composition {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(pairs: I) -> Composition {
        let mut out = Composition::new();
        for (path, text) in pairs {
            out.insert(path, text);
        }
        out
    }
}

impl Source for Composition {
    fn paths(&self) -> Result<Listing, String> {
        Ok(Listing::of(self.nodes.keys().cloned().collect()))
    }

    fn get(&self, path: &str) -> Result<Option<Cow<'_, str>>, String> {
        Ok(self.nodes.get(path).map(|t| Cow::Borrowed(t.as_str())))
    }

    fn name(&self) -> String {
        self.name.clone().unwrap_or_else(|| UNNAMED.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_composition_is_a_sorted_path_to_text_map_a_caller_fills_one_node_at_a_time() {
        let mut c = Composition::new();
        assert_eq!(c.paths().unwrap(), Listing::default());
        c.insert("song", "@drums/kick*0.5\n");
        c.insert("drums/kick", "sin(2*pi*50*t)\n");
        assert_eq!(
            c.paths().unwrap(),
            Listing::of(vec!["drums/kick".to_string(), "song".to_string()])
        );
        assert_eq!(
            c.get("drums/kick").unwrap().as_deref(),
            Some("sin(2*pi*50*t)\n")
        );
        assert_eq!(c.get("nope").unwrap(), None);
        c.insert("drums/kick", "saw(2*pi*50*t)\n");
        assert_eq!(
            c.get("drums/kick").unwrap().as_deref(),
            Some("saw(2*pi*50*t)\n"),
            "one node replaced, and no graph rebuilt to do it"
        );
    }

    #[test]
    fn a_composition_collected_from_pairs_equals_one_built_node_by_node() {
        let built = {
            let mut c = Composition::new();
            c.insert("a", "1\n").insert("b", "2\n");
            c
        };
        let collected: Composition = [("b", "2\n"), ("a", "1\n")].into_iter().collect();
        assert_eq!(built, collected);
        assert_eq!(collected.name(), "this composition");
        assert_eq!(collected.named("./song1").name(), "./song1");
    }
}
