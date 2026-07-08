//! Subsumption + canonical goal de-duplication (a shared contract for the search
//! layer's transposition table and proof-pool dedup).
//!
//! Two proof goals that are α-equivalent (differ only in the names of bound
//! variables) or that list the same hypotheses in a different order denote the
//! *same* goal, yet a naive string key treats them as distinct — so the search
//! re-explores work it has already done and the pool stores redundant candidates.
//! [`CanonicalGoal`] normalises a goal into an ordered-literal form whose
//! [`key`](CanonicalGoal::key) is invariant under those two transformations, and
//! [`subsumes`] decides when one (more general) goal makes another redundant.
//!
//! ## Scope / soundness limits (read before trusting it)
//!
//! This is a **pragmatic, string/literal-level canonicalizer**, deliberately not
//! a first-order unifier or a proof-theoretic entailment checker. It is tuned to
//! be *sound-leaning*: it only reports [`subsumes`] `= true` when the relationship
//! is structurally obvious (identical canonical conclusion, and the general goal's
//! hypothesis *set* is a subset of the specific goal's). Concretely it:
//! * splits a goal on the turnstile `⊢` (or ASCII `|-`) into hypotheses and a
//!   conclusion, and splits hypotheses on top-level commas;
//! * α-renames bound variables (those introduced by `∀ ∃ λ Π Σ ∏ ∑` or the words
//!   `forall`/`exists`/`lambda`/`fun`) to De-Bruijn-ish canonical names `v0, v1,
//!   …` in order of first introduction, **per literal**;
//! * collapses whitespace and sorts hypothesis literals into a stable order.
//!
//! It does **not** understand: implication between distinct conclusions
//! (`P ⊢ Q` vs `⊢ P → Q`), commutativity/associativity of connectives inside a
//! literal, defeq/unfolding, hypothesis *labels* (a hypothesis is matched by its
//! full literal text), or α-renaming of free variables shared across literals.
//! When in doubt it returns the *conservative* answer (no shared key / no
//! subsumption), so a false negative costs re-work but never unsoundly discards a
//! goal.

use std::collections::BTreeSet;

/// A goal normalised into ordered-literal form: a sorted, de-duplicated set of
/// hypothesis literals and a single conclusion literal, each α-canonicalised.
///
/// Two α-equivalent goals, or two goals differing only in hypothesis order,
/// produce equal [`CanonicalGoal`]s and hence an equal [`key`](Self::key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalGoal {
    /// Canonicalised hypothesis literals, sorted and de-duplicated (a set).
    hypotheses: Vec<String>,
    /// The canonicalised conclusion literal.
    conclusion: String,
}

impl CanonicalGoal {
    /// Canonicalize a goal string into ordered-literal form.
    ///
    /// The input is split on the first turnstile (`⊢` or ASCII `|-`) into
    /// `hypotheses ⊢ conclusion`; with no turnstile the whole string is the
    /// conclusion and there are no hypotheses. Hypotheses are split on top-level
    /// commas, each literal is α-canonicalised and whitespace-normalised, and the
    /// resulting literals are sorted and de-duplicated.
    pub fn parse(s: &str) -> CanonicalGoal {
        let (lhs, rhs) = split_turnstile(s);
        let conclusion = canonical_literal(rhs);
        let mut hypotheses: Vec<String> = Vec::new();
        if let Some(lhs) = lhs {
            for raw in split_top_level_commas(lhs) {
                let lit = canonical_literal(&raw);
                if !lit.is_empty() {
                    hypotheses.push(lit);
                }
            }
        }
        // Stable global order + de-dup so hypothesis reordering is invariant.
        hypotheses.sort();
        hypotheses.dedup();
        CanonicalGoal {
            hypotheses,
            conclusion,
        }
    }

    /// The canonical hash key: two α-equivalent or hypothesis-reordered goals
    /// return the same string. The key is the human-readable canonical form
    /// itself (`h0 , h1 ⊢ concl`), which is both a valid dedup key and directly
    /// inspectable in logs / transposition tables.
    pub fn key(&self) -> String {
        format!("{} ⊢ {}", self.hypotheses.join(" , "), self.conclusion)
    }

    /// The canonicalised hypothesis literals (sorted, de-duplicated).
    pub fn hypotheses(&self) -> &[String] {
        &self.hypotheses
    }

    /// The canonicalised conclusion literal.
    pub fn conclusion(&self) -> &str {
        &self.conclusion
    }
}

/// True if `general` makes `specific` redundant: it proves the same conclusion
/// from a subset of the hypotheses (weaker premises, same-or-equal conclusion),
/// so anything provable via `specific` is already provable via `general`.
///
/// Pragmatic, sound-leaning rule (see the module scope limits): requires the
/// canonical conclusions to be *equal* and `general`'s hypothesis set to be a
/// subset of `specific`'s. Equal goals subsume each other (each is redundant
/// given the other).
pub fn subsumes(general: &CanonicalGoal, specific: &CanonicalGoal) -> bool {
    if general.conclusion != specific.conclusion {
        return false;
    }
    let specific_hyps: BTreeSet<&str> = specific.hypotheses.iter().map(String::as_str).collect();
    general
        .hypotheses
        .iter()
        .all(|h| specific_hyps.contains(h.as_str()))
}

/// Convenience: [`subsumes`] over raw goal strings (parses both first).
pub fn subsumes_str(general: &str, specific: &str) -> bool {
    subsumes(&CanonicalGoal::parse(general), &CanonicalGoal::parse(specific))
}

/// Split a goal on its first turnstile into `(Some(hypotheses), conclusion)`, or
/// `(None, whole)` when there is none. Recognises the Unicode `⊢` and ASCII `|-`.
fn split_turnstile(s: &str) -> (Option<&str>, &str) {
    if let Some(idx) = s.find('⊢') {
        let (l, r) = s.split_at(idx);
        return (Some(l), &r['⊢'.len_utf8()..]);
    }
    if let Some(idx) = s.find("|-") {
        let (l, r) = s.split_at(idx);
        return (Some(l), &r[2..]);
    }
    (None, s)
}

/// Split hypotheses on top-level commas — commas that are *not* inside brackets
/// `() [] {}`. Note the scope limit: a bare quantifier-body comma (`∀ x, P x`) is
/// still treated as a separator, so a hypothesis whose top-level connective is a
/// quantifier should be parenthesised (`(∀ x, P x)`) to stay intact.
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth = (depth - 1).max(0);
                cur.push(c);
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// A lexical token: a maximal run of identifier characters, or a single other
/// (non-space) character.
enum Tok {
    Word(String),
    Sym(char),
}

/// Whether `w` (as a `Word`) or `c` (as a `Sym`) introduces bound variables.
fn is_quantifier_word(w: &str) -> bool {
    matches!(w, "forall" | "exists" | "lambda" | "fun")
}
fn is_quantifier_sym(c: char) -> bool {
    matches!(c, '∀' | '∃' | 'λ' | 'Π' | 'Σ' | '∏' | '∑')
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Tokenize into identifier words and single-character symbols, dropping
/// whitespace (which only separates tokens).
fn tokenize(s: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if is_ident_char(c) {
            cur.push(c);
        } else {
            if !cur.is_empty() {
                toks.push(Tok::Word(std::mem::take(&mut cur)));
            }
            if !c.is_whitespace() {
                toks.push(Tok::Sym(c));
            }
        }
    }
    if !cur.is_empty() {
        toks.push(Tok::Word(cur));
    }
    toks
}

/// α-canonicalize a single literal: rename each bound variable to `v0, v1, …` in
/// order of introduction and rebuild with single-space separation, so two
/// α-equivalent literals become byte-identical.
fn canonical_literal(s: &str) -> String {
    let toks = tokenize(s);

    // First pass: collect bound-variable names in introduction order. After a
    // quantifier token, every consecutive `Word` (a binder group like `∀ x y,`)
    // is a bound variable until the next symbol terminates the group.
    let mut order: Vec<String> = Vec::new();
    let mut collecting = false;
    for t in &toks {
        match t {
            Tok::Sym(c) if is_quantifier_sym(*c) => collecting = true,
            Tok::Word(w) if is_quantifier_word(w) => collecting = true,
            Tok::Word(w) if collecting => {
                if !order.iter().any(|n| n == w) {
                    order.push(w.clone());
                }
            }
            // Any other symbol ends the current binder group.
            Tok::Sym(_) => collecting = false,
            Tok::Word(_) => {}
        }
    }

    // Map each bound name to a canonical De-Bruijn-ish name.
    let rename = |w: &str| -> Option<String> {
        order
            .iter()
            .position(|n| n == w)
            .map(|i| format!("v{i}"))
    };

    // Second pass: rebuild, substituting bound variables, joining with single
    // spaces for a deterministic representation.
    let mut out = String::new();
    for t in &toks {
        if !out.is_empty() {
            out.push(' ');
        }
        match t {
            Tok::Word(w) => match rename(w) {
                Some(canon) => out.push_str(&canon),
                None => out.push_str(w),
            },
            Tok::Sym(c) => out.push(*c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_equivalent_goals_share_a_key() {
        // Same goal, bound variable renamed x -> y: must canonicalize identically.
        let a = CanonicalGoal::parse("⊢ ∀ x, P x → P x");
        let b = CanonicalGoal::parse("⊢ ∀ y, P y → P y");
        assert_eq!(a.key(), b.key(), "α-equivalent goals must share a key");

        // A genuinely different conclusion must NOT collide.
        let c = CanonicalGoal::parse("⊢ ∀ x, P x → Q x");
        assert_ne!(a.key(), c.key());
    }

    #[test]
    fn hypothesis_reordering_shares_a_key() {
        let a = CanonicalGoal::parse("P x, Q y ⊢ R z");
        let b = CanonicalGoal::parse("Q y, P x ⊢ R z");
        assert_eq!(a.key(), b.key(), "hyp order must not affect the key");
    }

    #[test]
    fn duplicate_hypotheses_collapse() {
        let a = CanonicalGoal::parse("P x, P x, Q y ⊢ R");
        let b = CanonicalGoal::parse("Q y, P x ⊢ R");
        assert_eq!(a.key(), b.key());
        assert_eq!(a.hypotheses().len(), 2);
    }

    #[test]
    fn ascii_turnstile_matches_unicode() {
        let a = CanonicalGoal::parse("P x |- Q x");
        let b = CanonicalGoal::parse("P x ⊢ Q x");
        assert_eq!(a.key(), b.key());
    }

    #[test]
    fn subset_hypotheses_same_conclusion_subsumes() {
        // `general` proves R from just {P x}; `specific` also assumes Q y. The
        // general (weaker-premise) goal makes the specific one redundant.
        let general = CanonicalGoal::parse("P x ⊢ R z");
        let specific = CanonicalGoal::parse("P x, Q y ⊢ R z");
        assert!(subsumes(&general, &specific));
        // Not the other way round: needing MORE hypotheses is not more general.
        assert!(!subsumes(&specific, &general));
    }

    #[test]
    fn subsumption_is_alpha_and_order_invariant() {
        // α-invariance: bound-variable rename in the (shared) conclusion; the
        // general goal drops a hypothesis the specific one keeps.
        assert!(subsumes_str("⊢ ∀ x, P x", "H ⊢ ∀ y, P y"));
        // Order-invariance: same hypothesis set in a different order.
        assert!(subsumes_str("A, B ⊢ C", "B, A ⊢ C"));
        // Bracketed commas do not fragment a hypothesis.
        assert!(subsumes_str("f(a, b) ⊢ C", "f(a, b), D ⊢ C"));
    }

    #[test]
    fn equal_goals_subsume_each_other() {
        let g = CanonicalGoal::parse("P ⊢ Q");
        assert!(subsumes(&g, &g));
    }

    #[test]
    fn unrelated_goals_do_not_subsume() {
        // Different conclusions: neither direction subsumes.
        let a = CanonicalGoal::parse("P x ⊢ Q x");
        let b = CanonicalGoal::parse("P x ⊢ R x");
        assert!(!subsumes(&a, &b));
        assert!(!subsumes(&b, &a));

        // Same conclusion but a hypothesis the other lacks: general has an extra
        // premise not present in `specific`, so it is not a subset.
        let g = CanonicalGoal::parse("P x, S w ⊢ Q x");
        let s = CanonicalGoal::parse("P x, Q y ⊢ Q x");
        assert!(!subsumes(&g, &s));
        assert!(!subsumes(&s, &g));
    }
}
