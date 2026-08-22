//! Who may see what the user publishes.

use coracle_lib::events::{HasCreatedAt, HasId, HasKind, HasPubkey, HasTags};
use coracle_lib::filters::Filter;
use serde::{Deserialize, Serialize};

use crate::model::{BLOCK, MUTE, Scope, TRUST};

/// One rule: which of the user's own events it governs, and who may see them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisibilityRule {
    /// The events this governs.
    pub filter: Filter,
    /// Who may see them.
    pub scope: Scope,
}

/// Who may see what the user publishes, as rules tried in order.
///
/// The first rule whose filter matches an event governs it, and `default`
/// governs an event no rule matches — so every event has exactly one scope and
/// no ordering leaves one uncovered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visibility {
    /// The rules, in precedence order.
    pub rules: Vec<VisibilityRule>,
    /// Who may see an event no rule matches.
    pub default: Scope,
}

impl Default for Visibility {
    /// The user's lists to trusted peers, and everything else public.
    ///
    /// The social graph is the sensitive half of what a user publishes: a
    /// trust list names people they have met in person. It is not encrypted,
    /// because trusted peers are meant to read it, so this rule is the whole
    /// of what keeps it from a stranger.
    fn default() -> Self {
        Self {
            rules: vec![VisibilityRule {
                filter: Filter::new().add_kinds([MUTE, TRUST, BLOCK]),
                scope: Scope::Trusted,
            }],
            default: Scope::Public,
        }
    }
}

impl Visibility {
    /// The scope for an event. Only events signed by the user make sense to check here.
    #[must_use]
    pub fn scope_for<E>(&self, event: &E) -> Scope
    where
        E: HasId + HasKind + HasPubkey + HasTags + HasCreatedAt,
    {
        self.rules
            .iter()
            .find(|rule| rule.filter.matches(event))
            .map_or(self.default, |rule| rule.scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use coracle_kinds::profile;
    use coracle_lib::filters::TagMatch;
    use coracle_lib::tags::Tags;

    use crate::fixtures::{author, event};

    fn rule(filter: Filter, scope: Scope) -> VisibilityRule {
        VisibilityRule { filter, scope }
    }

    #[test]
    fn the_first_matching_rule_governs() {
        let visibility = Visibility {
            rules: vec![
                rule(Filter::new().add_kinds([profile::KIND]), Scope::Public),
                rule(Filter::new(), Scope::Trusted),
            ],
            default: Scope::Nothing,
        };

        // The profile matches the first rule, so the catch-all below it is
        // never reached.
        let profile = event(author(1), profile::KIND, 1, "", Tags::new());
        let note = event(author(1), 1, 1, "", Tags::new());

        assert_eq!(visibility.scope_for(&profile), Scope::Public);
        assert_eq!(visibility.scope_for(&note), Scope::Trusted);
    }

    #[test]
    fn a_rule_below_one_that_matches_everything_is_dead() {
        let visibility = Visibility {
            rules: vec![
                rule(Filter::new(), Scope::Trusted),
                rule(Filter::new().add_kinds([profile::KIND]), Scope::Public),
            ],
            default: Scope::Nothing,
        };

        let profile = event(author(1), profile::KIND, 1, "", Tags::new());

        assert_eq!(visibility.scope_for(&profile), Scope::Trusted);
    }

    #[test]
    fn an_event_no_rule_matches_falls_to_the_default() {
        let visibility = Visibility {
            rules: vec![rule(
                Filter::new().add_kinds([profile::KIND]),
                Scope::Public,
            )],
            default: Scope::Nothing,
        };

        let note = event(author(1), 1, 1, "", Tags::new());

        assert_eq!(visibility.scope_for(&note), Scope::Nothing);
    }

    #[test]
    fn the_defaults_are_the_documents() {
        let visibility = Visibility::default();

        let mutes = event(author(1), MUTE, 1, "", Tags::new());
        let trust = event(author(1), TRUST, 1, "", Tags::new());
        let block = event(author(1), BLOCK, 1, "", Tags::new());
        let profile = event(author(1), profile::KIND, 1, "", Tags::new());
        let note = event(author(1), 1, 1, "", Tags::new());

        assert_eq!(visibility.scope_for(&mutes), Scope::Trusted);
        assert_eq!(visibility.scope_for(&trust), Scope::Trusted);
        assert_eq!(visibility.scope_for(&block), Scope::Trusted);
        assert_eq!(visibility.scope_for(&profile), Scope::Public);
        assert_eq!(visibility.scope_for(&note), Scope::Public);
    }

    #[test]
    fn a_rule_can_match_on_anything_a_filter_can() {
        let visibility = Visibility {
            rules: vec![rule(
                Filter::new().add_tag(TagMatch::Any, "t", "work"),
                Scope::Trusted,
            )],
            default: Scope::Public,
        };

        let work = event(author(1), 1, 1, "", Tags::new().add("t", ["work"]));
        let play = event(author(1), 1, 1, "", Tags::new().add("t", ["play"]));

        assert_eq!(visibility.scope_for(&work), Scope::Trusted);
        assert_eq!(visibility.scope_for(&play), Scope::Public);
    }
}
