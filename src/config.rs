
use log::{debug, trace};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use toml::value::Table;
use toml::{self, Value};

use crate::errors::*;
use crate::utils::{self, toml_ext::TomlExt};

/// The overall configuration object for MDBook, essentially an in-memory
/// representation of `book.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Metadata about the book.
    pub book: BookConfig,
    /// Information about the build environment.
    pub build: BuildConfig,
    /// Information about Rust language support.
    pub rust: RustConfig,
    rest: Value,
}

impl FromStr for Config {
    type Err = Error;

    /// Load a `Config` from some string.
    fn from_str(src: &str) -> Result<Self> {
        toml::from_str(src).with_context(|| "Invalid configuration file")
    }
}

impl Config {
    /// Load the configuration file from disk.
    pub fn from_disk<P: AsRef<Path>>(config_file: P) -> Result<Config> {
        let mut buffer = String::new();
        File::open(config_file)
            .with_context(|| "Unable to open the configuration file")?
            .read_to_string(&mut buffer)
            .with_context(|| "Couldn't read the file")?;

        Config::from_str(&buffer)
    }

    pub fn update_from_env(&mut self) {
        debug!("Updating the config from environment variables");

        let overrides =
            env::vars().filter_map(|(key, value)| parse_env(&key).map(|index| (index, value)));

        for (key, value) in overrides {
            trace!("{} => {}", key, value);
            let parsed_value = serde_json::from_str(&value)
                .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));

            if key == "ftd_output" || key == "build" {
                if let serde_json::Value::Object(ref map) = parsed_value {
                    // To `set` each `key`, we wrap them as `prefix.key`
                    for (k, v) in map {
                        let full_key = format!("{}.{}", key, k);
                        self.set(&full_key, v).expect("unreachable");
                    }
                    return;
                }
            }

            self.set(key, parsed_value).expect("unreachable");
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.rest.read(key)
    }

    /// Fetch a value from the `Config` so you can mutate it.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.rest.read_mut(key)
    }

    #[doc(hidden)]
    pub fn html_config(&self) -> Option<HtmlConfig> {
        match self
            .get_deserialized_opt("output.html")
            .with_context(|| "Parsing configuration [output.html]")
        {
            Ok(Some(config)) => Some(config),
            Ok(None) => None,
            Err(e) => {
                utils::log_backtrace(&e);
                None
            }
        }
    }

    /// Deprecated, use get_deserialized_opt instead.
    #[deprecated = "use get_deserialized_opt instead"]
    pub fn get_deserialized<'de, T: Deserialize<'de>, S: AsRef<str>>(&self, name: S) -> Result<T> {
        let name = name.as_ref();
        match self.get_deserialized_opt(name)? {
            Some(value) => Ok(value),
            None => bail!("Key not found, {:?}", name),
        }
    }

    /// Convenience function to fetch a value from the config and deserialize it
    /// into some arbitrary type.
    pub fn get_deserialized_opt<'de, T: Deserialize<'de>, S: AsRef<str>>(
        &self,
        name: S,
    ) -> Result<Option<T>> {
        let name = name.as_ref();
        self.get(name)
            .map(|value| {
                value
                    .clone()
                    .try_into()
                    .with_context(|| "Couldn't deserialize the value")
            })
            .transpose()
    }

    /// Set a config key, clobbering any existing values along the way.
    ///
    /// The only way this can fail is if we can't serialize `value` into a
    /// `toml::Value`.
    pub fn set<S: Serialize, I: AsRef<str>>(&mut self, index: I, value: S) -> Result<()> {
        let index = index.as_ref();

        let value = Value::try_from(value)
            .with_context(|| "Unable to represent the item as a JSON Value")?;

        if let Some(key) = index.strip_prefix("book.") {
            self.book.update_value(key, value);
        } else if let Some(key) = index.strip_prefix("build.") {
            self.build.update_value(key, value);
        } else {
            self.rest.insert(index, value);
        }

        Ok(())
    }

    /// Get the table associated with a particular renderer.
    pub fn get_renderer<I: AsRef<str>>(&self, index: I) -> Option<&Table> {
        let key = format!("output.{}", index.as_ref());
        self.get(&key).and_then(Value::as_table)
    }

    /// Get the table associated with a particular preprocessor.
    pub fn get_preprocessor<I: AsRef<str>>(&self, index: I) -> Option<&Table> {
        let key = format!("preprocessor.{}", index.as_ref());
        self.get(&key).and_then(Value::as_table)
    }

}

impl Default for Config {
    fn default() -> Config {
        Config {
            book: BookConfig::default(),
            build: BuildConfig::default(),
            rust: RustConfig::default(),
            rest: Value::Table(Table::default()),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Config {
    fn deserialize<D: Deserializer<'de>>(de: D) -> std::result::Result<Self, D::Error> {
        let raw = Value::deserialize(de)?;


        use serde::de::Error;
        let mut table = match raw {
            Value::Table(t) => t,
            _ => {
                return Err(D::Error::custom(
                    "A config file should always be a toml table",
                ));
            }
        };

        let book: BookConfig = table
            .remove("ftd_output")
            .map(|book| book.try_into().map_err(D::Error::custom))
            .transpose()?
            .unwrap_or_default();

        let build: BuildConfig = table
            .remove("build")
            .map(|build| build.try_into().map_err(D::Error::custom))
            .transpose()?
            .unwrap_or_default();

        let rust: RustConfig = table
            .remove("rust")
            .map(|rust| rust.try_into().map_err(D::Error::custom))
            .transpose()?
            .unwrap_or_default();

        Ok(Config {
            book,
            build,
            rust,
            rest: Value::Table(table),
        })
    }
}

impl Serialize for Config {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        // TODO: This should probably be removed and use a derive instead.
        let mut table = self.rest.clone();

        let book_config = Value::try_from(&self.book).expect("should always be serializable");
        table.insert("ftd_output", book_config);

        if self.build != BuildConfig::default() {
            let build_config = Value::try_from(&self.build).expect("should always be serializable");
            table.insert("build", build_config);
        }

        if self.rust != RustConfig::default() {
            let rust_config = Value::try_from(&self.rust).expect("should always be serializable");
            table.insert("rust", rust_config);
        }

        table.serialize(s)
    }
}

fn parse_env(key: &str) -> Option<String> {
    key.strip_prefix("ftd_")
        .map(|key| key.to_lowercase().replace("__", ".").replace('_', "-"))
}

/// Configuration options which are specific to the book and required for
/// loading it from disk.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct BookConfig {
    /// The book's title.
    pub title: Option<String>,
    /// The book's authors.
    pub authors: Vec<String>,
    /// An optional description for the book.
    pub description: Option<String>,
    /// Location of the book source relative to the book's root directory.
    pub src: PathBuf,
    /// Does this book support more than one language?
    pub multilingual: bool,
    /// The main language of the book.
    pub language: Option<String>,
}

impl Default for BookConfig {
    fn default() -> BookConfig {
        BookConfig {
            title: None,
            authors: Vec::new(),
            description: None,
            src: PathBuf::from("src"),
            multilingual: false,
            language: Some(String::from("en")),
        }
    }
}

/// Configuration for the build procedure.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct BuildConfig {
    /// Where to put built artefacts relative to the book's root directory.
    pub build_dir: PathBuf,
    /// Should non-existent markdown files specified in `SUMMARY.md` be created
    /// if they don't exist?
    pub create_missing: bool,
    /// Should the default preprocessors always be used when they are
    /// compatible with the renderer?
    pub use_default_preprocessors: bool,
    /// Extra directories to trigger rebuild when watching/serving
    pub extra_watch_dirs: Vec<PathBuf>,
}

impl Default for BuildConfig {
    fn default() -> BuildConfig {
        BuildConfig {
            build_dir: PathBuf::from("ftd_output"),
            create_missing: true,
            use_default_preprocessors: true,
            extra_watch_dirs: Vec::new(),
        }
    }
}

/// Configuration for the Rust compiler(e.g., for playground)
#[derive(Debug, Default, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct RustConfig {
    /// Rust edition used in playground
    pub edition: Option<RustEdition>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
/// Rust edition to use for the code.
pub enum RustEdition {
    /// The 2021 edition of Rust
    #[serde(rename = "2021")]
    E2021,
    /// The 2018 edition of Rust
    #[serde(rename = "2018")]
    E2018,
    /// The 2015 edition of Rust
    #[serde(rename = "2015")]
    E2015,
}

/// Configuration for the HTML renderer.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct HtmlConfig {
    /// The theme directory, if specified.
    pub theme: Option<PathBuf>,
    /// The default theme to use, defaults to 'light'
    pub default_theme: Option<String>,
    /// The theme to use if the browser requests the dark version of the site.
    /// Defaults to 'navy'.
    pub preferred_dark_theme: Option<String>,
    /// Use "smart quotes" instead of the usual `"` character.
    pub curly_quotes: bool,
    /// Should mathjax be enabled?
    pub mathjax_support: bool,
    /// Whether to fonts.css and respective font files to the output directory.
    pub copy_fonts: bool,
    /// An optional google analytics code.
    pub google_analytics: Option<String>,
    /// Additional CSS stylesheets to include in the rendered page's `<head>`.
    pub additional_css: Vec<PathBuf>,
    /// Additional JS scripts to include at the bottom of the rendered page's
    /// `<body>`.
    pub additional_js: Vec<PathBuf>,
    /// Fold settings.
    pub fold: Fold,
    /// Playground settings.
    #[serde(alias = "playpen")]
    pub playground: Playground,
    /// Print settings.
    pub print: Print,
    /// Don't render section labels.
    pub no_section_label: bool,
    /// Search settings. If `None`, the default will be used.
    pub search: Option<Search>,
    /// Git repository url. If `None`, the git button will not be shown.
    pub git_repository_url: Option<String>,
    /// FontAwesome icon class to use for the Git repository link.
    /// Defaults to `fa-github` if `None`.
    pub git_repository_icon: Option<String>,
    /// Input path for the 404 file, defaults to 404.md, set to "" to disable 404 file output
    pub input_404: Option<String>,
    /// Absolute url to site, used to emit correct paths for the 404 page, which might be accessed in a deeply nested directory
    pub site_url: Option<String>,
    /// The DNS subdomain or apex domain at which your book will be hosted. This
    /// string will be written to a file named CNAME in the root of your site,
    /// as required by GitHub Pages (see [*Managing a custom domain for your
    /// GitHub Pages site*][custom domain]).
    ///
    /// [custom domain]: https://docs.github.com/en/github/working-with-github-pages/managing-a-custom-domain-for-your-github-pages-site
    pub cname: Option<String>,
    /// Edit url template, when set shows a "Suggest an edit" button for
    /// directly jumping to editing the currently viewed page.
    /// Contains {path} that is replaced with chapter source file path
    pub edit_url_template: Option<String>,
    /// Endpoint of websocket, for livereload usage. Value loaded from .toml
    /// file is ignored, because our code overrides this field with an
    /// internal value (`LIVE_RELOAD_ENDPOINT)
    ///
    /// This config item *should not be edited* by the end user.
    #[doc(hidden)]
    pub live_reload_endpoint: Option<String>,
    /// The mapping from old pages to new pages/URLs to use when generating
    /// redirects.
    pub redirect: HashMap<String, String>,
}

impl Default for HtmlConfig {
    fn default() -> HtmlConfig {
        HtmlConfig {
            theme: None,
            default_theme: None,
            preferred_dark_theme: None,
            curly_quotes: false,
            mathjax_support: false,
            copy_fonts: true,
            google_analytics: None,
            additional_css: Vec::new(),
            additional_js: Vec::new(),
            fold: Fold::default(),
            playground: Playground::default(),
            print: Print::default(),
            no_section_label: false,
            search: None,
            git_repository_url: None,
            git_repository_icon: None,
            edit_url_template: None,
            input_404: None,
            site_url: None,
            cname: None,
            live_reload_endpoint: None,
            redirect: HashMap::new(),
        }
    }
}

impl HtmlConfig {
    /// Returns the directory of theme from the provided root directory. If the
    /// directory is not present it will append the default directory of "theme"
    pub fn theme_dir(&self, root: &Path) -> PathBuf {
        match self.theme {
            Some(ref d) => root.join(d),
            None => root.join("theme"),
        }
    }
}

/// Configuration for how to render the print icon, print.html, and print.css.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Print {
    /// Whether print support is enabled.
    pub enable: bool,
    /// Insert page breaks between chapters. Default: `true`.
    pub page_break: bool,
}

impl Default for Print {
    fn default() -> Self {
        Self {
            enable: true,
            page_break: true,
        }
    }
}

/// Configuration for how to fold chapters of sidebar.
#[derive(Default, Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Fold {
    /// When off, all folds are open. Default: `false`.
    pub enable: bool,
    /// The higher the more folded regions are open. When level is 0, all folds
    /// are closed.
    /// Default: `0`.
    pub level: u8,
}

/// Configuration for tweaking how the the HTML renderer handles the playground.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Playground {
    /// Should playground snippets be editable? Default: `false`.
    pub editable: bool,
    /// Display the copy button. Default: `true`.
    pub copyable: bool,
    /// Copy JavaScript files for the editor to the output directory?
    /// Default: `true`.
    pub copy_js: bool,
    /// Display line numbers on playground snippets. Default: `false`.
    pub line_numbers: bool,
    /// Display the run button. Default: `true`
    pub runnable: bool,
}

impl Default for Playground {
    fn default() -> Playground {
        Playground {
            editable: false,
            copyable: true,
            copy_js: true,
            line_numbers: false,
            runnable: true,
        }
    }
}

/// Configuration of the search functionality of the HTML renderer.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Search {
    /// Enable the search feature. Default: `true`.
    pub enable: bool,
    /// Maximum number of visible results. Default: `30`.
    pub limit_results: u32,
    /// The number of words used for a search result teaser. Default: `30`.
    pub teaser_word_count: u32,
    /// Define the logical link between multiple search words.
    /// If true, all search words must appear in each result. Default: `false`.
    pub use_boolean_and: bool,
    /// Boost factor for the search result score if a search word appears in the header.
    /// Default: `2`.
    pub boost_title: u8,
    /// Boost factor for the search result score if a search word appears in the hierarchy.
    /// The hierarchy contains all titles of the parent documents and all parent headings.
    /// Default: `1`.
    pub boost_hierarchy: u8,
    /// Boost factor for the search result score if a search word appears in the text.
    /// Default: `1`.
    pub boost_paragraph: u8,
    /// True if the searchword `micro` should match `microwave`. Default: `true`.
    pub expand: bool,
    /// Documents are split into smaller parts, separated by headings. This defines, until which
    /// level of heading documents should be split. Default: `3`. (`### This is a level 3 heading`)
    pub heading_split_level: u8,
    /// Copy JavaScript files for the search functionality to the output directory?
    /// Default: `true`.
    pub copy_js: bool,
}

impl Default for Search {
    fn default() -> Search {
        // Please update the documentation of `Search` when changing values!
        Search {
            enable: true,
            limit_results: 30,
            teaser_word_count: 30,
            use_boolean_and: false,
            boost_title: 2,
            boost_hierarchy: 1,
            boost_paragraph: 1,
            expand: true,
            heading_split_level: 3,
            copy_js: true,
        }
    }
}

/// Allows you to "update" any arbitrary field in a struct by round-tripping via
/// a `toml::Value`.
///
/// This is definitely not the most performant way to do things, which means you
/// should probably keep it away from tight loops...
trait Updateable<'de>: Serialize + Deserialize<'de> {
    fn update_value<S: Serialize>(&mut self, key: &str, value: S) {
        let mut raw = Value::try_from(&self).expect("unreachable");

        if let Ok(value) = Value::try_from(value) {
            raw.insert(key, value);
        } else {
            return;
        }

        if let Ok(updated) = raw.try_into() {
            *self = updated;
        }
    }
}

impl<'de, T> Updateable<'de> for T where T: Serialize + Deserialize<'de> {}
