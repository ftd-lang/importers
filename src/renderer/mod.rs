//! [RenderContext]: struct.RenderContext.html

pub use self::hbs_renderer::HtmlHandlebars;
pub use self::markdown_renderer::MarkdownRenderer;

//mod html_handlebars;
mod hbs_renderer;
mod markdown_renderer;

use shlex::Shlex;
use std::collections::HashMap;
use std::fs;
use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::book::Book;
use crate::config::Config;
use crate::errors::*;
use log::{error, info, trace, warn};
use toml::Value;

use serde::{Deserialize, Serialize};

pub trait Renderer {
    fn name(&self) -> &str;

    fn render(&self, ctx: &RenderContext) -> Result<()>;
}

/// The context provided to all renderers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderContext {
    pub root: PathBuf,
    pub book: Book,
    pub config: Config,
    pub destination: PathBuf,
    #[serde(skip)]
    pub(crate) chapter_titles: HashMap<PathBuf, String>,
    #[serde(skip)]
    __non_exhaustive: (),
}

impl RenderContext {
    /// Create a new `RenderContext`.
    pub fn new<P, Q>(root: P, book: Book, config: Config, destination: Q) -> RenderContext
    where
        P: Into<PathBuf>,
        Q: Into<PathBuf>,
    {
        RenderContext {
            book,
            config,
            root: root.into(),
            destination: destination.into(),
            chapter_titles: HashMap::new(),
            __non_exhaustive: (),
        }
    }

    /// Get the source directory's (absolute) path on disk.
    pub fn source_dir(&self) -> PathBuf {
        self.root.join(&self.config.book.src)
    }

    /// Load a `RenderContext` from its JSON representation.
    pub fn from_json<R: Read>(reader: R) -> Result<RenderContext> {
        serde_json::from_reader(reader).with_context(|| "Unable to deserialize the `RenderContext`")
    }
}

/// If the subprocess wishes to indicate that rendering failed, it should exit
/// with a non-zero return code.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CmdRenderer {
    name: String,
    cmd: String,
}

impl CmdRenderer {
    /// Create a new `CmdRenderer` which will invoke the provided `cmd` string.
    pub fn new(name: String, cmd: String) -> CmdRenderer {
        CmdRenderer { name, cmd }
    }

    fn compose_command(&self, root: &Path, destination: &Path) -> Result<Command> {
        let mut words = Shlex::new(&self.cmd);
        let exe = match words.next() {
            Some(e) => PathBuf::from(e),
            None => bail!("Command string was empty"),
        };

        let exe = if exe.components().count() == 1 {
            // Search PATH for the executable.
            exe
        } else {
            // Relative paths are preferred to be relative to the book root.
            let abs_exe = root.join(&exe);
            if abs_exe.exists() {
                abs_exe
            } else {
                // Historically paths were relative to the destination, but
                // this is not the preferred way.
                let legacy_path = destination.join(&exe);
                if legacy_path.exists() {
                    warn!(
                        "Renderer command `{}` uses a path relative to the \
                        renderer output directory `{}`. This was previously \
                        accepted, but has been deprecated. Relative executable \
                        paths should be relative to the book root.",
                        exe.display(),
                        destination.display()
                    );
                    legacy_path
                } else {
                    // Let this bubble through to later be handled by
                    // handle_render_command_error.
                    abs_exe
                }
            }
        };

        let mut cmd = Command::new(exe);

        for arg in words {
            cmd.arg(arg);
        }

        Ok(cmd)
    }
}

impl CmdRenderer {
    fn handle_render_command_error(&self, ctx: &RenderContext, error: io::Error) -> Result<()> {
        if let ErrorKind::NotFound = error.kind() {
            // Look for "output.{self.name}.optional".
            // If it exists and is true, treat this as a warning.
            // Otherwise, fail the build.

            let optional_key = format!("output.{}.optional", self.name);

            let is_optional = match ctx.config.get(&optional_key) {
                Some(Value::Boolean(value)) => *value,
                _ => false,
            };

            if is_optional {
                warn!(
                    "The command `{}` for backend `{}` was not found, \
                    but was marked as optional.",
                    self.cmd, self.name
                );
                return Ok(());
            } else {
                error!(
                    "The command `{0}` wasn't found, is the \"{1}\" backend installed? \
                    If you want to ignore this error when the \"{1}\" backend is not installed, \
                    set `optional = true` in the `[output.{1}]` section of the book.toml configuration file.",
                    self.cmd, self.name
                );
            }
        }
        Err(error).with_context(|| "Unable to start the backend")?
    }
}

impl Renderer for CmdRenderer {
    fn name(&self) -> &str {
        &self.name
    }

    fn render(&self, ctx: &RenderContext) -> Result<()> {
        info!("Invoking the \"{}\" renderer", self.name);
        //dbg!("came in render");
        let _ = fs::create_dir_all(&ctx.destination);

        let mut child = match self
            .compose_command(&ctx.root, &ctx.destination)?
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .current_dir(&ctx.destination)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return self.handle_render_command_error(ctx, e),
        };

        let mut stdin = child.stdin.take().expect("Child has stdin");
        if let Err(e) = serde_json::to_writer(&mut stdin, &ctx) {
            // Looks like the backend hung up before we could finish
            // sending it the render context. Log the error and keep going
            warn!("Error writing the RenderContext to the backend, {}", e);
        }

        // explicitly close the `stdin` file handle
        drop(stdin);

        let status = child
            .wait()
            .with_context(|| "Error waiting for the backend to complete")?;

        trace!("{} exited with output: {:?}", self.cmd, status);

        if !status.success() {
            error!("Renderer exited with non-zero return code.");
            bail!("The \"{}\" renderer failed", self.name);
        } else {
            Ok(())
        }
    }
}
