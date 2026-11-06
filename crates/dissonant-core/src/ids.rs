//! Where new ids come from.
//!
//! An edit that mints an id (a split clip, a new track) normally gets a random one. In a shared
//! session the same command runs twice: once on the guest, predicting the result, and once on
//! the host, for real. Both have to mint the *same* ids, or a track the guest just added would
//! lose its selection the moment the host confirmed it. So a command can run inside
//! [`scoped`], which derives every id from the operation's own id.

use std::cell::Cell;
use uuid::Uuid;

thread_local! {
    /// The operation being applied on this thread and how many ids it has minted so far.
    static SCOPE: Cell<Option<(Uuid, u64)>> = const { Cell::new(None) };
}

/// A new id: derived from the current operation inside [`scoped`], random otherwise.
pub fn mint() -> Uuid {
    SCOPE.with(|scope| match scope.get() {
        Some((op, n)) => {
            scope.set(Some((op, n + 1)));
            Uuid::new_v5(&op, &n.to_le_bytes())
        }
        None => Uuid::new_v4(),
    })
}

/// Run `f` with every [`mint`] derived from `op`, in order. Scopes nest; the outer one resumes.
pub fn scoped<R>(op: Uuid, f: impl FnOnce() -> R) -> R {
    let outer = SCOPE.with(|scope| scope.replace(Some((op, 0))));
    let out = f();
    SCOPE.with(|scope| scope.set(outer));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_operation_mints_the_same_ids_in_the_same_order() {
        let op = Uuid::new_v4();
        let first = scoped(op, || (mint(), mint()));
        let second = scoped(op, || (mint(), mint()));
        assert_eq!(first, second);
        assert_ne!(first.0, first.1);
    }

    #[test]
    fn outside_a_scope_ids_are_random() {
        assert_ne!(mint(), mint());
    }
}
