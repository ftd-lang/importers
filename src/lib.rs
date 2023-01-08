
pub mod book;
pub mod config;
pub mod preprocess;
pub mod renderer;
pub mod theme;
pub mod utils;

/// The current version of `mdbook`.
///
/// This is provided as a way for custom preprocessors and renderers to do
/// compatibility checks.
pub const MDBOOK_VERSION: &str = env!("CARGO_PKG_VERSION");

pub use crate::book::BookItem;
pub use crate::book::MDBook;
pub use crate::config::Config;
pub use crate::renderer::Renderer;

/// The error types used through out this crate.
pub mod errors {
    pub(crate) use anyhow::{bail, ensure, Context};
    pub use anyhow::{Error, Result};
}
