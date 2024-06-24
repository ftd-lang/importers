use once_cell::sync::Lazy;
use regex::Regex;
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::ops::RangeBounds;

/// Take a range of lines from a string.
pub fn take_lines<R: RangeBounds<usize>>(s: &str, range: R) -> String {
    let start = match range.start_bound() {
        Excluded(&n) => n + 1,
        Included(&n) => n,
        Unbounded => 0,
    };
    let lines = s.lines().skip(start);
    match range.end_bound() {
        Excluded(end) => lines
            .take(end.saturating_sub(start))
            .collect::<Vec<_>>()
            .join("\n"),
        Included(end) => lines
            .take((end + 1).saturating_sub(start))
            .collect::<Vec<_>>()
            .join("\n"),
        Unbounded => lines.collect::<Vec<_>>().join("\n"),
    }
}

static ANCHOR_START: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"ANCHOR:\s*(?P<anchor_name>[\w_-]+)").unwrap());
static ANCHOR_END: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"ANCHOR_END:\s*(?P<anchor_name>[\w_-]+)").unwrap());

/// Take anchored lines from a string.
/// Lines containing anchor are ignored.
pub fn take_anchored_lines(s: &str, anchor: &str) -> String {
    let mut retained = Vec::<&str>::new();
    let mut anchor_found = false;

    for l in s.lines() {
        if anchor_found {
            match ANCHOR_END.captures(l) {
                Some(cap) => {
                    if &cap["anchor_name"] == anchor {
                        break;
                    }
                }
                None => {
                    if !ANCHOR_START.is_match(l) {
                        retained.push(l);
                    }
                }
            }
        } else if let Some(cap) = ANCHOR_START.captures(l) {
            if &cap["anchor_name"] == anchor {
                anchor_found = true;
            }
        }
    }

    retained.join("\n")
}

/// Keep lines contained within the range specified as-is.
/// For any lines not in the range, include them but use `#` at the beginning. This will hide the
/// lines from initial display but include them when expanding the code snippet or testing with
/// rustdoc.
pub fn take_rustdoc_include_lines<R: RangeBounds<usize>>(s: &str, range: R) -> String {
    let mut output = String::with_capacity(s.len());

    for (index, line) in s.lines().enumerate() {
        if !range.contains(&index) {
            output.push_str("# ");
        }
        output.push_str(line);
        output.push('\n');
    }
    output.pop();
    output
}

/// Keep lines between the anchor comments specified as-is.
/// For any lines not between the anchors, include them but use `#` at the beginning. This will
/// hide the lines from initial display but include them when expanding the code snippet or testing
/// with rustdoc.
pub fn take_rustdoc_include_anchored_lines(s: &str, anchor: &str) -> String {
    let mut output = String::with_capacity(s.len());
    let mut within_anchored_section = false;

    for l in s.lines() {
        if within_anchored_section {
            match ANCHOR_END.captures(l) {
                Some(cap) => {
                    if &cap["anchor_name"] == anchor {
                        within_anchored_section = false;
                    }
                }
                None => {
                    if !ANCHOR_START.is_match(l) {
                        output.push_str(l);
                        output.push('\n');
                    }
                }
            }
        } else if let Some(cap) = ANCHOR_START.captures(l) {
            if &cap["anchor_name"] == anchor {
                within_anchored_section = true;
            }
        } else if !ANCHOR_END.is_match(l) {
            output.push_str("# ");
            output.push_str(l);
            output.push('\n');
        }
    }

    output.pop();
    output
}
