use crate::book::{Book, BookItem};
use crate::config::{BookConfig, Config, HtmlConfig, Playground, RustEdition};
use crate::errors::*;
use crate::renderer::{RenderContext, Renderer};
use crate::theme::{self, Theme};
use crate::utils;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use handlebars::Handlebars;
use log::{debug, trace, warn};
use once_cell::sync::Lazy;
use regex::{Captures, Regex};
use serde_json::json;

#[derive(Default)]
pub struct HtmlHandlebars;

impl HtmlHandlebars {
    pub fn new() -> Self {
        HtmlHandlebars
    }

    fn render_item(
        &self,
        item: &BookItem,
        mut ctx: RenderItemContext<'_>,
        print_content: &mut String,
    ) -> Result<()> {
        // FIXME: This should be made DRY-er and rely less on mutable state

        let (ch, path) = match item {
            BookItem::Chapter(ch) if !ch.is_draft_chapter() => (ch, ch.path.as_ref().unwrap()),
            _ => return Ok(()),
        };

        if let Some(ref edit_url_template) = ctx.html_config.edit_url_template {
            let full_path = ctx.book_config.src.to_str().unwrap_or_default().to_owned()
                + "/"
                + ch.source_path
                    .clone()
                    .unwrap_or_default()
                    .to_str()
                    .unwrap_or_default();

            let edit_url = edit_url_template.replace("{path}", &full_path);
            ctx.data
                .insert("git_repository_edit_url".to_owned(), json!(edit_url));
        }

        let content = ch.content.clone();
        let content = utils::render_markdown(&content, ctx.html_config.curly_quotes);

        let fixed_content =
            utils::render_markdown_with_path(&ch.content, ctx.html_config.curly_quotes, Some(path));
        if !ctx.is_index && ctx.html_config.print.page_break {
            // Add page break between chapters
            // See https://developer.mozilla.org/en-US/docs/Web/CSS/break-before and https://developer.mozilla.org/en-US/docs/Web/CSS/page-break-before
            // Add both two CSS properties because of the compatibility issue
            print_content
                .push_str(r#"<div style="break-before: page; page-break-before: always;"></div>"#);
        }
        print_content.push_str(&fixed_content);

        // Update the context with data for this file
        let ctx_path = path
            .to_str()
            .with_context(|| "Could not convert path to str")?;
        let filepath = Path::new(&ctx_path).with_extension("ftd");

        // "print.html" is used for the print page.
        if path == Path::new("print.md") {
            bail!("{} is reserved for internal use", path.display());
        };

        let book_title = ctx
            .data
            .get("book_title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

        let title = if let Some(title) = ctx.chapter_titles.get(path) {
            title.clone()
        } else if book_title.is_empty() {
            ch.name.clone()
        } else {
            ch.name.clone() + " - " + book_title
        };
        //dbg!(&content);
        ctx.data.insert("path".to_owned(), json!(path));
        ctx.data.insert("content".to_owned(), json!(content));
        ctx.data.insert("chapter_title".to_owned(), json!(ch.name));
        ctx.data.insert("title".to_owned(), json!(title));
        ctx.data.insert(
            "path_to_root".to_owned(),
            json!(utils::fs::path_to_root(&path)),
        );
        if let Some(ref section) = ch.number {
            ctx.data
                .insert("section".to_owned(), json!(section.to_string()));
        }

        // Render the handlebars template with the data
        //debug!("Render template");
        let rendered = ctx.handlebars.render("index", &ctx.data)?;

        let rendered =
            self.post_process(rendered, &ctx.html_config.playground, ctx.edition, &title);
        //dbg!(&filepath);
        // Write to file

        //dbg!("Creating {}", filepath.display());

        utils::fs::write_file(&ctx.destination, &filepath, rendered.as_bytes())?;

        if ctx.is_index {
            ctx.data.insert("path".to_owned(), json!("index.md"));
            ctx.data.insert("path_to_root".to_owned(), json!(""));
            ctx.data.insert("is_index".to_owned(), json!(true));
            // dbg!(&ctx.data);
            let rendered_index = ctx.handlebars.render("index", &ctx.data)?;
            //dbg!(&rendered_index);
            let rendered_index = self.post_process(
                rendered_index,
                &ctx.html_config.playground,
                ctx.edition,
                &title,
            );

            //dbg!(&ctx.destination);
            utils::fs::write_file(&ctx.destination, "index.ftd", rendered_index.as_bytes())?;
        }

        Ok(())
    }

    fn render_404(
        &self,
        ctx: &RenderContext,
        html_config: &HtmlConfig,
        src_dir: &Path,
        _handlebars: &mut Handlebars<'_>,
        data: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        //let destination = &ctx.destination;
        let content_404 = if let Some(ref filename) = html_config.input_404 {
            let path = src_dir.join(filename);
            std::fs::read_to_string(&path)
                .with_context(|| format!("unable to open 404 input file {:?}", path))?
        } else {
            // 404 input not explicitly configured try the default file 404.md
            let default_404_location = src_dir.join("404.md");
            if default_404_location.exists() {
                std::fs::read_to_string(&default_404_location).with_context(|| {
                    format!("unable to open 404 input file {:?}", default_404_location)
                })?
            } else {
                "# Document not found (404)\n\nThis URL is invalid, sorry. Please use the \
                navigation bar or search to continue."
                    .to_string()
            }
        };
        let html_content_404 = utils::render_markdown(&content_404, html_config.curly_quotes);

        let mut data_404 = data.clone();
        let base_url = if let Some(site_url) = &html_config.site_url {
            site_url
        } else {
            debug!(
                "HTML 'site-url' parameter not set, defaulting to '/'. Please configure \
                this to ensure the 404 page work correctly, especially if your site is hosted in a \
                subdirectory on the HTTP server."
            );
            "/"
        };
        data_404.insert("base_url".to_owned(), json!(base_url));
        // Set a dummy path to ensure other paths (e.g. in the TOC) are generated correctly
        data_404.insert("path".to_owned(), json!("404.md"));
        data_404.insert("content".to_owned(), json!(html_content_404));

        let mut title = String::from("Page not found");
        if let Some(book_title) = &ctx.config.book.title {
            title.push_str(" - ");
            title.push_str(book_title);
        }
        data_404.insert("title".to_owned(), json!(title));
        //let rendered = handlebars.render("index", &data_404)?;

        /*let rendered =
            self.post_process(rendered, &html_config.playground, ctx.config.rust.edition);
        let output_file = get_404_output_file(&html_config.input_404);
        utils::fs::write_file(destination, output_file, rendered.as_bytes())?;
        debug!("Creating 404.html ✓");*/
        Ok(())
    }

    #[cfg_attr(feature = "cargo-clippy", allow(clippy::let_and_return))]
    fn post_process(
        &self,
        rendered: String,
        playground_config: &Playground,
        edition: Option<RustEdition>,
        title: &String,
    ) -> String {
        //dbg!(&rendered);
        let rendered = embed_title(&rendered, title);
        //let rendered = build_header_links(&rendered);
        //let rendered = build_paragraph_with_markdown(&rendered);
        //dbg!("headers",&rendered);
        let rendered = fix_code_blocks(&rendered);
        //dbg!("block",&rendered);
        let rendered = add_playground_pre(&rendered, playground_config, edition);
        let rendered = remove_whitespaces(&rendered);
        rendered
    }

    fn copy_static_files(&self, destination: &Path) -> Result<()> {
        use crate::utils::fs::write_file;

        write_file(
            destination,
            "FPM.ftd",
            remove_whitespaces(
                "-- import: fpm

            -- fpm.package: wasif1024.github.io/fpm-site
            download-base-url: https://raw.githubusercontent.com/wasif1024/fpm-site/main
            
            -- fpm.dependency: fifthtry.github.io/doc-site as ds
            
            -- fpm.auto-import: ds
            
            -- fpm.sitemap:
            
            # Home: /
            nav-title: Home
            data: Section Data",
            )
            .as_bytes(),
        )?;

        Ok(())
    }

    /// Update the context with data for this file
    fn configure_print_version(
        &self,
        data: &mut serde_json::Map<String, serde_json::Value>,
        print_content: &str,
    ) {
        // Make sure that the Print chapter does not display the title from
        // the last rendered chapter by removing it from its context
        data.remove("title");
        data.insert("is_print".to_owned(), json!(true));
        data.insert("path".to_owned(), json!("print.md"));
        data.insert("content".to_owned(), json!(print_content));
        data.insert(
            "path_to_root".to_owned(),
            json!(utils::fs::path_to_root(Path::new("print.md"))),
        );
    }

    fn emit_redirects(
        &self,
        root: &Path,
        handlebars: &Handlebars<'_>,
        redirects: &HashMap<String, String>,
    ) -> Result<()> {
        if redirects.is_empty() {
            return Ok(());
        }

        log::debug!("Emitting redirects");

        for (original, new) in redirects {
            log::debug!("Redirecting \"{}\" → \"{}\"", original, new);
            // Note: all paths are relative to the build directory, so the
            // leading slash in an absolute path means nothing (and would mess
            // up `root.join(original)`).
            let original = original.trim_start_matches('/');
            let filename = root.join(original);
            self.emit_redirect(handlebars, &filename, new)?;
        }

        Ok(())
    }

    fn emit_redirect(
        &self,
        handlebars: &Handlebars<'_>,
        original: &Path,
        destination: &str,
    ) -> Result<()> {
        if original.exists() {
            // sanity check to avoid accidentally overwriting a real file.
            let msg = format!(
                "Not redirecting \"{}\" to \"{}\" because it already exists. Are you sure it needs to be redirected?",
                original.display(),
                destination,
            );
            return Err(Error::msg(msg));
        }

        if let Some(parent) = original.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Unable to ensure \"{}\" exists", parent.display()))?;
        }

        let ctx = json!({
            "url": destination,
        });
        let f = File::create(original)?;
        handlebars
            .render_to_write("redirect", &ctx, f)
            .with_context(|| {
                format!(
                    "Unable to create a redirect file at \"{}\"",
                    original.display()
                )
            })?;

        Ok(())
    }
}

// TODO(mattico): Remove some time after the 0.1.8 release
fn maybe_wrong_theme_dir(dir: &Path) -> Result<bool> {
    fn entry_is_maybe_book_file(entry: fs::DirEntry) -> Result<bool> {
        Ok(entry.file_type()?.is_file()
            && entry.path().extension().map_or(false, |ext| ext == "md"))
    }

    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            if entry_is_maybe_book_file(entry?).unwrap_or(false) {
                return Ok(false);
            }
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

impl Renderer for HtmlHandlebars {
    fn name(&self) -> &str {
        "html"
    }

    fn render(&self, ctx: &RenderContext) -> Result<()> {
        //dbg!("render here");
        let book_config = &ctx.config.book;
        let html_config = ctx.config.html_config().unwrap_or_default();
        let src_dir = ctx.root.join(&ctx.config.book.src);
        let destination = &ctx.destination;
        let book = &ctx.book;
        let build_dir = ctx.root.join(&ctx.config.build.build_dir);
        //dbg!(&book);
        if destination.exists() {
            utils::fs::remove_dir_content(destination)
                .with_context(|| "Unable to remove stale HTML output")?;
        }

        trace!("render");
        let mut handlebars = Handlebars::new();

        let theme_dir = match html_config.theme {
            Some(ref theme) => {
                let dir = ctx.root.join(theme);
                if !dir.is_dir() {
                    bail!("theme dir {} does not exist", dir.display());
                }
                dir
            }
            None => ctx.root.join("theme"),
        };

        if html_config.theme.is_none()
            && maybe_wrong_theme_dir(&src_dir.join("theme")).unwrap_or(false)
        {
            warn!(
                "Previous versions of mdBook erroneously accepted `./src/theme` as an automatic \
                 theme directory"
            );
            warn!("Please move your theme files to `./theme` for them to continue being used");
        }

        let theme = theme::Theme::new(theme_dir);

        debug!("Register the index handlebars template");
        handlebars.register_template_string("index", String::from_utf8(theme.index.clone())?)?;

        //dbg!("html_config",&html_config);
        //dbg!("handle-bars",&handlebars);
        //dbg!("Mdbook",&book);
        let mut data = make_data(&ctx.root, book, &ctx.config, &html_config, &theme)?;
        //dbg!(&data);
        // Print version
        let mut print_content = String::new();
        fs::create_dir_all(&destination)
            .with_context(|| "Unexpected error when constructing destination path")?;

        let mut is_index = true;
        for item in book.iter() {
            let ctx = RenderItemContext {
                handlebars: &handlebars,
                destination: destination.to_path_buf(),
                data: data.clone(),
                is_index,
                book_config: book_config.clone(),
                html_config: html_config.clone(),
                edition: ctx.config.rust.edition,
                chapter_titles: &ctx.chapter_titles,
            };
            self.render_item(item, ctx, &mut print_content)?;
            // Only the first non-draft chapter item should be treated as the "index"
            is_index &= !matches!(item, BookItem::Chapter(ch) if !ch.is_draft_chapter());
        }

        // Render 404 page
        if html_config.input_404 != Some("".to_string()) {
            self.render_404(ctx, &html_config, &src_dir, &mut handlebars, &mut data)?;
        }

        // Print version
        self.configure_print_version(&mut data, &print_content);
        if let Some(ref title) = ctx.config.book.title {
            data.insert("title".to_owned(), json!(title));
        }

        debug!("Copy static files");
        self.copy_static_files(destination)
            .with_context(|| "Unable to copy across static files")?;

        // Render search index
        #[cfg(feature = "search")]
        {
            let search = html_config.search.unwrap_or_default();
            if search.enable {
                super::search::create_files(&search, destination, book)?;
            }
        }

        self.emit_redirects(&ctx.destination, &handlebars, &html_config.redirect)
            .context("Unable to emit redirects")?;

        // Copy all remaining files, avoid a recursive copy from/to the book build dir
        utils::fs::copy_files_except_ext(&src_dir, destination, true, Some(&build_dir), &["md"])?;

        Ok(())
    }
}

fn make_data(
    _root: &Path,
    book: &Book,
    config: &Config,
    html_config: &HtmlConfig,
    _theme: &Theme,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    //dbg!(&book);
    //trace!("make_data");
    //dbg!("make data");
    let mut data = serde_json::Map::new();
    data.insert(
        "language".to_owned(),
        json!(config.book.language.clone().unwrap_or_default()),
    );
    data.insert(
        "book_title".to_owned(),
        json!(config.book.title.clone().unwrap_or_default()),
    );
    data.insert(
        "description".to_owned(),
        json!(config.book.description.clone().unwrap_or_default()),
    );

    data.insert("print_enable".to_owned(), json!(html_config.print.enable));
    data.insert("fold_enable".to_owned(), json!(html_config.fold.enable));
    data.insert("fold_level".to_owned(), json!(html_config.fold.level));

    let mut chapters = vec![];

    for item in book.iter() {
        //item
        // Create the data to inject in the template
        let mut chapter = BTreeMap::new();

        match *item {
            BookItem::PartTitle(ref title) => {
                chapter.insert("part".to_owned(), json!(title));
            }
            BookItem::Chapter(ref ch) => {
                if let Some(ref section) = ch.number {
                    chapter.insert("section".to_owned(), json!(section.to_string()));
                }

                chapter.insert(
                    "has_sub_items".to_owned(),
                    json!((!ch.sub_items.is_empty()).to_string()),
                );

                chapter.insert("name".to_owned(), json!(ch.name));
                if let Some(ref path) = ch.path {
                    let p = path
                        .to_str()
                        .with_context(|| "Could not convert path to str")?;
                    chapter.insert("path".to_owned(), json!(p));
                }
            }
            BookItem::Separator => {
                chapter.insert("spacer".to_owned(), json!("_spacer_"));
            }
        }

        chapters.push(chapter);
    }
    //dbg!(&chapters);
    data.insert("chapters".to_owned(), json!(chapters));
    //dbg!("data json");
    //dbg!(&data);
    //debug!("[*]: JSON constructed");
    Ok(data)
}

/// Goes through the rendered HTML, making sure all header tags have
/// an anchor respectively so people can link to sections directly.
/*fn build_header_links(html: &str) -> String {
    static BUILD_HEADER_LINKS: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"<h(\d)>(.*?)</h\d>").unwrap());

    let mut id_counter = HashMap::new();

    BUILD_HEADER_LINKS
        .replace_all(html, |caps: &Captures<'_>| {
            //dbg!(&caps);
            let level = caps[1]
                .parse()
                .expect("Regex should ensure we only ever get numbers here");

            insert_link_into_header(level, &caps[2], &mut id_counter)
        })
        .into_owned()
}*/
fn embed_title(html: &str, title: &String) -> String {
    static BUILD_HEADER_LINKS: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"<title>(.*?)</title>").unwrap());
    BUILD_HEADER_LINKS.replace_all(html, title).into_owned()
}
/*fn build_paragraph_with_markdown(html: &str) -> String {
    //dbg!(&html);
    static PARAGRAPH_ELEMENTS: Lazy<Regex> = Lazy::new(|| Regex::new(r#"<p>(.*?)</p>"#).unwrap());
    PARAGRAPH_ELEMENTS
        .replace_all(html, |caps: &Captures<'_>| {
            //dbg!(&caps);

            insert_markdown_into_paragraph(&caps[1])
        })
        .into_owned()
}*/
fn remove_whitespaces(html: &str) -> String {
    static PARAGRAPH_ELEMENTS: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"(?m)^ +| +$| +( )"#).unwrap());
    PARAGRAPH_ELEMENTS
        .replace_all(html.trim(), "$1")
        .into_owned()
}
/// Insert a sinle link into a header, making sure each link gets its own
/// unique ID by appending an auto-incremented number (if necessary).
/*fn insert_link_into_header(
    level: usize,
    content: &str,
    id_counter: &mut HashMap<String, usize>,
) -> String {
    //dbg!(&content);
    let id = utils::unique_id_from_content(content, id_counter);

    format!(r##"-- ds.h{level}: {id}"##, level = level, id = id,)
}
fn insert_markdown_into_paragraph(content: &str) -> String {
    //dbg!(&content);
    format!(
        r##"-- ds.markdown: 

        {text}
        "##,
        text = content
    )
}*/

// ```
// This function replaces all commas by spaces in the code block classes
fn fix_code_blocks(html: &str) -> String {
    static FIX_CODE_BLOCKS: Lazy<Regex> =
        Lazy::new(|| Regex::new(r##"<code([^>]+)class="([^"]+)"([^>]*)>"##).unwrap());

    FIX_CODE_BLOCKS
        .replace_all(html, |caps: &Captures<'_>| {
            let before = &caps[1];
            let classes = &caps[2].replace(',', " ");
            let after = &caps[3];

            format!(
                r#"<code{before}class="{classes}"{after}>"#,
                before = before,
                classes = classes,
                after = after
            )
        })
        .into_owned()
}

fn add_playground_pre(
    html: &str,
    playground_config: &Playground,
    edition: Option<RustEdition>,
) -> String {
    static ADD_PLAYGROUND_PRE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r##"((?s)<code[^>]?class="([^"]+)".*?>(.*?)</code>)"##).unwrap());

    ADD_PLAYGROUND_PRE
        .replace_all(html, |caps: &Captures<'_>| {
            let text = &caps[1];
            let classes = &caps[2];
            let code = &caps[3];

            if classes.contains("language-rust") {
                if (!classes.contains("ignore")
                    && !classes.contains("noplayground")
                    && !classes.contains("noplaypen")
                    && playground_config.runnable)
                    || classes.contains("mdbook-runnable")
                {
                    let contains_e2015 = classes.contains("edition2015");
                    let contains_e2018 = classes.contains("edition2018");
                    let contains_e2021 = classes.contains("edition2021");
                    let edition_class = if contains_e2015 || contains_e2018 || contains_e2021 {
                        // the user forced edition, we should not overwrite it
                        ""
                    } else {
                        match edition {
                            Some(RustEdition::E2015) => " edition2015",
                            Some(RustEdition::E2018) => " edition2018",
                            Some(RustEdition::E2021) => " edition2021",
                            None => "",
                        }
                    };

                    // wrap the contents in an external pre block
                    format!(
                        "<pre class=\"playground\"><code class=\"{}{}\">{}</code></pre>",
                        classes,
                        edition_class,
                        {
                            let content: Cow<'_, str> = if playground_config.editable
                                && classes.contains("editable")
                                || text.contains("fn main")
                                || text.contains("quick_main!")
                            {
                                code.into()
                            } else {
                                // we need to inject our own main
                                let (attrs, code) = partition_source(code);

                                format!("# #![allow(unused)]\n{}#fn main() {{\n{}#}}", attrs, code)
                                    .into()
                            };
                            hide_lines(&content)
                        }
                    )
                } else {
                    format!("<code class=\"{}\">{}</code>", classes, hide_lines(code))
                }
            } else {
                // not language-rust, so no-op
                text.to_owned()
            }
        })
        .into_owned()
}

fn hide_lines(content: &str) -> String {
    static BORING_LINES_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\s*)#(.?)(.*)$").unwrap());

    let mut result = String::with_capacity(content.len());
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        // Don't include newline on the last line.
        let newline = if lines.peek().is_none() { "" } else { "\n" };
        if let Some(caps) = BORING_LINES_REGEX.captures(line) {
            if &caps[2] == "#" {
                result += &caps[1];
                result += &caps[2];
                result += &caps[3];
                result += newline;
                continue;
            } else if &caps[2] != "!" && &caps[2] != "[" {
                result += "<span class=\"boring\">";
                result += &caps[1];
                if &caps[2] != " " {
                    result += &caps[2];
                }
                result += &caps[3];
                result += newline;
                result += "</span>";
                continue;
            }
        }
        result += line;
        result += newline;
    }
    result
}

fn partition_source(s: &str) -> (String, String) {
    let mut after_header = false;
    let mut before = String::new();
    let mut after = String::new();

    for line in s.lines() {
        let trimline = line.trim();
        let header = trimline.chars().all(char::is_whitespace) || trimline.starts_with("#![");
        if !header || after_header {
            after_header = true;
            after.push_str(line);
            after.push('\n');
        } else {
            before.push_str(line);
            before.push('\n');
        }
    }

    (before, after)
}

struct RenderItemContext<'a> {
    handlebars: &'a Handlebars<'a>,
    destination: PathBuf,
    data: serde_json::Map<String, serde_json::Value>,
    is_index: bool,
    book_config: BookConfig,
    html_config: HtmlConfig,
    edition: Option<RustEdition>,
    chapter_titles: &'a HashMap<PathBuf, String>,
}
