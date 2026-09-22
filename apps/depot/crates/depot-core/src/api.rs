//! Types of depot's JSON API, shared with its clients (gitmind links
//! depot-core for these), so both sides agree on the shape by construction.

use serde::{Deserialize, Serialize};

/// `GET /api/alpine`: what an Alpine client can install from this depot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AlpineListing {
    pub repos: Vec<AlpineRepoListing>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlpineRepoListing {
    /// `<branch>/<repo>/<arch>`
    pub repo: String,
    /// Path of the served repository, e.g. `/alpine/v3.22/main/aarch64/`.
    pub url: String,
    pub index_size: u64,
    pub packages: Vec<AlpinePkgListing>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlpinePkgListing {
    pub name: String,
    pub version: String,
    pub size: u64,
}

impl AlpineListing {
    pub fn from_catalog(c: &crate::catalog::Catalog) -> AlpineListing {
        AlpineListing {
            repos: c.alpine.iter().map(|(k, r)| AlpineRepoListing {
                repo: k.clone(),
                url: format!("/alpine/{k}/"),
                index_size: r.index_size,
                packages: r.packages.iter().map(|p| AlpinePkgListing { name: p.name.clone(), version: p.version.clone(), size: p.size }).collect(),
            }).collect(),
        }
    }

    pub fn repo(&self, key: &str) -> Option<&AlpineRepoListing> {
        self.repos.iter().find(|r| r.repo == key)
    }
}
