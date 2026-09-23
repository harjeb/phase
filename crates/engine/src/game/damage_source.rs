//! Incarnation-aware characteristic reads shared by damage and protection.

use crate::game::game_object::GameObject;
use crate::types::card_type::CoreType;
use crate::types::game_state::{GameState, LKISnapshot};
use crate::types::identifiers::ObjectIncarnationRef;
use crate::types::mana::ManaColor;
use crate::types::player::PlayerId;

/// A source's current characteristics or the last known characteristics of
/// exactly the incarnation that dealt damage. Never projects a replacement
/// object stored under the same id into the old source.
#[derive(Clone, Copy, Debug)]
pub enum DamageSourceView<'a> {
    Live(&'a GameObject),
    Lki(&'a LKISnapshot),
}

impl<'a> From<&'a GameObject> for DamageSourceView<'a> {
    fn from(object: &'a GameObject) -> Self {
        Self::Live(object)
    }
}

/// CR 400.7: a zone change creates a different object. Look back by incarnation,
/// not through the id-only LKI cache, which may describe a subsequent object.
/// Callers handling ordinary damage stamp the currently stored incarnation;
/// callers handling a frozen combat assignment must retain its original pin.
pub fn damage_source_view(
    state: &GameState,
    source: ObjectIncarnationRef,
) -> Option<DamageSourceView<'_>> {
    if let Some(object) = state.objects.get(&source.object_id) {
        if object.incarnation == source.incarnation {
            return Some(DamageSourceView::Live(object));
        }
    }
    state
        .lki_by_incarnation
        .get(&source.object_id)
        .and_then(|incarnations| incarnations.get(&source.incarnation))
        .map(DamageSourceView::Lki)
}

impl<'a> DamageSourceView<'a> {
    pub fn controller(self) -> PlayerId {
        match self {
            Self::Live(object) => object.controller,
            Self::Lki(snapshot) => snapshot.controller,
        }
    }

    pub(crate) fn controller_or_owner(self) -> PlayerId {
        match self {
            Self::Live(object) => object.controller_or_owner(),
            Self::Lki(snapshot) => snapshot.controller,
        }
    }

    pub fn effective_colors(self) -> Vec<ManaColor> {
        match self {
            Self::Live(object) => object.effective_colors(),
            Self::Lki(snapshot) => snapshot.colors.clone(),
        }
    }

    pub fn effective_mana_value(self) -> u32 {
        match self {
            Self::Live(object) => object.effective_mana_value(),
            Self::Lki(snapshot) => snapshot.mana_value,
        }
    }

    pub fn core_types(self) -> &'a [CoreType] {
        match self {
            Self::Live(object) => &object.card_types.core_types,
            Self::Lki(snapshot) => &snapshot.card_types,
        }
    }

    pub fn subtypes(self) -> &'a [String] {
        match self {
            Self::Live(object) => &object.card_types.subtypes,
            Self::Lki(snapshot) => &snapshot.subtypes,
        }
    }
}
