//! Component-scoped ID namespacing.
//!
//! `IdSpace` provides zero-allocation, const-fn ID generation for components.
//! Each component receives an `IdSpace` and derives child spaces for sub-components,
//! eliminating `format!()` / `String` churn entirely.
//!
//! Uses splitmix64-style mixing for practically collision-free IDs.

use crate::content_address::SourceId;

/// A component-scoped ID namespace.
///
/// # Usage
///
/// ```ignore
/// const IDS: IdSpace = IdSpace::new(1);
/// const CHILD_IDS: IdSpace = IDS.child(2);
///
/// let button_id: SourceId = IDS.id(0);
/// let child_widget_id: SourceId = CHILD_IDS.id(0);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct IdSpace(u64);

impl IdSpace {
    /// Create a new ID namespace from a seed value.
    pub const fn new(namespace: u64) -> Self {
        Self(namespace)
    }

    /// Derive a child namespace for a nested component.
    ///
    /// Uses splitmix64 mixing over `(self.0, tag)` to avoid structured collisions.
    pub const fn child(self, tag: u64) -> IdSpace {
        IdSpace(Self::mix(self.0, tag))
    }

    /// Produce a `SourceId` for a widget within this namespace.
    ///
    /// Sets the high bit to avoid collision with `SourceId::new()`'s atomic counter.
    pub const fn id(self, widget: u64) -> SourceId {
        SourceId::from_raw(Self::mix(self.0, widget) | (1 << 63))
    }

    /// Splitmix64-style finalizer: mix two u64s into a well-distributed result.
    ///
    /// Branchless, tiny, and avoids the structured collision risk of bare XOR.
    const fn mix(a: u64, b: u64) -> u64 {
        let mut z = a.wrapping_add(b.wrapping_mul(0x9E3779B97F4A7C15));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_namespaces_produce_different_ids() {
        let a = IdSpace::new(1);
        let b = IdSpace::new(2);
        assert_ne!(a.id(0), b.id(0));
    }

    #[test]
    fn different_widgets_produce_different_ids() {
        let ids = IdSpace::new(42);
        assert_ne!(ids.id(0), ids.id(1));
        assert_ne!(ids.id(1), ids.id(2));
    }

    #[test]
    fn child_namespaces_are_distinct() {
        let parent = IdSpace::new(1);
        let child_a = parent.child(1);
        let child_b = parent.child(2);
        assert_ne!(child_a.id(0), child_b.id(0));
    }

    #[test]
    fn ids_have_high_bit_set() {
        let ids = IdSpace::new(0);
        let source_id = ids.id(0);
        assert!(source_id.raw() & (1 << 63) != 0);
    }

    #[test]
    fn nested_children_are_distinct() {
        let root = IdSpace::new(0);
        let child = root.child(1);
        let grandchild = child.child(2);
        // All produce different IDs for widget 0
        assert_ne!(root.id(0), child.id(0));
        assert_ne!(child.id(0), grandchild.id(0));
        assert_ne!(root.id(0), grandchild.id(0));
    }

    #[test]
    fn const_evaluable() {
        // Verify these can be used in const context
        const IDS: IdSpace = IdSpace::new(123);
        const CHILD: IdSpace = IDS.child(456);
        const SID: SourceId = CHILD.id(789);
        assert!(SID.raw() & (1 << 63) != 0);
    }
}
