//! Typed message boundaries for component composition.
//!
//! `MsgMap` + `Slot` provide zero-clone message routing between parent and child
//! components. Messages are routed by value — no `Clone` required.
//!
//! `Slot` is optional. The primary composition pattern is direct field access:
//! ```ignore
//! self.child.update(msg, ctx).map_msg(ParentMsg::Child)
//! ```
//! Use `Slot` when you want a generic boundary that hides the child type.

use std::marker::PhantomData;

use crate::app::{Command, MouseResponse, Subscription};
use crate::event_context::{CaptureState, KeyEvent, MouseEvent};
use crate::layout_snapshot::{HitResult, LayoutSnapshot};

use super::{Component, Ctx, IdSpace};

/// Maps between a parent message type and a component's local message type.
///
/// Routing is entirely by value — no `Clone` required on either type.
/// `try_unwrap` returns the parent message back on mismatch so it can
/// be routed to other children without cloning.
pub trait MsgMap: 'static {
    type Parent: Send + std::fmt::Debug + 'static;
    type Local: Send + std::fmt::Debug + 'static;

    /// Wrap a local message into the parent message type.
    fn wrap(local: Self::Local) -> Self::Parent;

    /// Try to extract a local message from a parent message.
    /// Returns `Err(parent)` if this message is not for this component.
    fn try_unwrap(parent: Self::Parent) -> Result<Self::Local, Self::Parent>;
}

/// A typed boundary that wraps a child component with message mapping.
///
/// The child is stored as a field — nesting is structural, not dynamic.
pub struct Slot<C: Component, Map: MsgMap<Local = C::Message>> {
    /// The child component instance.
    pub child: C,
    _map: PhantomData<Map>,
}

impl<C, Map> Slot<C, Map>
where
    C: Component,
    Map: MsgMap<Local = C::Message>,
{
    /// Create a new slot wrapping a child component.
    pub fn new(child: C) -> Self {
        Self {
            child,
            _map: PhantomData,
        }
    }

    /// Try to route a parent message to this child.
    ///
    /// Returns `Ok((command, output))` if the message was for this child,
    /// or `Err(parent_msg)` if it wasn't (so it can be tried elsewhere).
    pub fn try_update(
        &mut self,
        msg: Map::Parent,
        ctx: &mut Ctx,
    ) -> Result<(Command<Map::Parent>, C::Output), Map::Parent> {
        match Map::try_unwrap(msg) {
            Ok(local) => {
                let (cmd, output) = self.child.update(local, ctx);
                Ok((cmd.map_msg(Map::wrap), output))
            }
            Err(parent) => Err(parent),
        }
    }

    /// Delegate view rendering to the child.
    pub fn view(&self, snapshot: &mut LayoutSnapshot, ids: IdSpace) {
        self.child.view(snapshot, ids);
    }

    /// Delegate key event to the child, mapping any emitted message.
    pub fn on_key(&self, event: KeyEvent) -> Option<Map::Parent> {
        self.child.on_key(event).map(Map::wrap)
    }

    /// Delegate mouse event to the child, mapping any emitted message.
    pub fn on_mouse(
        &self,
        event: MouseEvent,
        hit: Option<HitResult>,
        capture: &CaptureState,
    ) -> MouseResponse<Map::Parent> {
        self.child.on_mouse(event, hit, capture).map(Map::wrap)
    }

    /// Delegate subscription creation to the child, mapping message types.
    pub fn subscription(&self) -> Subscription<Map::Parent> {
        self.child.subscription().map_msg(Map::wrap)
    }
}
