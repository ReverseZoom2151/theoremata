//! The @-mention popup that completes a proof-graph node name inside the composer.
//!
//! WHY this module exists: Theoremata is graph-first, so a user typing a chat
//! message often wants to point at an existing node by name ("prove @lemma_3
//! next"). This mirrors the slash-command popup in `command_popup.rs`, but over
//! the PROJECT's live node names instead of a fixed command registry. The two
//! differences that follow from "live node names" drive the whole design:
//!   1. The candidate set is dynamic and owned (`&[String]`), not `&'static`, so
//!      matches are returned as owned `String`s rather than borrowed specs.
//!   2. A mention can appear ANYWHERE in the message, not just at the start, so
//!      activation keys off the LAST whitespace-separated token rather than the
//!      whole composer text.
//!
//! Like the slash popup, this depends only on ratatui + crossterm: it produces
//! styled lines for the integrator to draw and reports its own height; it never
//! touches the store, the model, or the composer's text buffer. The integrator
//! is responsible for splicing the accepted name back into the composer.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// The most rows the popup will ever show at once. WHY a cap: a bare "@" matches
/// every node in the project, and a popup taller than a handful of rows would
/// eat the transcript. Extra matches are simply not drawn.
const MAX_ROWS: usize = 8;

/// The style used to make a node name stand out, matching the slash popup's cyan
/// so the two completion surfaces read as one family.
fn name_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

/// The dim style for the empty-state line, so a mistyped mention reads as "no
/// match" rather than a blank box.
fn dim_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// The @-mention popup state. It is ACTIVE only while the token under the end of
/// the composer text starts with '@' and has no whitespace yet: that means the
/// user is still typing the mention. Once a space follows the mention the name
/// is finished, so completing it no longer makes sense and the popup steps aside.
pub struct MentionPopup {
    /// True while the last token is a partial @mention.
    active: bool,
    /// The typed text WITHOUT the leading '@', e.g. "lem" for "@lem". Stored so
    /// matching is a pure function of this filter and the supplied node names.
    filter: String,
    /// The node names fed on the last `sync`, already ranked best-first and
    /// capped. Held as owned Strings because the candidate set is dynamic; the
    /// popup does not borrow the caller's slice past the `sync` call.
    ranked: Vec<String>,
    /// Index into `ranked` of the highlighted row.
    selected: usize,
}

impl Default for MentionPopup {
    fn default() -> Self {
        Self::new()
    }
}

impl MentionPopup {
    pub fn new() -> Self {
        MentionPopup {
            active: false,
            filter: String::new(),
            ranked: Vec::new(),
            selected: 0,
        }
    }

    /// Feed the current composer text AND the current list of node names on every
    /// keystroke. Activation looks at the LAST whitespace-separated token: the
    /// popup is active iff that token starts with '@'. The filter is that token
    /// with the '@' stripped, so "prove @lem" filters on "lem" while "prove
    /// @lemma_3 and" (a space after the mention) is inactive because the last
    /// token is "and".
    pub fn sync(&mut self, composer_text: &str, node_names: &[String]) {
        // The last whitespace-separated token is the one under the cursor-end.
        // `split_whitespace` collapses trailing whitespace, so a text ending in a
        // space yields a last token that is NOT the mention, which is exactly the
        // "mention finished" case we want to treat as inactive.
        let last_token = composer_text.split_whitespace().next_back();
        let active = matches!(last_token, Some(tok) if tok.starts_with('@'));

        // The filter is the last token minus its leading '@'. When inactive we
        // clear it so a stale filter never leaks into a later match query.
        let new_filter = if active {
            // `active` guarantees a Some token starting with '@'.
            last_token.unwrap()[1..].to_string()
        } else {
            String::new()
        };

        // Reset the selection to the top whenever the filter changes, so the best
        // (top-ranked) match is always the default as the user narrows.
        if new_filter != self.filter {
            self.selected = 0;
        }

        self.active = active;
        self.filter = new_filter;
        self.ranked = if active {
            Self::rank(&self.filter, node_names)
        } else {
            Vec::new()
        };

        // Keep the selection in range in case the ranked list shrank.
        if self.selected >= self.ranked.len() {
            self.selected = self.ranked.len().saturating_sub(1);
        }
    }

    /// Rank node names against the filter, best first, capped at `MAX_ROWS`. WHY
    /// this ranking (exact, then prefix, then substring, each alphabetical by
    /// name): it is cheap, predictable, and matches how people expect a mention
    /// menu to behave, without the surprises of a fuzzy scorer. All comparisons
    /// are case-insensitive so "@LEM" still finds "lemma_1". An empty filter
    /// ("@" just typed) treats every name as a prefix match, listing all names so
    /// the user can browse.
    fn rank(filter: &str, node_names: &[String]) -> Vec<String> {
        let f = filter.to_lowercase();
        let mut scored: Vec<(u8, &String)> = node_names
            .iter()
            .filter_map(|name| {
                let lower = name.to_lowercase();
                let rank = if lower == f {
                    0u8
                } else if lower.starts_with(&f) {
                    // An empty filter lands here for every name, so "@" browses.
                    1
                } else if lower.contains(&f) {
                    2
                } else {
                    return None;
                };
                Some((rank, name))
            })
            .collect();
        // Sort by rank first, then alphabetically by name within a rank. The
        // name tie-break is total, so the order is deterministic for a given
        // filter and node list (and stable across duplicate ranks).
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
        scored
            .into_iter()
            .take(MAX_ROWS)
            .map(|(_, name)| name.clone())
            .collect()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The ranked node names for the current partial mention, best first (owned).
    /// Empty when inactive or when nothing matches.
    pub fn matches(&self) -> Vec<String> {
        self.ranked.clone()
    }

    /// Handle a navigation key while the popup is active. Up and Down move the
    /// selection (wrapping at the ends), Tab and Enter accept the highlighted
    /// node. Returns the accepted node name so the integrator can replace the
    /// partial @token in the composer with `@<name> `; returns None for a key the
    /// popup does not consume (and for accept when there is nothing to accept).
    pub fn key(&mut self, key: KeyEvent) -> Option<String> {
        if self.ranked.is_empty() {
            return None;
        }
        // Defend against a selection left out of range by a shrinking list.
        if self.selected >= self.ranked.len() {
            self.selected = self.ranked.len() - 1;
        }
        match key.code {
            KeyCode::Up => {
                // Wrap to the bottom when moving up off the top row.
                self.selected = if self.selected == 0 {
                    self.ranked.len() - 1
                } else {
                    self.selected - 1
                };
                None
            }
            KeyCode::Down => {
                // Wrap to the top when moving down off the last row.
                self.selected = (self.selected + 1) % self.ranked.len();
                None
            }
            KeyCode::Tab | KeyCode::Enter => Some(self.ranked[self.selected].clone()),
            _ => None,
        }
    }

    /// The popup rendered to styled lines for drawing above the composer, with
    /// the selected row highlighted. The `width` is accepted for the caller's
    /// layout budget; node names are short single-column entries that fit
    /// comfortably, so no per-line truncation is applied here.
    pub fn lines(&self, _width: u16) -> Vec<Line<'static>> {
        if !self.active {
            return Vec::new();
        }
        if self.ranked.is_empty() {
            // Honest empty state rather than a blank box, so a mention that hits
            // no node reads as "no matching node" instead of a silent no-op.
            return vec![Line::styled("  no matching node".to_string(), dim_style())];
        }
        let sel = self.selected.min(self.ranked.len() - 1);
        self.ranked
            .iter()
            .enumerate()
            .map(|(i, name)| {
                // Each row is a bold "@name" so it reads as a mention token.
                let mut span = Span::styled(format!("  @{name}"), name_style());
                if i == sel {
                    // The selected row reverses so the whole row highlights, which
                    // reads clearly on any terminal palette without a background.
                    span.style = span.style.add_modifier(Modifier::REVERSED);
                }
                Line::from(span)
            })
            .collect()
    }

    /// The number of visible rows the popup occupies. The integrator uses this to
    /// reserve height above the composer. When active with no matches we still
    /// show a single empty-state line, so the height is at least one while active
    /// and zero when inactive.
    pub fn height(&self) -> u16 {
        if !self.active {
            return 0;
        }
        (self.ranked.len().min(MAX_ROWS)).max(1) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};

    fn kev(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn active_on_partial_mention_with_prefix_order() {
        let nodes = names(&["lemma_1", "lemma_3", "theorem"]);
        let mut p = MentionPopup::new();
        p.sync("prove @lem", &nodes);
        assert!(p.is_active());
        // Both lemmas are prefix matches; "theorem" is not; alphabetical order.
        assert_eq!(
            p.matches(),
            vec!["lemma_1".to_string(), "lemma_3".to_string()]
        );
    }

    #[test]
    fn trailing_space_after_mention_is_inactive() {
        let nodes = names(&["lemma_1", "lemma_3", "theorem"]);
        let mut p = MentionPopup::new();
        p.sync("@lemma_3 x", &nodes);
        assert!(!p.is_active());
        assert!(p.matches().is_empty());
        assert_eq!(p.height(), 0);
    }

    #[test]
    fn plain_text_is_inactive() {
        let nodes = names(&["lemma_1"]);
        let mut p = MentionPopup::new();
        p.sync("hello", &nodes);
        assert!(!p.is_active());
    }

    #[test]
    fn bare_at_lists_all_capped() {
        let nodes = names(&["b_node", "a_node", "c_node"]);
        let mut p = MentionPopup::new();
        p.sync("@", &nodes);
        assert!(p.is_active());
        // Empty filter lists everything, alphabetical.
        assert_eq!(
            p.matches(),
            vec![
                "a_node".to_string(),
                "b_node".to_string(),
                "c_node".to_string()
            ]
        );
    }

    #[test]
    fn down_then_enter_returns_second_match() {
        let nodes = names(&["lemma_1", "lemma_3"]);
        let mut p = MentionPopup::new();
        p.sync("prove @lem", &nodes);
        let second = p.matches()[1].clone();
        assert_eq!(p.key(kev(KeyCode::Down)), None);
        assert_eq!(p.key(kev(KeyCode::Enter)), Some(second));
    }

    #[test]
    fn case_insensitive_filter_matches() {
        let nodes = names(&["Lemma_Main", "theorem"]);
        let mut p = MentionPopup::new();
        p.sync("@lemma", &nodes);
        assert!(p.is_active());
        assert_eq!(p.matches(), vec!["Lemma_Main".to_string()]);
    }

    #[test]
    fn empty_node_list_is_inactive_rows() {
        let nodes: Vec<String> = Vec::new();
        let mut p = MentionPopup::new();
        p.sync("@lem", &nodes);
        // The token IS a mention, so the popup is active, but there is nothing to
        // offer: matches empty, and the rendered lines are the honest empty state.
        assert!(p.is_active());
        assert!(p.matches().is_empty());
        assert_eq!(p.key(kev(KeyCode::Enter)), None);
        let lines = p.lines(40);
        assert_eq!(lines.len(), 1);
        assert_eq!(p.height(), 1);
    }

    #[test]
    fn selection_wraps_up_to_last() {
        let nodes = names(&["a_node", "b_node", "c_node"]);
        let mut p = MentionPopup::new();
        p.sync("@_node", &nodes);
        let n = p.matches().len();
        assert_eq!(n, 3);
        // Up from the top wraps to the last row, accepted via Tab.
        assert_eq!(p.key(kev(KeyCode::Up)), None);
        assert_eq!(p.key(kev(KeyCode::Tab)), Some(p.matches()[n - 1].clone()));
    }

    #[test]
    fn substring_matches_when_no_prefix() {
        let nodes = names(&["main_lemma", "theorem"]);
        let mut p = MentionPopup::new();
        p.sync("@lemma", &nodes);
        // No name starts with "lemma", but "main_lemma" contains it.
        assert_eq!(p.matches(), vec!["main_lemma".to_string()]);
    }

    #[test]
    fn exact_ranks_ahead_of_prefix() {
        let nodes = names(&["lemma", "lemma_1"]);
        let mut p = MentionPopup::new();
        p.sync("@lemma", &nodes);
        // Exact "lemma" must sort ahead of the longer prefix match.
        assert_eq!(p.matches()[0], "lemma".to_string());
    }
}
