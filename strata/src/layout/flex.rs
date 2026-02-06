//! Shared flex distribution logic for Column and Row.
//!
//! This module provides axis-agnostic flex layout math, avoiding duplication
//! between Column and Row. The key function is `distribute_flex`, which takes
//! child size/flex info and returns allocated sizes.

use super::context::{FlexAllocation, LayoutContext};
use super::child::LayoutChild;
use super::length::Length;

/// Input for flex distribution: either a fixed size or a flex factor.
#[derive(Debug, Clone, Copy)]
pub enum FlexInput {
    /// Fixed-size child (already measured).
    Fixed(f32),
    /// Flex child with factor (e.g., Fill = 1.0, FillPortion(2) = 2.0).
    Flex(f32),
}

impl FlexInput {
    /// Create from a Length and measured size.
    ///
    /// - Fixed/Shrink → Fixed(measured_size)
    /// - Fill/FillPortion → Flex(factor)
    pub fn from_length(length: Length, measured_size: f32) -> Self {
        match length {
            Length::Fixed(px) => FlexInput::Fixed(px),
            Length::Shrink => FlexInput::Fixed(measured_size),
            Length::Fill => FlexInput::Flex(1.0),
            Length::FillPortion(n) => FlexInput::Flex(n as f32),
        }
    }

    pub fn is_flex(&self) -> bool {
        matches!(self, FlexInput::Flex(_))
    }
}

/// Distribute available space among children using flex factors.
///
/// This is the core layout algorithm shared by Column and Row.
/// It uses the scratch buffer in LayoutContext to avoid allocations.
///
/// # Arguments
/// * `ctx` - Layout context (uses flex_scratch for output)
/// * `inputs` - Iterator of (FlexInput, cross_size) for each child
/// * `available_main` - Total available space on main axis
/// * `spacing` - Space between children
///
/// # Returns
/// Slice of FlexAllocation with allocated main_size for each child.
pub fn distribute_flex<'a>(
    ctx: &'a mut LayoutContext,
    inputs: impl Iterator<Item = FlexInput>,
    available_main: f32,
    spacing: f32,
    child_count: usize,
) -> &'a [FlexAllocation] {
    ctx.flex_scratch.clear();

    // First pass: collect inputs and compute totals
    let mut total_fixed = 0.0f32;
    let mut total_flex = 0.0f32;
    let mut temp_inputs: Vec<FlexInput> = Vec::with_capacity(child_count);

    for input in inputs {
        match input {
            FlexInput::Fixed(size) => {
                total_fixed += size;
            }
            FlexInput::Flex(factor) => {
                total_flex += factor;
            }
        }
        temp_inputs.push(input);
    }

    // Account for spacing
    if temp_inputs.len() > 1 {
        total_fixed += spacing * (temp_inputs.len() - 1) as f32;
    }

    // Calculate space available for flex children
    let available_for_flex = (available_main - total_fixed).max(0.0);

    // Second pass: compute allocations
    for input in temp_inputs {
        let (main_size, is_flex) = match input {
            FlexInput::Fixed(size) => (size, false),
            FlexInput::Flex(factor) => {
                let size = if total_flex > 0.0 {
                    (factor / total_flex) * available_for_flex
                } else {
                    0.0
                };
                (size, true)
            }
        };
        ctx.flex_scratch.push(FlexAllocation { main_size, is_flex });
    }

    &ctx.flex_scratch
}

/// Convenience function to measure a child for flex distribution.
///
/// For Column (is_column=true): measures height, handles containers with height Length.
/// For Row (is_column=false): measures width, handles containers with width Length.
pub fn measure_child_for_flex(
    child: &LayoutChild,
    is_column: bool,
    available_cross: f32,
) -> FlexInput {
    // Get the length for the main axis
    let main_length = child.main_length(is_column);

    // Measure the child's intrinsic size on main axis
    let measured = match child {
        // Special cases that need available_cross for width-first layout
        LayoutChild::Flow(f) => {
            if is_column {
                // Column needs height, which depends on available width
                f.height_for_width(available_cross)
            } else {
                f.measure().width
            }
        }
        LayoutChild::Row(r) => {
            if is_column {
                r.height_for_width(available_cross)
            } else {
                r.measure().width
            }
        }
        LayoutChild::Column(c) => {
            if is_column {
                c.height_for_width(available_cross)
            } else {
                c.measure().width
            }
        }
        // Standard cases
        _ => child.measure_main(is_column),
    };

    FlexInput::from_length(main_length, measured)
}

/// Compute total fixed size (non-flex children) for overflow detection.
pub fn total_fixed_size(
    children: &[LayoutChild],
    is_column: bool,
    available_cross: f32,
    spacing: f32,
) -> f32 {
    let mut total = 0.0f32;

    for child in children {
        let input = measure_child_for_flex(child, is_column, available_cross);
        if let FlexInput::Fixed(size) = input {
            total += size;
        }
    }

    // Add spacing
    if children.len() > 1 {
        total += spacing * (children.len() - 1) as f32;
    }

    total
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_snapshot::LayoutSnapshot;

    #[test]
    fn test_distribute_flex_all_fixed() {
        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        let inputs = vec![
            FlexInput::Fixed(50.0),
            FlexInput::Fixed(30.0),
            FlexInput::Fixed(20.0),
        ];

        let allocations = distribute_flex(&mut ctx, inputs.into_iter(), 200.0, 10.0, 3);

        assert_eq!(allocations.len(), 3);
        assert_eq!(allocations[0].main_size, 50.0);
        assert_eq!(allocations[1].main_size, 30.0);
        assert_eq!(allocations[2].main_size, 20.0);
        assert!(!allocations[0].is_flex);
    }

    #[test]
    fn test_distribute_flex_all_flex() {
        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        let inputs = vec![
            FlexInput::Flex(1.0),
            FlexInput::Flex(1.0),
        ];

        // 200 - 10 spacing = 190 available for flex, split evenly
        let allocations = distribute_flex(&mut ctx, inputs.into_iter(), 200.0, 10.0, 2);

        assert_eq!(allocations.len(), 2);
        assert_eq!(allocations[0].main_size, 95.0);
        assert_eq!(allocations[1].main_size, 95.0);
        assert!(allocations[0].is_flex);
    }

    #[test]
    fn test_distribute_flex_mixed() {
        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        let inputs = vec![
            FlexInput::Fixed(50.0),  // Fixed 50
            FlexInput::Flex(1.0),    // Flex 1
            FlexInput::Flex(2.0),    // Flex 2
        ];

        // 200 - 50 fixed - 20 spacing = 130 for flex
        // Flex 1 gets 130 * 1/3 ≈ 43.33
        // Flex 2 gets 130 * 2/3 ≈ 86.67
        let allocations = distribute_flex(&mut ctx, inputs.into_iter(), 200.0, 10.0, 3);

        assert_eq!(allocations.len(), 3);
        assert_eq!(allocations[0].main_size, 50.0);
        assert!(!allocations[0].is_flex);

        // Check flex allocations sum to 130
        let flex_total = allocations[1].main_size + allocations[2].main_size;
        assert!((flex_total - 130.0).abs() < 0.01);

        // Check proportions
        assert!((allocations[2].main_size / allocations[1].main_size - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_distribute_flex_no_space() {
        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        let inputs = vec![
            FlexInput::Fixed(100.0),
            FlexInput::Flex(1.0),
        ];

        // Only 50 available, fixed takes 100 → flex gets 0
        let allocations = distribute_flex(&mut ctx, inputs.into_iter(), 50.0, 0.0, 2);

        assert_eq!(allocations[0].main_size, 100.0);
        assert_eq!(allocations[1].main_size, 0.0);
    }

    #[test]
    fn test_flex_input_from_length() {
        assert!(matches!(
            FlexInput::from_length(Length::Fixed(100.0), 50.0),
            FlexInput::Fixed(100.0)
        ));
        assert!(matches!(
            FlexInput::from_length(Length::Shrink, 50.0),
            FlexInput::Fixed(50.0)
        ));
        assert!(matches!(
            FlexInput::from_length(Length::Fill, 50.0),
            FlexInput::Flex(1.0)
        ));
        assert!(matches!(
            FlexInput::from_length(Length::FillPortion(3), 50.0),
            FlexInput::Flex(3.0)
        ));
    }
}
