//! Historical printing admission. Catalog records must come from host-owned
//! set data; requests supply only printing IDs, never edition/language claims.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::database::CardDatabase;
use crate::game::deck_validation::{validate_deck_for_format, DeckCompatibilityRequest};
use crate::types::custom_format::HistoricalPrintingPolicy;
use crate::types::format::SelectedFormat;

/// The subset of a trusted printing record needed by the Swedish rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoricalPrinting {
    pub card_name: String,
    pub set_code: String,
    pub language: String,
}

/// Validate every submitted copy, then apply the existing card-pool, ban,
/// restriction, deck-size and sideboard checks. IDs follow main-deck order,
/// then sideboard, commander, companion, signature spell, planar and scheme.
/// Missing/extra IDs and catalog misses fail closed; basic lands have no
/// printing exemption. The caller retains the original format for game play.
pub fn validate_deck_with_printings(
    db: &CardDatabase,
    request: &DeckCompatibilityRequest,
    printing_ids: &[String],
    catalog: &BTreeMap<String, HistoricalPrinting>,
) -> Result<(), Vec<String>> {
    let Some(selected) = &request.selected_format else {
        return validate_deck_for_format(db, request);
    };
    let mut config = selected
        .rules()
        .map_err(|error| vec![error.to_string()])?
        .into_owned();
    let policy = config
        .custom_rules
        .as_ref()
        .map_or(HistoricalPrintingPolicy::Any, |rules| {
            rules.legality.legacy.printing_policy
        });
    if policy == HistoricalPrintingPolicy::Any {
        return validate_deck_for_format(db, request);
    }
    let names: Vec<_> = request
        .main_deck
        .iter()
        .chain(&request.sideboard)
        .chain(&request.commander)
        .chain(&request.companion)
        .chain(&request.signature_spell)
        .chain(&request.planar_deck)
        .chain(&request.scheme_deck)
        .collect();
    if names.len() != printing_ids.len() {
        return Err(vec![
            "A selected printing ID is required for every submitted card copy".into(),
        ]);
    }
    let mut errors = Vec::new();
    for (name, id) in names.iter().zip(printing_ids) {
        let Some(printing) = catalog.get(id) else {
            errors.push(format!("Unknown selected printing: {id}"));
            continue;
        };
        let matches_card = db
            .get_face_by_name(name)
            .zip(db.get_face_by_name(&printing.card_name))
            .is_some_and(|(submitted, printed)| submitted.name == printed.name);
        let known_edition = db.printings_for(name).is_some_and(|sets| {
            sets.iter()
                .any(|set| set.eq_ignore_ascii_case(&printing.set_code))
        });
        let allowed = match policy {
            HistoricalPrintingPolicy::Any => true,
            HistoricalPrintingPolicy::SwedishOriginal => {
                ["LEA", "LEB", "2ED", "ARN", "ATQ", "LEG", "DRK", "SUM"]
                    .iter()
                    .any(|set| set.eq_ignore_ascii_case(&printing.set_code))
                    && printing.language.eq_ignore_ascii_case("en")
            }
        };
        if !matches_card || !known_edition || !allowed {
            errors.push(format!("Illegal selected printing {id} for {name}"));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    // Only this already-proven printing requirement is discharged. Preserve
    // every other rule (especially card-text/combat gates and restricted cards)
    // and run the same authoritative admission function as name-only callers.
    config
        .custom_rules
        .as_mut()
        .expect("historical policy has custom rules")
        .legality
        .legacy
        .printing_policy = HistoricalPrintingPolicy::Any;
    let mut checked = request.clone();
    checked.selected_format = Some(SelectedFormat::Resolved(Box::new(config)));
    validate_deck_for_format(db, &checked)
}
