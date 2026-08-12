//! The `WHERE` clause a query compiles to, and the parameters it binds.
//!
//! One type, because the two cannot be built apart: a placeholder is numbered
//! by its parameter's position in the list, so a clause written without the
//! list in hand names the wrong value — and SQLite will run it anyway. Every
//! clause here takes its numbers from [`Conditions::bind`] rather than counting
//! for itself.
//!
//! `0` is how "matches nothing" is written. It comes up wherever a constraint
//! names the empty set, which is a different thing from the absent constraint
//! that matches everything.

use rusqlite::types::Value;

/// A `WHERE` fragment and the parameters it binds, kept together so the two
/// cannot drift apart.
#[derive(Debug, Default)]
pub(crate) struct Conditions {
    clauses: Vec<String>,
    params: Vec<Value>,
}

impl Conditions {
    /// No constraint at all, which matches everything.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The parameters, in the order their placeholders name them.
    pub(crate) fn params(&self) -> &[Value] {
        &self.params
    }

    /// The `WHERE` clause these compile to, empty when they constrain nothing.
    ///
    /// Clauses are ANDed, which is what a filter's fields mean: an author and a
    /// window that do not overlap match nothing rather than either one.
    pub(crate) fn where_clause(&self) -> String {
        if self.clauses.is_empty() {
            return String::new();
        }

        format!("WHERE {}", self.clauses.join(" AND "))
    }

    /// Bind one value, and return the placeholder number naming it.
    pub(crate) fn bind(&mut self, value: Value) -> usize {
        self.params.push(value);
        self.params.len()
    }

    /// Bind several values, and return the placeholder list naming them —
    /// `?3, ?4, ?5` — ready to drop into an `IN`.
    ///
    /// Empty in, empty out, which is not valid SQL. Callers that can be handed
    /// an empty set go through [`push_set`](Self::push_set) or
    /// [`push_excluded`](Self::push_excluded), which decide what it means
    /// first.
    pub(crate) fn bind_all(&mut self, values: impl IntoIterator<Item = Value>) -> String {
        let first = self.params.len() + 1;

        self.params.extend(values);

        (first..=self.params.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Add a finished clause, whose placeholders are already bound.
    pub(crate) fn push(&mut self, clause: impl Into<String>) {
        self.clauses.push(clause.into());
    }

    /// Add the clause nothing satisfies.
    pub(crate) fn push_never(&mut self) {
        self.push("0");
    }

    /// Add a set-membership clause. An empty set matches nothing, which SQL
    /// cannot express as `IN ()` — and which is a different thing from the
    /// absent constraint that matches everything.
    pub(crate) fn push_set(&mut self, column: &str, values: Vec<Value>) {
        if values.is_empty() {
            self.push_never();
            return;
        }

        let placeholders = self.bind_all(values);

        self.push(format!("{column} IN ({placeholders})"));
    }

    /// Add a clause excluding a set. The mirror of [`push_set`](Self::push_set):
    /// an empty set excludes nobody, so it constrains nothing and adds no
    /// clause.
    pub(crate) fn push_excluded(&mut self, column: &str, values: Vec<Value>) {
        if values.is_empty() {
            return;
        }

        let placeholders = self.bind_all(values);

        self.push(format!("{column} NOT IN ({placeholders})"));
    }
}

/// A text parameter.
pub(crate) fn text(value: impl AsRef<str>) -> Value {
    Value::Text(value.as_ref().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_constrains_nothing() {
        let conditions = Conditions::new();

        assert_eq!(conditions.where_clause(), "");
        assert!(conditions.params().is_empty());
    }

    #[test]
    fn placeholders_keep_their_place_across_mixed_binds() {
        let mut conditions = Conditions::new();

        let first = conditions.bind(text("a"));
        conditions.push(format!("x = ?{first}"));

        let set = conditions.bind_all([text("b"), text("c")]);
        conditions.push(format!("y IN ({set})"));

        let last = conditions.bind(Value::Integer(4));
        conditions.push(format!("z = ?{last}"));

        assert_eq!(
            conditions.where_clause(),
            "WHERE x = ?1 AND y IN (?2, ?3) AND z = ?4"
        );

        // Which is the invariant: the nth placeholder names the nth parameter.
        assert_eq!(
            conditions.params(),
            [text("a"), text("b"), text("c"), Value::Integer(4)]
        );
    }

    #[test]
    fn an_empty_set_means_opposite_things_to_include_and_exclude() {
        let mut membership = Conditions::new();
        membership.push_set("x", Vec::new());
        assert_eq!(membership.where_clause(), "WHERE 0");

        // Excluding nobody is not a constraint, so it leaves no clause behind.
        let mut exclusion = Conditions::new();
        exclusion.push_excluded("x", Vec::new());
        assert_eq!(exclusion.where_clause(), "");

        assert!(membership.params().is_empty());
        assert!(exclusion.params().is_empty());
    }
}
