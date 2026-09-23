//! Focused historical policy/admission tests; no global card-text mutation.
use std::collections::BTreeMap;

use engine::database::historical_card_text::{
    oracle_text_for_format, resolve_card_face_for_format, CLASSIC_ILLUSIONARY_MASK_TEXT,
    CLASSIC_TIME_VAULT_TEXT,
};
use engine::database::historical_printing::{validate_deck_with_printings, HistoricalPrinting};
use engine::database::CardDatabase;
use engine::game::deck_validation::{
    evaluate_deck_compatibility, validate_deck_for_format, DeckCompatibilityRequest,
};
use engine::types::custom_format::{
    custom_format_registry, passes_legacy_axis_gate, passes_reprint_fidelity_gate,
    swedish_old_school, CardTextPolicy, HistoricalPrintingPolicy, LegacyRuleSet,
};
use engine::types::format::{FormatConfig, SelectedFormat};

fn database() -> CardDatabase {
    let mut cards = serde_json::Map::new();
    for name in [
        "Plains",
        "Island",
        "Black Lotus",
        "Jeweled Bird",
        "Time Vault",
        "Illusionary Mask",
    ] {
        let basic = matches!(name, "Plains" | "Island");
        cards.insert(name.to_lowercase(), serde_json::json!({
            "name": name,
            "mana_cost": {"type": "NoCost"},
            "card_type": {
                "supertypes": if basic { vec!["Basic"] } else { vec![] },
                "core_types": if basic { vec!["Land"] } else { vec!["Artifact"] },
                "subtypes": []
            },
            "power": null, "toughness": null, "loyalty": null, "defense": null,
            "oracle_text": if name == "Jeweled Bird" {
                "Remove this card from your deck before playing if you're not playing for ante."
            } else {
                "Modern database text."
            }, "non_ability_text": null, "flavor_name": null,
            "keywords": [], "abilities": [], "triggers": [], "static_abilities": [], "replacements": [],
            "color_override": null, "scryfall_oracle_id": null, "legalities": {},
            "printings": ["LEA", "LEB", "2ED", "ARN", "ATQ", "LEG", "DRK", "SUM", "3ED", "4ED", "CEI", "CED", "M30"]
        }));
    }
    CardDatabase::from_json_str(&serde_json::Value::Object(cards).to_string()).unwrap()
}

fn request() -> DeckCompatibilityRequest {
    DeckCompatibilityRequest {
        main_deck: vec!["Plains".into(); 60],
        selected_format: Some(SelectedFormat::Resolved(Box::new(
            FormatConfig::for_custom_rules(&swedish_old_school().rules),
        ))),
        player_count: 2,
        ..Default::default()
    }
}

fn printing(card_name: &str, set_code: &str, language: &str) -> HistoricalPrinting {
    HistoricalPrinting {
        card_name: card_name.into(),
        set_code: set_code.into(),
        language: language.into(),
    }
}

#[test]
fn swedish_checks_selected_editions_and_language_including_basic_lands() {
    let db = database();
    let request = request();
    let ids = vec!["selected".into(); 60];
    for set in ["LEA", "LEB", "2ED", "ARN", "ATQ", "LEG", "DRK", "SUM"] {
        let catalog = BTreeMap::from([("selected".into(), printing("Plains", set, "en"))]);
        assert_eq!(
            validate_deck_with_printings(&db, &request, &ids, &catalog),
            Ok(()),
            "{set}"
        );
    }
    // Every card NAME still has an Alpha printing; selecting a reprint must fail.
    for (set, language) in [
        ("3ED", "en"),
        ("4ED", "en"),
        ("CED", "en"),
        ("CEI", "en"),
        ("M30", "en"),
        ("LEG", "it"),
        ("LEA", ""),
        ("", "en"),
    ] {
        let catalog = BTreeMap::from([("selected".into(), printing("Plains", set, language))]);
        assert!(
            validate_deck_with_printings(&db, &request, &ids, &catalog).is_err(),
            "{set}/{language}"
        );
    }
}

#[test]
fn selected_printings_must_cover_all_copies_and_match_catalog_identity() {
    let db = database();
    let request = request();
    let ids = vec!["selected".into(); 60];
    let mut catalog = BTreeMap::from([("selected".into(), printing("Plains", "LEA", "en"))]);
    assert!(validate_deck_with_printings(&db, &request, &ids[..59], &catalog).is_err());
    assert!(
        validate_deck_with_printings(&db, &request, &vec!["selected".into(); 61], &catalog)
            .is_err()
    );
    assert!(
        validate_deck_with_printings(&db, &request, &vec!["missing".into(); 60], &catalog).is_err()
    );
    catalog.insert("selected".into(), printing("Island", "LEA", "en"));
    assert!(validate_deck_with_printings(&db, &request, &ids, &catalog).is_err());
}

#[test]
fn swedish_name_only_full_and_summary_paths_fail_closed() {
    let db = database();
    let mut request = request();
    assert!(validate_deck_for_format(&db, &request)
        .unwrap_err()
        .iter()
        .any(|reason| reason.contains("Selected printing")));
    for summary in [false, true] {
        request.summary_only = summary;
        let result = evaluate_deck_compatibility(&db, &request);
        assert_eq!(result.selected_format_compatible, Some(false));
        assert!(result
            .selected_format_reasons
            .iter()
            .any(|reason| reason.contains("Selected printing")));
    }
}

#[test]
fn printed_admission_keeps_sideboard_restrictions_ante_and_deck_size_checks() {
    let db = database();
    let catalog = BTreeMap::from([
        ("land".into(), printing("Plains", "LEA", "en")),
        ("lotus".into(), printing("Black Lotus", "LEA", "en")),
        ("bird".into(), printing("Jeweled Bird", "ARN", "en")),
        ("reprint".into(), printing("Black Lotus", "3ED", "en")),
    ]);
    let mut request = request();
    request.sideboard = vec!["Black Lotus".into()];
    let mut ids = vec!["land".into(); 60];
    ids.push("lotus".into());
    assert_eq!(
        validate_deck_with_printings(&db, &request, &ids, &catalog),
        Ok(())
    );
    ids[60] = "reprint".into();
    assert!(validate_deck_with_printings(&db, &request, &ids, &catalog).is_err());
    ids[60] = "lotus".into();
    request.main_deck[0] = "Black Lotus".into();
    ids[0] = "lotus".into();
    assert!(validate_deck_with_printings(&db, &request, &ids, &catalog).is_err());
    request.main_deck[0] = "Plains".into();
    ids[0] = "land".into();
    request.sideboard[0] = "Jeweled Bird".into();
    ids[60] = "bird".into();
    assert!(validate_deck_with_printings(&db, &request, &ids, &catalog).is_err());
    request.sideboard.clear();
    ids.pop();
    request.main_deck.pop();
    ids.pop();
    assert!(validate_deck_with_printings(&db, &request, &ids, &catalog).is_err());
}

#[test]
fn classic_text_is_format_scoped_and_does_not_mutate_database_faces() {
    let db = database();
    let modern = FormatConfig::standard();
    let mut rules = swedish_old_school().rules;
    rules.legality.legacy.card_text = CardTextPolicy::ClassicMagic;
    let classic = FormatConfig::for_custom_rules(&rules);
    for (name, expected) in [
        ("Time Vault", CLASSIC_TIME_VAULT_TEXT),
        ("Illusionary Mask", CLASSIC_ILLUSIONARY_MASK_TEXT),
    ] {
        let face = db.get_face_by_name(name).unwrap();
        assert_eq!(oracle_text_for_format(face, &classic), Some(expected));
        assert_eq!(
            oracle_text_for_format(face, &modern),
            Some("Modern database text.")
        );
        assert_eq!(resolve_card_face_for_format(face, &modern).unwrap(), *face);
        assert!(resolve_card_face_for_format(face, &classic).is_err());
        assert_eq!(face.oracle_text.as_deref(), Some("Modern database text."));
    }
    let plains = db.get_face_by_name("Plains").unwrap();
    assert_eq!(
        oracle_text_for_format(plains, &classic),
        plains.oracle_text.as_deref()
    );
    assert!(CLASSIC_TIME_VAULT_TEXT.contains("Skip your next turn:"));
    assert!(CLASSIC_TIME_VAULT_TEXT.contains("Remove a time counter"));
    assert!(CLASSIC_ILLUSIONARY_MASK_TEXT.contains("0/1"));
    assert!(CLASSIC_ILLUSIONARY_MASK_TEXT.contains("removing all mask counters"));
}

#[test]
fn classic_admission_stays_closed_independently_of_combat_and_printing() {
    let db = database();
    let mut request = request();
    let mut rules = swedish_old_school().rules;
    // Deliberately modern damage timing: card text alone must close the gate.
    rules.legality.legacy.card_text = CardTextPolicy::ClassicMagic;
    assert!(!passes_legacy_axis_gate(&rules.legality.legacy));
    let config = FormatConfig::for_custom_rules(&rules);
    assert!(
        serde_json::from_value::<FormatConfig>(serde_json::to_value(&config).unwrap()).is_err()
    );
    request.selected_format = Some(SelectedFormat::Resolved(Box::new(config)));
    let ids = vec!["land".into(); 60];
    let catalog = BTreeMap::from([("land".into(), printing("Plains", "LEA", "en"))]);
    assert!(validate_deck_for_format(&db, &request).is_err());
    assert!(validate_deck_with_printings(&db, &request, &ids, &catalog).is_err());
}

#[test]
fn old_payload_defaults_and_swedish_registry_withholding_are_preserved() {
    let mut old_payload = serde_json::to_value(LegacyRuleSet::default()).unwrap();
    old_payload
        .as_object_mut()
        .unwrap()
        .remove("printing_policy");
    old_payload.as_object_mut().unwrap().remove("card_text");
    let legacy: LegacyRuleSet = serde_json::from_value(old_payload).unwrap();
    assert_eq!(legacy.printing_policy, HistoricalPrintingPolicy::Any);
    assert_eq!(legacy.card_text, CardTextPolicy::Oracle);
    let preset = swedish_old_school();
    assert!(passes_reprint_fidelity_gate(&preset));
    assert!(!custom_format_registry()
        .iter()
        .any(|registered| registered.rules.id == preset.rules.id));
    let config = FormatConfig::for_custom_rules(&preset.rules);
    assert_eq!(
        serde_json::from_value::<FormatConfig>(serde_json::to_value(&config).unwrap()).unwrap(),
        config
    );
}
