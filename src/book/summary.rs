use crate::errors::*;
use log::{debug, trace, warn};
use memchr::{self, Memchr};
use pulldown_cmark::{self, Event, HeadingLevel, Tag};
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};
use std::iter::FromIterator;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

/// All other elements are unsupported and will be ignored at best or result in
/// an error.
pub fn parse_summary(summary: &str) -> Result<Summary> {
    let parser = SummaryParser::new(summary);
    parser.parse()
}

/// The parsed `SUMMARY.md`, specifying how the book should be laid out.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// An optional title for the `SUMMARY.md`, currently just ignored.
    pub title: Option<String>,
    /// Chapters before the main text (e.g. an introduction).
    pub prefix_chapters: Vec<SummaryItem>,
    /// The main numbered chapters of the book, broken into one or more possibly named parts.
    pub numbered_chapters: Vec<SummaryItem>,
    /// Items which come after the main document (e.g. a conclusion).
    pub suffix_chapters: Vec<SummaryItem>,
}

/// A struct representing an entry in the `SUMMARY.md`, possibly with nested
/// entries.
///
/// This is roughly the equivalent of `[Some section](./path/to/file.md)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// The name of the chapter.
    pub name: String,
    /// The location of the chapter's source file, taking the book's `src`
    /// directory as the root.
    pub location: Option<PathBuf>,
    /// The section number, if this chapter is in the numbered section.
    pub number: Option<SectionNumber>,
    /// Any nested items this chapter may contain.
    pub nested_items: Vec<SummaryItem>,
}

impl Link {
    /// Create a new link with no nested items.
    pub fn new<S: Into<String>, P: AsRef<Path>>(name: S, location: P) -> Link {
        Link {
            name: name.into(),
            location: Some(location.as_ref().to_path_buf()),
            number: None,
            nested_items: Vec::new(),
        }
    }
}

impl Default for Link {
    fn default() -> Self {
        Link {
            name: String::new(),
            location: Some(PathBuf::new()),
            number: None,
            nested_items: Vec::new(),
        }
    }
}

/// An item in `SUMMARY.md` which could be either a separator or a `Link`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SummaryItem {
    /// A link to a chapter.
    Link(Link),
    /// A separator (`---`).
    Separator,
    /// A part title.
    PartTitle(String),
}

impl SummaryItem {
    fn maybe_link_mut(&mut self) -> Option<&mut Link> {
        match *self {
            SummaryItem::Link(ref mut l) => Some(l),
            _ => None,
        }
    }
}

impl From<Link> for SummaryItem {
    fn from(other: Link) -> SummaryItem {
        SummaryItem::Link(other)
    }
}

struct SummaryParser<'a> {
    src: &'a str,
    stream: pulldown_cmark::OffsetIter<'a, 'a>,
    offset: usize,

    back: Option<Event<'a>>,
}

macro_rules! collect_events {
    ($stream:expr,start $delimiter:pat) => {
        collect_events!($stream, Event::Start($delimiter))
    };
    ($stream:expr,end $delimiter:pat) => {
        collect_events!($stream, Event::End($delimiter))
    };
    ($stream:expr, $delimiter:pat) => {{
        let mut events = Vec::new();

        loop {
            let event = $stream.next().map(|(ev, _range)| ev);
            trace!("Next event: {:?}", event);

            match event {
                Some($delimiter) => break,
                Some(other) => events.push(other),
                None => {
                    debug!(
                        "Reached end of stream without finding the closing pattern, {}",
                        stringify!($delimiter)
                    );
                    break;
                }
            }
        }

        events
    }};
}

impl<'a> SummaryParser<'a> {
    fn new(text: &str) -> SummaryParser<'_> {
        let pulldown_parser = pulldown_cmark::Parser::new(text).into_offset_iter();

        SummaryParser {
            src: text,
            stream: pulldown_parser,
            offset: 0,
            back: None,
        }
    }

    /// Get the current line and column to give the user more useful error
    /// messages.
    fn current_location(&self) -> (usize, usize) {
        let previous_text = self.src[..self.offset].as_bytes();
        let line = Memchr::new(b'\n', previous_text).count() + 1;
        let start_of_line = memchr::memrchr(b'\n', previous_text).unwrap_or(0);
        let col = self.src[start_of_line..self.offset].chars().count();

        (line, col)
    }

    /// Parse the text the `SummaryParser` was created with.
    fn parse(mut self) -> Result<Summary> {
        let title = self.parse_title();

        let prefix_chapters = self
            .parse_affix(true)
            .with_context(|| "There was an error parsing the prefix chapters")?;
        let numbered_chapters = self
            .parse_parts()
            .with_context(|| "There was an error parsing the numbered chapters")?;
        let suffix_chapters = self
            .parse_affix(false)
            .with_context(|| "There was an error parsing the suffix chapters")?;

        Ok(Summary {
            title,
            prefix_chapters,
            numbered_chapters,
            suffix_chapters,
        })
    }

    /// Parse the affix chapters.
    fn parse_affix(&mut self, is_prefix: bool) -> Result<Vec<SummaryItem>> {
        let mut items = Vec::new();
        debug!(
            "Parsing {} items",
            if is_prefix { "prefix" } else { "suffix" }
        );

        loop {
            match self.next_event() {
                Some(ev @ Event::Start(Tag::List(..)))
                | Some(ev @ Event::Start(Tag::Heading(HeadingLevel::H1, ..))) => {
                    if is_prefix {
                        // we've finished prefix chapters and are at the start
                        // of the numbered section.
                        self.back(ev);
                        break;
                    } else {
                        bail!(self.parse_error("Suffix chapters cannot be followed by a list"));
                    }
                }
                Some(Event::Start(Tag::Link(_type, href, _title))) => {
                    let link = self.parse_link(href.to_string());
                    items.push(SummaryItem::Link(link));
                }
                Some(Event::Rule) => items.push(SummaryItem::Separator),
                Some(_) => {}
                None => break,
            }
        }

        Ok(items)
    }

    fn parse_parts(&mut self) -> Result<Vec<SummaryItem>> {
        let mut parts = vec![];

        // We want the section numbers to be continues through all parts.
        let mut root_number = SectionNumber::default();
        let mut root_items = 0;

        loop {
            // Possibly match a title or the end of the "numbered chapters part".
            let title = match self.next_event() {
                Some(ev @ Event::Start(Tag::Paragraph)) => {
                    // we're starting the suffix chapters
                    self.back(ev);
                    break;
                }

                Some(Event::Start(Tag::Heading(HeadingLevel::H1, ..))) => {
                    debug!("Found a h1 in the SUMMARY");

                    let tags = collect_events!(self.stream, end Tag::Heading(HeadingLevel::H1, ..));
                    Some(stringify_events(tags))
                }

                Some(ev) => {
                    self.back(ev);
                    None
                }

                None => break, // EOF, bail...
            };

            // Parse the rest of the part.
            let numbered_chapters = self
                .parse_numbered(&mut root_items, &mut root_number)
                .with_context(|| "There was an error parsing the numbered chapters")?;

            if let Some(title) = title {
                parts.push(SummaryItem::PartTitle(title));
            }
            parts.extend(numbered_chapters);
        }

        Ok(parts)
    }

    /// Finishes parsing a link once the `Event::Start(Tag::Link(..))` has been opened.
    fn parse_link(&mut self, href: String) -> Link {
        let href = href.replace("%20", " ");
        let link_content = collect_events!(self.stream, end Tag::Link(..));
        let name = stringify_events(link_content);

        let path = if href.is_empty() {
            None
        } else {
            Some(PathBuf::from(href))
        };

        Link {
            name,
            location: path,
            number: None,
            nested_items: Vec::new(),
        }
    }

    /// Parse the numbered chapters.
    fn parse_numbered(
        &mut self,
        root_items: &mut u32,
        root_number: &mut SectionNumber,
    ) -> Result<Vec<SummaryItem>> {
        let mut items = Vec::new();

        // For the first iteration, we want to just skip any opening paragraph tags, as that just
        // marks the start of the list. But after that, another opening paragraph indicates that we
        // have started a new part or the suffix chapters.
        let mut first = true;

        loop {
            match self.next_event() {
                Some(ev @ Event::Start(Tag::Paragraph)) => {
                    if !first {
                        // we're starting the suffix chapters
                        self.back(ev);
                        break;
                    }
                }
                // The expectation is that pulldown cmark will terminate a paragraph before a new
                // heading, so we can always count on this to return without skipping headings.
                Some(ev @ Event::Start(Tag::Heading(HeadingLevel::H1, ..))) => {
                    // we're starting a new part
                    self.back(ev);
                    break;
                }
                Some(ev @ Event::Start(Tag::List(..))) => {
                    self.back(ev);
                    let mut bunch_of_items = self.parse_nested_numbered(root_number)?;

                    // if we've resumed after something like a rule the root sections
                    // will be numbered from 1. We need to manually go back and update
                    // them
                    update_section_numbers(&mut bunch_of_items, 0, *root_items);
                    *root_items += bunch_of_items.len() as u32;
                    items.extend(bunch_of_items);
                }
                Some(Event::Start(other_tag)) => {
                    trace!("Skipping contents of {:?}", other_tag);

                    // Skip over the contents of this tag
                    while let Some(event) = self.next_event() {
                        if event == Event::End(other_tag.clone()) {
                            break;
                        }
                    }
                }
                Some(Event::Rule) => {
                    items.push(SummaryItem::Separator);
                }

                // something else... ignore
                Some(_) => {}

                // EOF, bail...
                None => {
                    break;
                }
            }

            // From now on, we cannot accept any new paragraph opening tags.
            first = false;
        }

        Ok(items)
    }

    /// Push an event back to the tail of the stream.
    fn back(&mut self, ev: Event<'a>) {
        assert!(self.back.is_none());
        trace!("Back: {:?}", ev);
        self.back = Some(ev);
    }

    fn next_event(&mut self) -> Option<Event<'a>> {
        let next = self.back.take().or_else(|| {
            self.stream.next().map(|(ev, range)| {
                self.offset = range.start;
                ev
            })
        });

        trace!("Next event: {:?}", next);

        next
    }

    fn parse_nested_numbered(&mut self, parent: &SectionNumber) -> Result<Vec<SummaryItem>> {
        debug!("Parsing numbered chapters at level {}", parent);
        let mut items = Vec::new();

        loop {
            match self.next_event() {
                Some(Event::Start(Tag::Item)) => {
                    let item = self.parse_nested_item(parent, items.len())?;
                    items.push(item);
                }
                Some(Event::Start(Tag::List(..))) => {
                    // Skip this tag after comment because it is not nested.
                    if items.is_empty() {
                        continue;
                    }
                    // recurse to parse the nested list
                    let (_, last_item) = get_last_link(&mut items)?;
                    let last_item_number = last_item
                        .number
                        .as_ref()
                        .expect("All numbered chapters have numbers");

                    let sub_items = self.parse_nested_numbered(last_item_number)?;

                    last_item.nested_items = sub_items;
                }
                Some(Event::End(Tag::List(..))) => break,
                Some(_) => {}
                None => break,
            }
        }

        Ok(items)
    }

    fn parse_nested_item(
        &mut self,
        parent: &SectionNumber,
        num_existing_items: usize,
    ) -> Result<SummaryItem> {
        loop {
            match self.next_event() {
                Some(Event::Start(Tag::Paragraph)) => continue,
                Some(Event::Start(Tag::Link(_type, href, _title))) => {
                    let mut link = self.parse_link(href.to_string());

                    let mut number = parent.clone();
                    number.0.push(num_existing_items as u32 + 1);
                    trace!(
                        "Found chapter: {} {} ({})",
                        number,
                        link.name,
                        link.location
                            .as_ref()
                            .map(|p| p.to_str().unwrap_or(""))
                            .unwrap_or("[draft]")
                    );

                    link.number = Some(number);

                    return Ok(SummaryItem::Link(link));
                }
                other => {
                    warn!("Expected a start of a link, actually got {:?}", other);
                    bail!(self.parse_error(
                        "The link items for nested chapters must only contain a hyperlink"
                    ));
                }
            }
        }
    }

    fn parse_error<D: Display>(&self, msg: D) -> Error {
        let (line, col) = self.current_location();
        anyhow::anyhow!(
            "failed to parse SUMMARY.md line {}, column {}: {}",
            line,
            col,
            msg
        )
    }

    /// Try to parse the title line.
    fn parse_title(&mut self) -> Option<String> {
        loop {
            match self.next_event() {
                Some(Event::Start(Tag::Heading(HeadingLevel::H1, ..))) => {
                    debug!("Found a h1 in the SUMMARY");

                    let tags = collect_events!(self.stream, end Tag::Heading(HeadingLevel::H1, ..));
                    return Some(stringify_events(tags));
                }
                // Skip a HTML element such as a comment line.
                Some(Event::Html(_)) => {}
                // Otherwise, no title.
                Some(ev) => {
                    self.back(ev);
                    return None;
                }
                _ => return None,
            }
        }
    }
}

fn update_section_numbers(sections: &mut [SummaryItem], level: usize, by: u32) {
    for section in sections {
        if let SummaryItem::Link(ref mut link) = *section {
            if let Some(ref mut number) = link.number {
                number.0[level] += by;
            }

            update_section_numbers(&mut link.nested_items, level, by);
        }
    }
}

/// Gets a pointer to the last `Link` in a list of `SummaryItem`s, and its
/// index.
fn get_last_link(links: &mut [SummaryItem]) -> Result<(usize, &mut Link)> {
    links
        .iter_mut()
        .enumerate()
        .filter_map(|(i, item)| item.maybe_link_mut().map(|l| (i, l)))
        .rev()
        .next()
        .ok_or_else(||
            anyhow::anyhow!("Unable to get last link because the list of SummaryItems doesn't contain any Links")
            )
}

/// Removes the styling from a list of Markdown events and returns just the
/// plain text.
fn stringify_events(events: Vec<Event<'_>>) -> String {
    events
        .into_iter()
        .filter_map(|t| match t {
            Event::Text(text) | Event::Code(text) => Some(text.into_string()),
            Event::SoftBreak => Some(String::from(" ")),
            _ => None,
        })
        .collect()
}

/// A section number like "1.2.3", basically just a newtype'd `Vec<u32>` with
/// a pretty `Display` impl.
#[derive(Debug, Eq, PartialEq, Clone, Default, Serialize, Deserialize)]
pub struct SectionNumber(pub Vec<u32>);

impl Display for SectionNumber {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            write!(f, "0")
        } else {
            for item in &self.0 {
                write!(f, "{}.", item)?;
            }
            Ok(())
        }
    }
}

impl Deref for SectionNumber {
    type Target = Vec<u32>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SectionNumber {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl FromIterator<u32> for SectionNumber {
    fn from_iter<I: IntoIterator<Item = u32>>(it: I) -> Self {
        SectionNumber(it.into_iter().collect())
    }
}
