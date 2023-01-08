#![allow(missing_docs)]

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::errors::*;
use log::warn;
pub static INDEX: &[u8] = include_bytes!("index.hbs");



/// You should only ever use the static variables directly if you want to
/// override the user's theme with the defaults.
#[derive(Debug, Eq, PartialEq)]
pub struct Theme {
    pub index: Vec<u8>,
   
}

impl Theme {
    /// Creates a `Theme` from the given `theme_dir`.
    /// If a file is found in the theme dir, it will override the default version.
    pub fn new<P: AsRef<Path>>(theme_dir: P) -> Self {
        let theme_dir = theme_dir.as_ref();
        let mut theme = Theme::default();

        // If the theme directory doesn't exist there's no point continuing...
        if !theme_dir.exists() || !theme_dir.is_dir() {
            return theme;
        }

        // Check for individual files, if they exist copy them across
        {
            let files = vec![
                (theme_dir.join("index.hbs"), &mut theme.index),
            ];

            let load_with_warn = |filename: &Path, dest| {
                if !filename.exists() {
                    // Don't warn if the file doesn't exist.
                    return false;
                }
                if let Err(e) = load_file_contents(filename, dest) {
                    warn!("Couldn't load custom file, {}: {}", filename.display(), e);
                    false
                } else {
                    true
                }
            };

            for (filename, dest) in files {
                load_with_warn(&filename, dest);
            }

        }

        theme
    }
}

impl Default for Theme {
    fn default() -> Theme {
        Theme {
            index: INDEX.to_owned(),
        }
    }
}

/// Checks if a file exists, if so, the destination buffer will be filled with
/// its contents.
fn load_file_contents<P: AsRef<Path>>(filename: P, dest: &mut Vec<u8>) -> Result<()> {
    let filename = filename.as_ref();

    let mut buffer = Vec::new();
    File::open(filename)?.read_to_end(&mut buffer)?;

    // We needed the buffer so we'd only overwrite the existing content if we
    // could successfully load the file into memory.
    dest.clear();
    dest.append(&mut buffer);

    Ok(())
}
