//! Format-scoped historical text. This module deliberately distinguishes
//! authored text from a runnable CardFace: changing oracle_text alone leaves
//! modern compiled abilities in place and is not a valid rules override.

use crate::types::card::CardFace;
use crate::types::custom_format::CardTextPolicy;
use crate::types::format::FormatConfig;

pub const CLASSIC_MAGIC_TEXT_SOURCE: &str =
    "https://raw.githubusercontent.com/northern-information/lordsofthepit.com/main/src/pages/formats.md";

/// Published Classic Magic text, retaining the source's pre-modern terminology.
pub const CLASSIC_TIME_VAULT_TEXT: &str = "Time Vault comes into play tapped. Time Vault doesn’t untap during your untap step.\nSkip your next turn: Untap Time Vault and put a time counter on it.\nT, Remove a time counter from Time Vault: Take an extra turn after this one. Play this ability if only there’s a time counter on Time Vault.";
pub const CLASSIC_ILLUSIONARY_MASK_TEXT: &str = "X: Put a creature card with converted mana cost X or less from your hand into play face down as a 0/1 creature. Put X mask counters on that creature. Play this ability only any time you could play a sorcery. You may turn the creature face up any time you could play an instant by removing all mask counters from it.";

/// Resolve authoring/display text without mutating the shared Oracle database.
/// This is not a compiled-ability resolver: callers loading playable faces must
/// use `resolve_card_face_for_format` and handle its unsupported-policy error.
pub fn oracle_text_for_format<'a>(face: &'a CardFace, config: &FormatConfig) -> Option<&'a str> {
    if text_policy(config) == CardTextPolicy::ClassicMagic {
        match face.name.as_str() {
            "Time Vault" => return Some(CLASSIC_TIME_VAULT_TEXT),
            "Illusionary Mask" => return Some(CLASSIC_ILLUSIONARY_MASK_TEXT),
            _ => {}
        }
    }
    face.oracle_text.as_deref()
}

fn text_policy(config: &FormatConfig) -> CardTextPolicy {
    config
        .custom_rules
        .as_ref()
        .map_or(CardTextPolicy::Oracle, |rules| {
            rules.legality.legacy.card_text
        })
}

/// Fail closed until both authentic overrides have executable runtime support.
/// Returning a face with historical text and modern compiled abilities would
/// falsely admit a playable Classic deck. The policy gate applies even to a
/// deck without these two cards: runtime copies/Wishes may introduce them.
pub fn resolve_card_face_for_format(
    face: &CardFace,
    config: &FormatConfig,
) -> Result<CardFace, &'static str> {
    match text_policy(config) {
        CardTextPolicy::Oracle => Ok(face.clone()),
        CardTextPolicy::ClassicMagic => Err(
            "Classic Magic card text is unavailable: Time Vault needs a skip-turn activation cost; Illusionary Mask needs 0/1 mask-counter face-down and instant-timing face-up actions",
        ),
    }
}
