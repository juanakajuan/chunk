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
    let removed_text = expand_tabs(removed);
    let added_text = expand_tabs(added);
    if removed_text == added_text {
        return None;
    }

    let removed_tokens = intraline_tokens(&removed_text);
    let added_tokens = intraline_tokens(&added_text);
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

    let removed_ranges = changed_intraline_ranges(&removed_tokens, &common.removed);
    let added_ranges = changed_intraline_ranges(&added_tokens, &common.added);
    if removed_ranges.is_empty() && added_ranges.is_empty() {
        return None;
    }

    Some(IntralinePairRanges {
        removed: removed_ranges,
        added: added_ranges,
    })
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

    while let Some((byte_start, character)) = chars.next() {
        let kind = intraline_token_kind(character);
        let mut byte_end = byte_start + character.len_utf8();
        let mut char_end = char_start + 1;

        if kind != IntralineTokenKind::Punctuation {
            while let Some((next_byte, next_character)) = chars.peek().copied() {
                if intraline_token_kind(next_character) != kind {
                    break;
                }
                chars.next();
                byte_end = next_byte + next_character.len_utf8();
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

fn intraline_token_kind(character: char) -> IntralineTokenKind {
    if character.is_alphanumeric() || character == '_' {
        IntralineTokenKind::Word
    } else if character.is_whitespace() {
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
            let current = removed_index * width + added_index;
            let skip_removed_index = (removed_index + 1) * width + added_index;
            let skip_added_index = current + 1;
            let skip_both_index = skip_removed_index + 1;
            let tokens_match = intraline_tokens_equal(removed[removed_index], added[added_index]);
            lengths[current] = if tokens_match {
                lengths[skip_both_index] + 1
            } else {
                lengths[skip_removed_index].max(lengths[skip_added_index])
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
            continue;
        }

        let skip_removed_len = lengths[(removed_index + 1) * width + added_index];
        let skip_added_len = lengths[removed_index * width + added_index + 1];
        if skip_removed_len >= skip_added_len {
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
    if let Some(related) = intraline_similarity_meets_threshold(
        removed,
        added,
        removed_common,
        added_common,
        is_word_token,
        INTRALINE_MIN_WORD_SIMILARITY_PERCENT,
    ) {
        return related;
    }

    intraline_similarity_meets_threshold(
        removed,
        added,
        removed_common,
        added_common,
        is_non_whitespace_token,
        INTRALINE_MIN_FALLBACK_SIMILARITY_PERCENT,
    )
    .unwrap_or(false)
}

fn intraline_similarity_meets_threshold(
    removed: &[IntralineToken<'_>],
    added: &[IntralineToken<'_>],
    removed_common: &[bool],
    added_common: &[bool],
    include_token: fn(IntralineTokenKind) -> bool,
    min_similarity_percent: usize,
) -> Option<bool> {
    let denominator = intraline_char_count(removed, None, include_token).max(intraline_char_count(
        added,
        None,
        include_token,
    ));
    if denominator == 0 {
        return None;
    }

    let common = intraline_char_count(removed, Some(removed_common), include_token).min(
        intraline_char_count(added, Some(added_common), include_token),
    );
    Some(common * 100 >= denominator * min_similarity_percent)
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
        for styled_char in &mut chars[start..end] {
            styled_char.style = styled_char.style.add_modifier(Modifier::BOLD);
        }
    }

    chars_to_spans(&chars)
}
