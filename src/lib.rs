#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Kube Error: {0}")]
    KubeError(#[from] kube::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Expose all controller components used by main.
pub mod controller;

/// Resource type definitions.
pub mod resources;
