use std::collections::VecDeque;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::summary::{parse_summary, Link, SectionNumber, Summary, SummaryItem};
use crate::config::BuildConfig;
use crate::errors::*;
use crate::utils::bracket_escape;
use log::debug;
use serde::{Deserialize, Serialize};

/// Load a book into memory from its `src/` directory.
pub fn load_book<P: AsRef<Path>>(src_dir: P, cfg: &BuildConfig) -> Result<Book> {
    let src_dir = src_dir.as_ref();
    let summary_md = src_dir.join("SUMMARY.md");

    let mut summary_content = String::new();
    File::open(&summary_md)
        .with_context(|| format!("Couldn't open SUMMARY.md in {:?} directory", src_dir))?
        .read_to_string(&mut summary_content)?;
    //dbg!(&summary_content);

    let summary = parse_summary(&summary_content)
        .with_context(|| format!("Summary parsing failed for file={:?}", summary_md))?;
    //dbg!(&summary);
    if cfg.create_missing {
        create_missing(src_dir, &summary).with_context(|| "Unable to create missing chapters")?;
    }
    //create_fpm_ftd(&summary_content,&src_dir).with_context(|| "Unable to copy across static files")?;
    load_book_from_disk(&summary, src_dir)
}
/*fn create_fpm_ftd(summary_content:&String,src_dir: &Path) -> Result<()> {
    //dbg!(&src_dir);
    //dbg!(&summary_content);
    let fpm_ftd_string=format!("{}{}",String::from("-- import: fpm

    -- fpm.package: wasif1024.github.io/fpm-site
    download-base-url: https://raw.githubusercontent.com/wasif1024/fpm-site/main

    -- fpm.dependency: fifthtry.github.io/doc-site as ds

    -- fpm.auto-import: ds

    -- fpm.sitemap:
    "),&summary_content);

    use crate::utils::fs::write_file;

        write_file(
            src_dir,
            "FPM.ftd",
            fpm_ftd_string.as_bytes(),
        )?;
        Ok(())
}*/
fn create_missing(src_dir: &Path, summary: &Summary) -> Result<()> {
    let mut items: Vec<_> = summary
        .prefix_chapters
        .iter()
        .chain(summary.numbered_chapters.iter())
        .chain(summary.suffix_chapters.iter())
        .collect();

    while !items.is_empty() {
        let next = items.pop().expect("already checked");

        if let SummaryItem::Link(ref link) = *next {
            if let Some(ref location) = link.location {
                let filename = src_dir.join(location);
                if !filename.exists() {
                    if let Some(parent) = filename.parent() {
                        if !parent.exists() {
                            fs::create_dir_all(parent)?;
                        }
                    }
                    debug!("Creating missing file {}", filename.display());

                    let mut f = File::create(&filename).with_context(|| {
                        format!("Unable to create missing file: {}", filename.display())
                    })?;
                    writeln!(f, "# {}", bracket_escape(&link.name))?;
                }
            }

            items.extend(&link.nested_items);
        }
    }

    Ok(())
}

/// A dumb tree structure representing a book.
///
/// For the moment a book is just a collection of [`BookItems`] which are
/// accessible by either iterating (immutably) over the book with [`iter()`], or
/// recursively applying a closure to each section to mutate the chapters, using
/// [`for_each_mut()`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Book {
    /// The sections in this book.
    pub sections: Vec<BookItem>,
}

impl Book {
    /// Create an empty book.
    pub fn new() -> Self {
        Default::default()
    }

    /// Get a depth-first iterator over the items in the book.
    pub fn iter(&self) -> BookItems<'_> {
        BookItems {
            items: self.sections.iter().collect(),
        }
    }

    /// Recursively apply a closure to each item in the book, allowing you to
    /// mutate them.
    ///
    /// # Note
    ///
    /// Unlike the `iter()` method, this requires a closure instead of returning
    /// an iterator. This is because using iterators can possibly allow you
    /// to have iterator invalidation errors.
    pub fn for_each_mut<F>(&mut self, mut func: F)
    where
        F: FnMut(&mut BookItem),
    {
        for_each_mut(&mut func, &mut self.sections);
    }

    /// Append a `BookItem` to the `Book`.
    pub fn push_item<I: Into<BookItem>>(&mut self, item: I) -> &mut Self {
        self.sections.push(item.into());
        self
    }
}

pub fn for_each_mut<'a, F, I>(func: &mut F, items: I)
where
    F: FnMut(&mut BookItem),
    I: IntoIterator<Item = &'a mut BookItem>,
{
    for item in items {
        if let BookItem::Chapter(ch) = item {
            for_each_mut(func, &mut ch.sub_items);
        }

        func(item);
    }
}

/// Enum representing any type of item which can be added to a book.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BookItem {
    /// A nested chapter.
    Chapter(Chapter),
    /// A section separator.
    Separator,
    /// A part title.
    PartTitle(String),
}

impl From<Chapter> for BookItem {
    fn from(other: Chapter) -> BookItem {
        BookItem::Chapter(other)
    }
}

/// The representation of a "chapter", usually mapping to a single file on
/// disk however it may contain multiple sub-chapters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    /// The chapter's name.
    pub name: String,
    /// The chapter's contents.
    pub content: String,
    /// The chapter's section number, if it has one.
    pub number: Option<SectionNumber>,
    /// Nested items.
    pub sub_items: Vec<BookItem>,
    /// The chapter's location, relative to the `SUMMARY.md` file.
    pub path: Option<PathBuf>,
    /// The chapter's source file, relative to the `SUMMARY.md` file.
    pub source_path: Option<PathBuf>,
    /// An ordered list of the names of each chapter above this one in the hierarchy.
    pub parent_names: Vec<String>,
}

impl Chapter {
    /// Create a new chapter with the provided content.
    pub fn new<P: Into<PathBuf>>(
        name: &str,
        content: String,
        p: P,
        parent_names: Vec<String>,
    ) -> Chapter {
        let path: PathBuf = p.into();
        Chapter {
            name: name.to_string(),
            content,
            path: Some(path.clone()),
            source_path: Some(path),
            parent_names,
            ..Default::default()
        }
    }

    /// Create a new draft chapter that is not attached to a source markdown file (and thus
    /// has no content).
    pub fn new_draft(name: &str, parent_names: Vec<String>) -> Self {
        Chapter {
            name: name.to_string(),
            content: String::new(),
            path: None,
            source_path: None,
            parent_names,
            ..Default::default()
        }
    }

    /// Check if the chapter is a draft chapter, meaning it has no path to a source markdown file.
    pub fn is_draft_chapter(&self) -> bool {
        self.path.is_none()
    }
}

/// Use the provided `Summary` to load a `Book` from disk.
///
/// You need to pass in the book's source directory because all the links in
/// `SUMMARY.md` give the chapter locations relative to it.
pub(crate) fn load_book_from_disk<P: AsRef<Path>>(summary: &Summary, src_dir: P) -> Result<Book> {
    debug!("Loading the book from disk");
    let src_dir = src_dir.as_ref();

    let prefix = summary.prefix_chapters.iter();
    let numbered = summary.numbered_chapters.iter();
    let suffix = summary.suffix_chapters.iter();

    let summary_items = prefix.chain(numbered).chain(suffix);

    let mut chapters = Vec::new();

    for summary_item in summary_items {
        let chapter = load_summary_item(summary_item, src_dir, Vec::new())?;
        chapters.push(chapter);
    }

    Ok(Book { sections: chapters })
}

fn load_summary_item<P: AsRef<Path> + Clone>(
    item: &SummaryItem,
    src_dir: P,
    parent_names: Vec<String>,
) -> Result<BookItem> {
    match item {
        SummaryItem::Separator => Ok(BookItem::Separator),
        SummaryItem::Link(ref link) => {
            load_chapter(link, src_dir, parent_names).map(BookItem::Chapter)
        }
        SummaryItem::PartTitle(title) => Ok(BookItem::PartTitle(title.clone())),
    }
}

fn load_chapter<P: AsRef<Path>>(
    link: &Link,
    src_dir: P,
    parent_names: Vec<String>,
) -> Result<Chapter> {
    let src_dir = src_dir.as_ref();

    let mut ch = if let Some(ref link_location) = link.location {
        debug!("Loading {} ({})", link.name, link_location.display());

        let location = if link_location.is_absolute() {
            link_location.clone()
        } else {
            src_dir.join(link_location)
        };

        let mut f = File::open(&location)
            .with_context(|| format!("Chapter file not found, {}", link_location.display()))?;

        let mut content = String::new();
        f.read_to_string(&mut content).with_context(|| {
            format!("Unable to read \"{}\" ({})", link.name, location.display())
        })?;

        if content.as_bytes().starts_with(b"\xef\xbb\xbf") {
            content.replace_range(..3, "");
        }

        let stripped = location
            .strip_prefix(&src_dir)
            .expect("Chapters are always inside a book");

        Chapter::new(&link.name, content, stripped, parent_names.clone())
    } else {
        Chapter::new_draft(&link.name, parent_names.clone())
    };

    let mut sub_item_parents = parent_names;

    ch.number = link.number.clone();

    sub_item_parents.push(link.name.clone());
    let sub_items = link
        .nested_items
        .iter()
        .map(|i| load_summary_item(i, src_dir, sub_item_parents.clone()))
        .collect::<Result<Vec<_>>>()?;

    ch.sub_items = sub_items;

    Ok(ch)
}

/// A depth-first iterator over the items in a book.
///
/// # Note
///
/// This struct shouldn't be created directly, instead prefer the
/// [`Book::iter()`] method.
pub struct BookItems<'a> {
    items: VecDeque<&'a BookItem>,
}

impl<'a> Iterator for BookItems<'a> {
    type Item = &'a BookItem;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.items.pop_front();

        if let Some(&BookItem::Chapter(ref ch)) = item {
            // if we wanted a breadth-first iterator we'd `extend()` here
            for sub_item in ch.sub_items.iter().rev() {
                self.items.push_front(sub_item);
            }
        }

        item
    }
}

impl Display for Chapter {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(ref section_number) = self.number {
            write!(f, "{} ", section_number)?;
        }

        write!(f, "{}", self.name)
    }
}
