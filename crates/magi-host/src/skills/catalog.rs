use super::{Client, Document, Error, Metadata, metadata, parse, wire};
use crate::cancel::Cancel;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use magi_proto::workflow::SourceClass;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub class: SourceClass,
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    sources: [Source; 3],
}

impl Roots {
    pub fn new(launch: &Path, workspace: Option<&Path>, config_home: &Path) -> Result<Self, Error> {
        wire::path(launch)?;
        wire::path(config_home)?;
        let workspace = workspace.map_or_else(|| launch.to_owned(), |path| launch.join(path));
        wire::path(&workspace)?;
        let sources = [
            Source {
                class: SourceClass::Project,
                root: workspace.join(".magi/skills"),
            },
            Source {
                class: SourceClass::Agents,
                root: workspace.join(".agents/skills"),
            },
            Source {
                class: SourceClass::User,
                root: config_home.join("magi/skills"),
            },
        ];
        for source in &sources {
            wire::path(&source.root)?;
        }
        Ok(Self { sources })
    }

    pub fn sources(&self) -> &[Source; 3] {
        &self.sources
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    Duplicate {
        name: String,
        selected: Source,
        shadowed: Source,
    },
    InvalidNames {
        source: Source,
        count: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub name: String,
    pub source: Source,
    pub metadata: Result<Metadata, Error>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    entries: BTreeMap<String, CatalogEntry>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("workflow catalog failed for {location:?}: {cause}")]
pub struct DiscoveryError {
    pub location: Source,
    pub cause: Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub source: Source,
    pub document: Document,
}

impl Catalog {
    pub async fn discover(
        client: &Client,
        roots: &Roots,
        cancel: &Cancel,
    ) -> Result<Self, DiscoveryError> {
        let mut catalog = Self {
            entries: BTreeMap::new(),
            diagnostics: Vec::new(),
        };
        for source in roots.sources() {
            let result = client
                .request(&source.root, wire::Operation::Catalog {}, cancel)
                .await;
            let listing = match result {
                Ok(wire::Payload::Catalog(listing)) => listing,
                Ok(_) => {
                    return Err(DiscoveryError {
                        location: source.clone(),
                        cause: Error::Protocol,
                    });
                }
                Err(cause) => {
                    return Err(DiscoveryError {
                        location: source.clone(),
                        cause,
                    });
                }
            };
            catalog.merge(source, listing);
        }
        Ok(catalog)
    }

    fn merge(&mut self, source: &Source, listing: wire::Listing) {
        if listing.invalid_names != 0 {
            self.diagnostics.push(Diagnostic::InvalidNames {
                source: source.clone(),
                count: listing.invalid_names,
            });
        }
        for candidate in listing.entries {
            if let Some(selected) = self.entries.get(&candidate.name) {
                self.diagnostics.push(Diagnostic::Duplicate {
                    name: candidate.name,
                    selected: selected.source.clone(),
                    shadowed: source.clone(),
                });
                continue;
            }
            let metadata = candidate.metadata.map_err(Error::Read).and_then(|text| {
                metadata(&candidate.name, text.as_bytes()).map_err(Error::Document)
            });
            self.entries.insert(
                candidate.name.clone(),
                CatalogEntry {
                    name: candidate.name,
                    source: source.clone(),
                    metadata,
                },
            );
        }
    }

    pub fn entries(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.values()
    }
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub async fn load(
        &self,
        client: &Client,
        name: &str,
        cancel: &Cancel,
    ) -> Result<Loaded, Error> {
        if cancel.is_requested() {
            return Err(Error::Cancelled);
        }
        let entry = self.entries.get(name).ok_or(Error::Missing)?;
        let expected = entry.metadata.as_ref().map_err(Clone::clone)?;
        let wire::Payload::Source(source) = client
            .request(
                &entry.source.root,
                wire::Operation::Load { name: name.into() },
                cancel,
            )
            .await?
        else {
            return Err(Error::Protocol);
        };
        let document = parse(name, source.text.as_bytes()).map_err(Error::Document)?;
        if document.name != expected.name || document.description != expected.description {
            return Err(Error::Changed);
        }
        if cancel.is_requested() {
            return Err(Error::Cancelled);
        }
        Ok(Loaded {
            source: entry.source.clone(),
            document,
        })
    }
}

#[cfg(test)]
mod tests;
