use std::ops::Range;

use ratatui::style::Modifier;
use ratatui::text::Span;

use crate::model::{DiffLine, DiffLineKind};

use super::text::{chars_to_spans, expand_tabs, styled_chars};

const INTRALINE_MAX_BLOCK_LINES: usize = 32;
const INTRALINE_MAX_TOKENS: usize = 512;
const INTRALINE_MIN_WORD_SIMILARITY_PERCENT: usize = 35;
const INTRALINE_MIN_FALLBACK_SIMILARITY_PERCENT: usize = 50;

pub(super) fn intraline_ranges_for_block(block: &[DiffLine]) -> Vec<Vec<Range<usize>>> {
    let mut ranges = vec![Vec::new(); block.len()];
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for (index, line) in block.iter().enumerate() {
        match line.kind {
            DiffLineKind::Removed => removed.push((index, line)),
            DiffLineKind::Added => added.push((index, line)),
            DiffLineKind::Context | DiffLineKind::Meta => {}
        }
    }

    if removed.is_empty()
        || added.is_empty()
        || removed.len() > INTRALINE_MAX_BLOCK_LINES
        || added.len() > INTRALINE_MAX_BLOCK_LINES
    {
        return ranges;
    }

    let pairs = removed.into_iter().zip(added);
    for ((removed_index, removed_line), (added_index, added_line)) in pairs {
        let Some(pair_ranges) = intraline_pair_ranges(&removed_line.content, &added_line.content)
        else {
            continue;
        };

        ranges[removed_index] = pair_ranges.removed;
        ranges[added_index] = pair_ranges.added;
    }

    ranges
}

pub(super) fn is_intraline_candidate(kind: DiffLineKind) -> bool {
    matches!(kind, DiffLineKind::Added | DiffLineKind::Removed)
}

pub(super) fn intraline_block_end(lines: &[DiffLine], start: usize) -> usize {
    lines[start..]
        .iter()
        .position(|line| !is_intraline_candidate(line.kind))
        .map_or(lines.len(), |offset| start + offset)
}

struct IntralinePairRanges {
    removed: Vec<Range<usize>>,
    added: Vec<Range<usize>>,
}

fn intraline_pair_ranges(removed: &str, added: &str) -> Option<IntralinePairRanges> {
    let removed = expand_tabs(removed);
    let added = expand_tabs(added);
    if removed == added {
        return None;
    }

    let removed_tokens = intraline_tokens(&removed);
    let added_tokens = intraline_tokens(&added);
    if removed_tokens.is_empty()
        || added_tokens.is_empty()
        || removed_tokens.len() > INTRALINE_MAX_TOKENS
        || added_tokens.len() > INTRALINE_MAX_TOKENS
    {
        return None;
    }

    let common = common_intraline_tokens(&removed_tokens, &added_tokens);
    if !intraline_lines_are_related(
        &removed_tokens,
        &added_tokens,
        &common.removed,
        &common.added,
    ) {
        return None;
    }

    let removed = changed_intraline_ranges(&removed_tokens, &common.removed);
    let added = changed_intraline_ranges(&added_tokens, &common.added);
    if removed.is_empty() && added.is_empty() {
        return None;
    }

    Some(IntralinePairRanges { removed, added })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntralineTokenKind {
    Word,
    Whitespace,
    Punctuation,
}

#[derive(Debug, Clone, Copy)]
struct IntralineToken<'a> {
    text: &'a str,
    start: usize,
    end: usize,
    kind: IntralineTokenKind,
}

struct CommonIntralineTokens {
    removed: Vec<bool>,
    added: Vec<bool>,
}

fn intraline_tokens(text: &str) -> Vec<IntralineToken<'_>> {
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut char_start = 0;

    while let Some((byte_start, value)) = chars.next() {
        let kind = intraline_token_kind(value);
        let mut byte_end = byte_start + value.len_utf8();
        let mut char_end = char_start + 1;

        if kind != IntralineTokenKind::Punctuation {
            while let Some((next_byte, next_value)) = chars.peek().copied() {
                if intraline_token_kind(next_value) != kind {
                    break;
                }
                chars.next();
                byte_end = next_byte + next_value.len_utf8();
                char_end += 1;
            }
        }

        tokens.push(IntralineToken {
            text: &text[byte_start..byte_end],
            start: char_start,
            end: char_end,
            kind,
        });
        char_start = char_end;
    }

    tokens
}

fn intraline_token_kind(value: char) -> IntralineTokenKind {
    if value.is_alphanumeric() || value == '_' {
        IntralineTokenKind::Word
    } else if value.is_whitespace() {
        IntralineTokenKind::Whitespace
    } else {
        IntralineTokenKind::Punctuation
    }
}

fn common_intraline_tokens(
    removed: &[IntralineToken<'_>],
    added: &[IntralineToken<'_>],
) -> CommonIntralineTokens {
    let width = added.len() + 1;
    let mut lengths = vec![0; (removed.len() + 1) * width];

    for removed_index in (0..removed.len()).rev() {
        for added_index in (0..added.len()).rev() {
            let index = removed_index * width + added_index;
            lengths[index] = if intraline_tokens_equal(removed[removed_index], added[added_index]) {
                lengths[(removed_index + 1) * width + added_index + 1] + 1
            } else {
                lengths[(removed_index + 1) * width + added_index]
                    .max(lengths[removed_index * width + added_index + 1])
            };
        }
    }

    let mut removed_common = vec![false; removed.len()];
    let mut added_common = vec![false; added.len()];
    let mut removed_index = 0;
    let mut added_index = 0;
    while removed_index < removed.len() && added_index < added.len() {
        if intraline_tokens_equal(removed[removed_index], added[added_index]) {
            removed_common[removed_index] = true;
            added_common[added_index] = true;
            removed_index += 1;
            added_index += 1;
        } else if lengths[(removed_index + 1) * width + added_index]
            >= lengths[removed_index * width + added_index + 1]
        {
            removed_index += 1;
        } else {
            added_index += 1;
        }
    }

    CommonIntralineTokens {
        removed: removed_common,
        added: added_common,
    }
}

fn intraline_tokens_equal(left: IntralineToken<'_>, right: IntralineToken<'_>) -> bool {
    left.kind == right.kind && left.text == right.text
}

fn intraline_lines_are_related(
    removed: &[IntralineToken<'_>],
    added: &[IntralineToken<'_>],
    removed_common: &[bool],
    added_common: &[bool],
) -> bool {
    let removed_words = intraline_char_count(removed, None, is_word_token);
    let added_words = intraline_char_count(added, None, is_word_token);
    let word_denominator = removed_words.max(added_words);
    if word_denominator > 0 {
        let common_words = intraline_char_count(removed, Some(removed_common), is_word_token).min(
            intraline_char_count(added, Some(added_common), is_word_token),
        );
        return common_words * 100 >= word_denominator * INTRALINE_MIN_WORD_SIMILARITY_PERCENT;
    }

    let removed_non_whitespace = intraline_char_count(removed, None, is_non_whitespace_token);
    let added_non_whitespace = intraline_char_count(added, None, is_non_whitespace_token);
    let fallback_denominator = removed_non_whitespace.max(added_non_whitespace);
    if fallback_denominator == 0 {
        return false;
    }

    let common_non_whitespace =
        intraline_char_count(removed, Some(removed_common), is_non_whitespace_token).min(
            intraline_char_count(added, Some(added_common), is_non_whitespace_token),
        );
    common_non_whitespace * 100 >= fallback_denominator * INTRALINE_MIN_FALLBACK_SIMILARITY_PERCENT
}

fn intraline_char_count(
    tokens: &[IntralineToken<'_>],
    common: Option<&[bool]>,
    include_token: fn(IntralineTokenKind) -> bool,
) -> usize {
    tokens
        .iter()
        .enumerate()
        .filter(|(index, token)| {
            include_token(token.kind) && common.is_none_or(|common| common[*index])
        })
        .map(|(_, token)| intraline_token_len(token))
        .sum()
}

fn is_word_token(kind: IntralineTokenKind) -> bool {
    kind == IntralineTokenKind::Word
}

fn is_non_whitespace_token(kind: IntralineTokenKind) -> bool {
    kind != IntralineTokenKind::Whitespace
}

fn intraline_token_len(token: &IntralineToken<'_>) -> usize {
    token.end - token.start
}

fn changed_intraline_ranges(tokens: &[IntralineToken<'_>], common: &[bool]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut range_start = None;
    let mut range_end = 0;

    for (token, is_common) in tokens.iter().zip(common) {
        if *is_common {
            if let Some(start) = range_start.take() {
                ranges.push(start..range_end);
            }
            continue;
        }

        range_start.get_or_insert(token.start);
        range_end = token.end;
    }

    if let Some(start) = range_start {
        ranges.push(start..range_end);
    }

    ranges
}

pub(super) fn emphasize_spans(
    spans: Vec<Span<'static>>,
    ranges: &[Range<usize>],
) -> Vec<Span<'static>> {
    if ranges.is_empty() {
        return spans;
    }

    let mut chars = styled_chars(spans);
    for range in ranges {
        let start = range.start.min(chars.len());
        let end = range.end.min(chars.len());
        for character in &mut chars[start..end] {
            character.style = character.style.add_modifier(Modifier::BOLD);
        }
    }

    chars_to_spans(&chars)
}
