#![allow(missing_docs)] // FIXME: Document this

pub mod fs;
mod string;
pub(crate) mod toml_ext;
use crate::errors::Error;
use log::error;
use once_cell::sync::Lazy;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, LinkType, Options, Parser, Tag};
use regex::Regex;

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;
pub(crate) enum MarkDownEvents {
    Heading,
    Paragraph,
    Link,
}
impl MarkDownEvents {
    /*const EVENTS_ITER: [MarkDownEvents; 3] = [
        MarkDownEvents::Heading,
        MarkDownEvents::Paragraph,
        MarkDownEvents::Link,
    ];*/
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            MarkDownEvents::Heading => "heading",
            MarkDownEvents::Paragraph => "paragraph",
            MarkDownEvents::Link => "link",
        }
    }

    /*pub(crate) fn from_str(s: &str) -> Self {
        match s {
            "heading" => MarkDownEvents::Heading,
            "paragraph" => MarkDownEvents::Paragraph,
            "link" => MarkDownEvents::Link,
            _ => panic!("Invalid Markdown Events {}", s),
        }
    }*/
}

pub use self::string::{
    take_anchored_lines, take_lines, take_rustdoc_include_anchored_lines,
    take_rustdoc_include_lines,
};

/// Replaces multiple consecutive whitespace characters with a single space character.
pub fn collapse_whitespace(text: &str) -> Cow<'_, str> {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s\s+").unwrap());
    RE.replace_all(text, " ")
}

/// Convert the given string to a valid HTML element ID.
/// The only restriction is that the ID must not contain any ASCII whitespace.
pub fn normalize_id(content: &str) -> String {
    content
        .chars()
        .filter_map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                Some(ch.to_ascii_lowercase())
            } else if ch.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>()
}

/// Generate an ID for use with anchors which is derived from a "normalised"
/// string.
// This function should be made private when the deprecation expires.
#[deprecated(since = "0.4.16", note = "use unique_id_from_content instead")]
pub fn id_from_content(content: &str) -> String {
    let mut content = content.to_string();

    // Skip any tags or html-encoded stuff
    static HTML: Lazy<Regex> = Lazy::new(|| Regex::new(r"(<.*?>)").unwrap());
    content = HTML.replace_all(&content, "").into();
    const REPL_SUB: &[&str] = &["&lt;", "&gt;", "&amp;", "&#39;", "&quot;"];
    for sub in REPL_SUB {
        content = content.replace(sub, "");
    }

    // Remove spaces and hashes indicating a header
    let trimmed = content.trim().trim_start_matches('#').trim();
    normalize_id(trimmed)
}

/// Generate an ID for use with anchors which is derived from a "normalised"
/// string.
///
/// Each ID returned will be unique, if the same `id_counter` is provided on
/// each call.
pub fn unique_id_from_content(content: &str, id_counter: &mut HashMap<String, usize>) -> String {
    let id = {
        #[allow(deprecated)]
        id_from_content(content)
    };

    // If we have headers with the same normalized id, append an incrementing counter
    let id_count = id_counter.entry(id.clone()).or_insert(0);
    let unique_id = match *id_count {
        0 => id,
        id_count => format!("{}-{}", id, id_count),
    };
    *id_count += 1;
    unique_id
}

/// Fix links to the correct location.
///
/// This adjusts links, such as turning `.md` extensions to `.html`.
///
/// `path` is the path to the page being rendered relative to the root of the
/// book. This is used for the `print.html` page so that links on the print
/// page go to the original location. Normal page rendering sets `path` to
/// None. Ideally, print page links would link to anchors on the print page,
/// but that is very difficult.
fn adjust_links<'a>(event: Event<'a>, path: Option<&Path>) -> Event<'a> {
    static SCHEME_LINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-z][a-z0-9+.-]*:").unwrap());
    static MD_LINK: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?P<link>.*)\.md(?P<anchor>#.*)?").unwrap());

    fn fix<'a>(dest: CowStr<'a>, path: Option<&Path>) -> CowStr<'a> {
        if dest.starts_with('#') {
            // Fragment-only link.
            if let Some(path) = path {
                let mut base = path.display().to_string();
                if base.ends_with(".md") {
                    base.replace_range(base.len() - 3.., ".ftd");
                }
                return format!("/{}/{}", base, dest).into();
            } else {
                return dest;
            }
        }
        // Don't modify links with schemes like `https`.
        if !SCHEME_LINK.is_match(&dest) {
            // This is a relative link, adjust it as necessary.
            let mut fixed_link = String::new();
            if let Some(path) = path {
                //dbg!("came here1");
                let base = path
                    .parent()
                    .expect("path can't be empty")
                    .to_str()
                    .expect("utf-8 paths only");
                if !base.is_empty() {
                    write!(fixed_link, "{}/", base).unwrap();
                }
            }

            if let Some(caps) = MD_LINK.captures(&dest) {
                //dbg!(&caps);
                fixed_link.push_str(&caps["link"]);
                fixed_link.push_str(".ftd");
                if let Some(anchor) = caps.name("anchor") {
                    fixed_link.push_str(anchor.as_str());
                }
            } else {
                fixed_link.push_str(&dest);
            };
            //dbg!(&fixed_link);
            return CowStr::from(fixed_link);
        }

        dest
    }

    fn fix_html<'a>(html: CowStr<'a>, path: Option<&Path>) -> CowStr<'a> {
        // This is a terrible hack, but should be reasonably reliable. Nobody
        // should ever parse a tag with a regex. However, there isn't anything
        // in Rust that I know of that is suitable for handling partial html
        // fragments like those generated by pulldown_cmark.
        //
        // There are dozens of HTML tags/attributes that contain paths, so
        // feel free to add more tags if desired; these are the only ones I
        // care about right now.
        static HTML_LINK: Lazy<Regex> =
            Lazy::new(|| Regex::new(r#"(<(?:a|img) [^>]*?(?:src|href)=")([^"]+?)""#).unwrap());

        HTML_LINK
            .replace_all(&html, |caps: &regex::Captures<'_>| {
                let fixed = fix(caps[2].into(), path);
                format!("{}{}\"", &caps[1], fixed)
            })
            .into_owned()
            .into()
    }

    match event {
        Event::Start(Tag::Link(link_type, dest, title)) => {
            Event::Start(Tag::Link(link_type, fix(dest, path), title))
        }
        Event::Start(Tag::Image(link_type, dest, title)) => {
            Event::Start(Tag::Image(link_type, fix(dest, path), title))
        }
        Event::Html(html) => Event::Html(fix_html(html, path)),
        _ => event,
    }
}

/// Wrapper around the pulldown-cmark parser for rendering markdown to HTML.
pub fn render_markdown(text: &str, curly_quotes: bool) -> String {
    render_markdown_with_path(text, curly_quotes, None)
}

pub fn new_cmark_parser(text: &str, curly_quotes: bool) -> Parser<'_, '_> {
    //dbg!(&text);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    if curly_quotes {
        opts.insert(Options::ENABLE_SMART_PUNCTUATION);
    }
    //dbg!(&opts);
    Parser::new_ext(text, opts)
}

pub fn render_markdown_with_path(text: &str, curly_quotes: bool, path: Option<&Path>) -> String {
    let mut rendered_docsite = String::with_capacity(text.len() * 3 / 2);
    let p = new_cmark_parser(text, curly_quotes);
    let mut parsed_str: String;
    let mut current_tag: String = String::from("");
    let mut tag_started: bool = false;
    /*for obj in p{
        dbg!(obj);
    }*/
    let events = p
        .map(clean_codeblock_headers)
        .map(|event| adjust_links(event, path))
        .flat_map(|event| {
            let (a, b) = wrap_tables(event);

            a.into_iter().chain(b)
        });
    let mut tag_parsed_string = "".to_string();
    for event in events {
        (parsed_str, current_tag, tag_started) = render_to_docsite(event, current_tag, tag_started);

        if !tag_started {
            if current_tag == MarkDownEvents::Link.as_str() {
                tag_parsed_string = format!("{}{}\n", parsed_str, tag_parsed_string);
                rendered_docsite = format!("{}{}", rendered_docsite, tag_parsed_string);
            } else if current_tag == MarkDownEvents::Heading.as_str() {
                tag_parsed_string = format!("{}{}\n", tag_parsed_string, parsed_str);
                rendered_docsite = format!("{}{}", rendered_docsite, tag_parsed_string);
            }else {
                tag_parsed_string = format!("{}\n{}", tag_parsed_string, parsed_str);
                rendered_docsite = format!("{}{}", rendered_docsite, tag_parsed_string);
            }
            tag_parsed_string = "".to_string();
        } else {
            dbg!("tag closed");
            dbg!(&current_tag);
            tag_parsed_string = parsed_str;
        }
    }

    rendered_docsite
}
pub fn render_to_docsite(
    event: Event,
    mut current_tag: String,
    mut tag_started: bool,
) -> (String, String, bool) {
    let mut result_str = String::from("");
    //let mut tag_type = String::from("heading");
    dbg!(&event);
    match &event {
        Event::Start(tag) => match tag {
            Tag::Heading(heading_level, _fragment_identifier, _class_list) => {
                tag_started = true;
                current_tag = MarkDownEvents::Heading.as_str().to_string();
                result_str = format!(
                    r##"-- ds.{heading_level}: "##
                );
            }
            Tag::Paragraph => {
                tag_started = true;
                current_tag = MarkDownEvents::Paragraph.as_str().to_string();
                dbg!("in paragraph");
                result_str = r##"-- ds.markdown: "##.to_string();
            }
            Tag::Link(link_type, url, _title) => {
                tag_started = true;
                if *link_type == LinkType::Inline {
                    current_tag = MarkDownEvents::Link.as_str().to_string();
                    let parsed_url = url.to_string().replace(".ftd", "");
                    result_str = format!(r##"(/{parsed_url}/)"##);
                }
            }
            Tag::List(ordered_list_first_item_number) => {
                tag_started = true;
                println!(
                    "List ordered_list_first_item_number: {:?}",
                    ordered_list_first_item_number
                )
            }
            Tag::Item => {
                tag_started = true;
                println!("Item (this is a list item)")
            }
            Tag::Emphasis => {
                tag_started = true;
                println!("Emphasis (this is a span tag)")
            }
            Tag::Strong => {
                tag_started = true;
                println!("Strong (this is a span tag)")
            }
            Tag::Strikethrough => {
                tag_started = true;
                println!("Strikethrough (this is a span tag)")
            }
            Tag::BlockQuote => {
                tag_started = true;
                println!("BlockQuote")
            }
            Tag::CodeBlock(code_block_kind) => {
                tag_started = true;
                println!("CodeBlock code_block_kind: {:?}", code_block_kind)
            }
            Tag::Image(_link_type, url, _title) => {
                tag_started = true;
                let image_url = url.replace('/', ".");
                result_str = format!(
                    r##"-- ds.image: 
                src: $assets.files{image_url}
                align: center"##
                );
            }
            Tag::Table(column_text_alignment_list) => {
                tag_started = true;
                println!(
                    "Table column_text_alignment_list: {:?}",
                    column_text_alignment_list
                )
            }
            Tag::TableHead => {
                tag_started = true;
                println!("TableHead (contains TableRow tags")
            }
            Tag::TableRow => {
                tag_started = true;
                println!("TableRow (contains TableCell tags)")
            }
            Tag::TableCell => {
                tag_started = true;
                println!("TableCell (contains inline tags)")
            }
            Tag::FootnoteDefinition(label) => {
                tag_started = true;
                println!("FootnoteDefinition label: {}", label)
            }
        },
        Event::Text(s) => {
            tag_started = false;
            if current_tag == *MarkDownEvents::Heading.as_str().to_string() {
                result_str = format!(
                    r##" {s}"##,
                );
            } else if current_tag == *MarkDownEvents::Link.as_str().to_string() {
                result_str = format!(r##"[{s}]"##,);
            } else if current_tag == *MarkDownEvents::Paragraph.as_str().to_string() {
                result_str = format!(
                    r##"{s}"##,
                );
            } else {
                result_str = "".to_string();
            }

            //println!("Text: {:?}", s.trim())
        }
        Event::SoftBreak => println!("SoftBreak"),
        Event::HardBreak => println!("HardBreak"),
        Event::End(tag) => {
            tag_started = false;
            println!("End: {:?}", tag)
        }
        Event::Code(s) => {
            println!("Code: {:?}", s)
        }
        /*Event::Html(s) => println!("Html: {:?}", s),
        Event::Text(s) => println!("Text: {:?}", s),

        Event::FootnoteReference(s) => println!("FootnoteReference: {:?}", s),
        Event::TaskListMarker(b) => println!("TaskListMarker: {:?}", b),

        Event::Rule => println!("Rule"),*/
        _ => {}
    }
    //String::from("yes")
    (result_str, current_tag, tag_started)
}
/// Wraps tables in a `.table-wrapper` class to apply overflow-x rules to.
fn wrap_tables(event: Event<'_>) -> (Option<Event<'_>>, Option<Event<'_>>) {
    match event {
        Event::Start(Tag::Table(_)) => (
            Some(Event::Html(r#"<div class="table-wrapper">"#.into())),
            Some(event),
        ),
        Event::End(Tag::Table(_)) => (Some(event), Some(Event::Html(r#"</div>"#.into()))),
        _ => (Some(event), None),
    }
}

fn clean_codeblock_headers(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(ref info))) => {
            let info: String = info
                .chars()
                .map(|x| match x {
                    ' ' | '\t' => ',',
                    _ => x,
                })
                .filter(|ch| !ch.is_whitespace())
                .collect();

            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(CowStr::from(info))))
        }
        _ => event,
    }
}

/// Prints a "backtrace" of some `Error`.
pub fn log_backtrace(e: &Error) {
    error!("Error: {}", e);

    for cause in e.chain().skip(1) {
        error!("\tCaused By: {}", cause);
    }
}

pub(crate) fn bracket_escape(mut s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    let needs_escape: &[char] = &['<', '>'];
    while let Some(next) = s.find(needs_escape) {
        escaped.push_str(&s[..next]);
        match s.as_bytes()[next] {
            b'<' => escaped.push_str("&lt;"),
            b'>' => escaped.push_str("&gt;"),
            _ => unreachable!(),
        }
        s = &s[next + 1..];
    }
    escaped.push_str(s);
    escaped
}
