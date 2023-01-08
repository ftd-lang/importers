
//! [1]: ../index.html

#[allow(clippy::module_inception)]
mod book;
mod summary;

pub use self::book::{load_book, Book, BookItem, BookItems, Chapter};
//pub use self::init::BookBuilder;
pub use self::summary::{parse_summary, Link, SectionNumber, Summary, SummaryItem};

use log::{debug, error, info, log_enabled, trace, warn};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::string::ToString;
use tempfile::Builder as TempFileBuilder;
use toml::Value;
use topological_sort::TopologicalSort;

use crate::errors::*;
use crate::preprocess::{
    CmdPreprocessor, IndexPreprocessor, LinkPreprocessor, Preprocessor, PreprocessorContext,
};
use crate::renderer::{CmdRenderer, HtmlHandlebars, MarkdownRenderer, RenderContext, Renderer};
use crate::utils;

use crate::config::{Config, RustEdition};

/// The object used to manage and build a book.
pub struct MDBook {
    /// The book's root directory.
    pub root: PathBuf,
    /// The configuration used to tweak now a book is built.
    pub config: Config,
    /// A representation of the book's contents in memory.
    pub book: Book,
    renderers: Vec<Box<dyn Renderer>>,

    /// List of pre-processors to be run on the book.
    preprocessors: Vec<Box<dyn Preprocessor>>,
}

impl MDBook {
    /// Load a book from its root directory on disk.
    pub fn load<P: Into<PathBuf>>(book_root: P) -> Result<MDBook> {
        let book_root = book_root.into();
        let config_location = book_root.join("config.toml");

        let mut config = if config_location.exists() {
            debug!("Loading config from {}", config_location.display());
            Config::from_disk(&config_location)?
        } else {
            Config::default()
        };

        config.update_from_env();

        if log_enabled!(log::Level::Trace) {
            for line in format!("Config: {:#?}", config).lines() {
                trace!("{}", line);
            }
        }

        MDBook::load_with_config(book_root, config)
    }

    /// Load a book from its root directory using a custom `Config`.
    pub fn load_with_config<P: Into<PathBuf>>(book_root: P, config: Config) -> Result<MDBook> {
        let root = book_root.into();

        let src_dir = root.join(&config.book.src);
        let book = book::load_book(&src_dir, &config.build)?;

        let renderers = determine_renderers(&config);
        let preprocessors = determine_preprocessors(&config)?;

        Ok(MDBook {
            root,
            config,
            book,
            renderers,
            preprocessors,
        })
    }

    /// Load a book from its root directory using a custom `Config` and a custom summary.
    pub fn load_with_config_and_summary<P: Into<PathBuf>>(
        book_root: P,
        config: Config,
        summary: Summary,
    ) -> Result<MDBook> {
        let root = book_root.into();

        let src_dir = root.join(&config.book.src);
        let book = book::load_book_from_disk(&summary, &src_dir)?;

        let renderers = determine_renderers(&config);
        let preprocessors = determine_preprocessors(&config)?;

        Ok(MDBook {
            root,
            config,
            book,
            renderers,
            preprocessors,
        })
    }

    /// ```
    pub fn iter(&self) -> BookItems<'_> {
        self.book.iter()
    }


    /// Tells the renderer to build our book and put it in the build directory.
    pub fn build(&self) -> Result<()> {
        for renderer in &self.renderers {
            self.execute_build_process(&**renderer)?;
        }

        Ok(())
    }

    /// Run the entire build process for a particular [`Renderer`].
    pub fn execute_build_process(&self, renderer: &dyn Renderer) -> Result<()> {
        //dbg!("build process");
        let mut preprocessed_book = self.book.clone();
        //dbg!(&preprocessed_book);
        let preprocess_ctx = PreprocessorContext::new(
            self.root.clone(),
            self.config.clone(),
            renderer.name().to_string(),
        );
        //dbg!(&preprocess_ctx);
        for preprocessor in &self.preprocessors {
            if preprocessor_should_run(&**preprocessor, renderer, &self.config) {
                debug!("Running the {} preprocessor.", preprocessor.name());
                preprocessed_book = preprocessor.run(&preprocess_ctx, preprocessed_book)?;
            }
        }
        let name = renderer.name();
        dbg!(&name);
        let build_dir = self.build_dir_for(name);

        let mut render_context = RenderContext::new(
            self.root.clone(),
            preprocessed_book,
            self.config.clone(),
            build_dir,
        );
        //dbg!(&render_context);
        render_context
            .chapter_titles
            .extend(preprocess_ctx.chapter_titles.borrow_mut().drain());
        //dbg!(&render_context);
        info!("Running the {} backend", renderer.name());
        renderer
            .render(&render_context)
            .with_context(|| "Rendering failed")
    }

    /// You can change the default renderer to another one by using this method.
    /// The only requirement is that your renderer implement the [`Renderer`]
    /// trait.
    pub fn with_renderer<R: Renderer + 'static>(&mut self, renderer: R) -> &mut Self {
        self.renderers.push(Box::new(renderer));
        self
    }

    /// Register a [`Preprocessor`] to be used when rendering the book.
    pub fn with_preprocessor<P: Preprocessor + 'static>(&mut self, preprocessor: P) -> &mut Self {
        self.preprocessors.push(Box::new(preprocessor));
        self
    }

    /// Run `rustdoc` tests on the book, linking against the provided libraries.
    pub fn test(&mut self, library_paths: Vec<&str>) -> Result<()> {
        // test_chapter with chapter:None will run all tests.
        self.test_chapter(library_paths, None)
    }

    /// Run `rustdoc` tests on a specific chapter of the book, linking against the provided libraries.
    /// If `chapter` is `None`, all tests will be run.
    pub fn test_chapter(&mut self, library_paths: Vec<&str>, chapter: Option<&str>) -> Result<()> {
        let library_args: Vec<&str> = (0..library_paths.len())
            .map(|_| "-L")
            .zip(library_paths.into_iter())
            .flat_map(|x| vec![x.0, x.1])
            .collect();

        let temp_dir = TempFileBuilder::new().prefix("mdbook-").tempdir()?;

        let mut chapter_found = false;

        // FIXME: Is "test" the proper renderer name to use here?
        let preprocess_context =
            PreprocessorContext::new(self.root.clone(), self.config.clone(), "test".to_string());

        let book = LinkPreprocessor::new().run(&preprocess_context, self.book.clone())?;
        // Index Preprocessor is disabled so that chapter paths continue to point to the
        // actual markdown files.

        let mut failed = false;
        for item in book.iter() {
            if let BookItem::Chapter(ref ch) = *item {
                let chapter_path = match ch.path {
                    Some(ref path) if !path.as_os_str().is_empty() => path,
                    _ => continue,
                };

                if let Some(chapter) = chapter {
                    if ch.name != chapter && chapter_path.to_str() != Some(chapter) {
                        if chapter == "?" {
                            info!("Skipping chapter '{}'...", ch.name);
                        }
                        continue;
                    }
                }
                chapter_found = true;
                info!("Testing chapter '{}': {:?}", ch.name, chapter_path);

                // write preprocessed file to tempdir
                let path = temp_dir.path().join(&chapter_path);
                let mut tmpf = utils::fs::create_file(&path)?;
                tmpf.write_all(ch.content.as_bytes())?;

                let mut cmd = Command::new("rustdoc");
                cmd.arg(&path).arg("--test").args(&library_args);

                if let Some(edition) = self.config.rust.edition {
                    match edition {
                        RustEdition::E2015 => {
                            cmd.args(&["--edition", "2015"]);
                        }
                        RustEdition::E2018 => {
                            cmd.args(&["--edition", "2018"]);
                        }
                        RustEdition::E2021 => {
                            cmd.args(&["--edition", "2021"]);
                        }
                    }
                }

                debug!("running {:?}", cmd);
                let output = cmd.output()?;

                if !output.status.success() {
                    failed = true;
                    error!(
                        "rustdoc returned an error:\n\
                        \n--- stdout\n{}\n--- stderr\n{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
            }
        }
        if failed {
            bail!("One or more tests failed");
        }
        if let Some(chapter) = chapter {
            if !chapter_found {
                bail!("Chapter not found: {}", chapter);
            }
        }
        Ok(())
    }


    ///
    pub fn build_dir_for(&self, backend_name: &str) -> PathBuf {
        let build_dir = self.root.join(&self.config.build.build_dir);

        if self.renderers.len() <= 1 {
            build_dir
        } else {
            build_dir.join(backend_name)
        }
    }

    /// Get the directory containing this book's source files.
    pub fn source_dir(&self) -> PathBuf {
        self.root.join(&self.config.book.src)
    }

    /// Get the directory containing the theme resources for the book.
    pub fn theme_dir(&self) -> PathBuf {
        self.config
            .html_config()
            .unwrap_or_default()
            .theme_dir(&self.root)
    }
}

/// Look at the `Config` and try to figure out what renderers to use.
fn determine_renderers(config: &Config) -> Vec<Box<dyn Renderer>> {
    let mut renderers = Vec::new();

    if let Some(output_table) = config.get("output").and_then(Value::as_table) {
        renderers.extend(output_table.iter().map(|(key, table)| {
            if key == "html" {
                Box::new(HtmlHandlebars::new()) as Box<dyn Renderer>
            } else if key == "markdown" {
                Box::new(MarkdownRenderer::new()) as Box<dyn Renderer>
            } else {
                interpret_custom_renderer(key, table)
            }
        }));
    }

    // if we couldn't find anything, add the HTML renderer as a default
    if renderers.is_empty() {
        renderers.push(Box::new(HtmlHandlebars::new()));
    }

    renderers
}

const DEFAULT_PREPROCESSORS: &[&str] = &["links", "index"];

fn is_default_preprocessor(pre: &dyn Preprocessor) -> bool {
    let name = pre.name();
    name == LinkPreprocessor::NAME || name == IndexPreprocessor::NAME
}

/// Look at the `MDBook` and try to figure out what preprocessors to run.
fn determine_preprocessors(config: &Config) -> Result<Vec<Box<dyn Preprocessor>>> {
    // Collect the names of all preprocessors intended to be run, and the order
    // in which they should be run.
    let mut preprocessor_names = TopologicalSort::<String>::new();

    if config.build.use_default_preprocessors {
        for name in DEFAULT_PREPROCESSORS {
            preprocessor_names.insert(name.to_string());
        }
    }

    if let Some(preprocessor_table) = config.get("preprocessor").and_then(Value::as_table) {
        for (name, table) in preprocessor_table.iter() {
            preprocessor_names.insert(name.to_string());

            let exists = |name| {
                (config.build.use_default_preprocessors && DEFAULT_PREPROCESSORS.contains(&name))
                    || preprocessor_table.contains_key(name)
            };

            if let Some(before) = table.get("before") {
                let before = before.as_array().ok_or_else(|| {
                    Error::msg(format!(
                        "Expected preprocessor.{}.before to be an array",
                        name
                    ))
                })?;
                for after in before {
                    let after = after.as_str().ok_or_else(|| {
                        Error::msg(format!(
                            "Expected preprocessor.{}.before to contain strings",
                            name
                        ))
                    })?;

                    if !exists(after) {
                        // Only warn so that preprocessors can be toggled on and off (e.g. for
                        // troubleshooting) without having to worry about order too much.
                        warn!(
                            "preprocessor.{}.after contains \"{}\", which was not found",
                            name, after
                        );
                    } else {
                        preprocessor_names.add_dependency(name, after);
                    }
                }
            }

            if let Some(after) = table.get("after") {
                let after = after.as_array().ok_or_else(|| {
                    Error::msg(format!(
                        "Expected preprocessor.{}.after to be an array",
                        name
                    ))
                })?;
                for before in after {
                    let before = before.as_str().ok_or_else(|| {
                        Error::msg(format!(
                            "Expected preprocessor.{}.after to contain strings",
                            name
                        ))
                    })?;

                    if !exists(before) {
                        // See equivalent warning above for rationale
                        warn!(
                            "preprocessor.{}.before contains \"{}\", which was not found",
                            name, before
                        );
                    } else {
                        preprocessor_names.add_dependency(before, name);
                    }
                }
            }
        }
    }

    // Now that all links have been established, queue preprocessors in a suitable order
    let mut preprocessors = Vec::with_capacity(preprocessor_names.len());
    // `pop_all()` returns an empty vector when no more items are not being depended upon
    for mut names in std::iter::repeat_with(|| preprocessor_names.pop_all())
        .take_while(|names| !names.is_empty())
    {
        names.sort();
        for name in names {
            let preprocessor: Box<dyn Preprocessor> = match name.as_str() {
                "links" => Box::new(LinkPreprocessor::new()),
                "index" => Box::new(IndexPreprocessor::new()),
                _ => {
                   
                    let table = &config.get("preprocessor").unwrap().as_table().unwrap()[&name];
                    let command = get_custom_preprocessor_cmd(&name, table);
                    Box::new(CmdPreprocessor::new(name, command))
                }
            };
            preprocessors.push(preprocessor);
        }
    }

    if preprocessor_names.is_empty() {
        Ok(preprocessors)
    } else {
        Err(Error::msg("Cyclic dependency detected in preprocessors"))
    }
}

fn get_custom_preprocessor_cmd(key: &str, table: &Value) -> String {
    table
        .get("command")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("mdbook-{}", key))
}

fn interpret_custom_renderer(key: &str, table: &Value) -> Box<CmdRenderer> {
    // look for the `command` field, falling back to using the key
    // prepended by "mdbook-"
    let table_dot_command = table
        .get("command")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let command = table_dot_command.unwrap_or_else(|| format!("mdbook-{}", key));

    Box::new(CmdRenderer::new(key.to_string(), command))
}

fn preprocessor_should_run(
    preprocessor: &dyn Preprocessor,
    renderer: &dyn Renderer,
    cfg: &Config,
) -> bool {
    // default preprocessors should be run by default (if supported)
    if cfg.build.use_default_preprocessors && is_default_preprocessor(preprocessor) {
        return preprocessor.supports_renderer(renderer.name());
    }

    let key = format!("preprocessor.{}.renderers", preprocessor.name());
    let renderer_name = renderer.name();

    if let Some(Value::Array(ref explicit_renderers)) = cfg.get(&key) {
        return explicit_renderers
            .iter()
            .filter_map(Value::as_str)
            .any(|name| name == renderer_name);
    }

    preprocessor.supports_renderer(renderer_name)
}
