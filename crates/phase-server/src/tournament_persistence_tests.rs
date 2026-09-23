use super::*;
use lobby_broker::tournament::{BracketShape, MatchArity, ScoringPolicy};
use lobby_broker::{LobbyClientMessage as C, LobbyServerMessage as S};

fn create_message() -> C {
    C::CreateTournament {
        name: "Durable tournament".into(),
        arity: MatchArity::HEAD_TO_HEAD,
        scoring: Some(ScoringPolicy::default_for_arity(MatchArity::HEAD_TO_HEAD)),
        bracket: BracketShape::Swiss,
        total_rounds: Some(1),
        plus_rounds: None,
        format: None,
        match_type: None,
    }
}

#[test]
fn native_tournament_authority_survives_reopen_before_success_is_published() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let db =
        persistence::GameDb::open(file.path(), persistence::SessionRetention::Multiplayer).unwrap();
    let mut broker = Broker::new();
    let mut conn = ConnState::default();
    let out = handle_durable_tournament(&mut broker, &mut conn, create_message(), &db).unwrap();
    let (code, organizer_token) = out
        .iter()
        .find_map(|out| match out {
            Outbound::ToSelf(S::TournamentCreated {
                code,
                organizer_token,
                ..
            }) => Some((code.clone(), organizer_token.clone())),
            _ => None,
        })
        .expect("published durable creation credential");
    for player in ["Alice", "Bob"] {
        handle_durable_tournament(
            &mut broker,
            &mut conn,
            C::JoinTournament {
                code: code.clone(),
                player_key: player.into(),
                display_name: player.into(),
            },
            &db,
        )
        .unwrap();
    }
    handle_durable_tournament(
        &mut broker,
        &mut conn,
        C::StartTournamentRound {
            code: code.clone(),
            organizer_token: organizer_token.clone(),
            request_id: None,
        },
        &db,
    )
    .unwrap();
    assert_eq!(broker.tournaments().get(&code).unwrap().pairings.len(), 1);
    let renewal = C::RenewTournamentCredential {
        code: code.clone(),
        role: lobby_broker::tournament::TournamentRole::Organizer,
        token: organizer_token,
        rotation_nonce: "restart-retry".into(),
    };
    let rotated = handle_durable_tournament(&mut broker, &mut conn, renewal.clone(), &db).unwrap();
    let rotated = rotated
        .into_iter()
        .find_map(|out| match out {
            Outbound::ToSelf(message @ S::TournamentCredentialRenewed { .. }) => {
                Some(serde_json::to_value(message).unwrap())
            }
            _ => None,
        })
        .expect("rotation reply");
    let expected = serde_json::to_value(broker.tournaments()).unwrap();
    drop(db);
    let db =
        persistence::GameDb::open(file.path(), persistence::SessionRetention::Multiplayer).unwrap();
    let restored = db.load_tournaments().unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), expected);
    let mut broker = Broker::new();
    *broker.tournaments_mut() = restored;
    let retried =
        handle_durable_tournament(&mut broker, &mut ConnState::default(), renewal, &db).unwrap();
    let retried = retried
        .into_iter()
        .find_map(|out| match out {
            Outbound::ToSelf(message @ S::TournamentCredentialRenewed { .. }) => {
                Some(serde_json::to_value(message).unwrap())
            }
            _ => None,
        })
        .expect("rotation retry after restart");
    assert_eq!(retried, rotated);
}

#[test]
fn native_tournament_write_failure_rolls_back_without_success_outbounds() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let db =
        persistence::GameDb::open(file.path(), persistence::SessionRetention::Multiplayer).unwrap();
    let mut broker = Broker::new();
    let mut conn = ConnState::default();
    rusqlite::Connection::open(file.path())
        .unwrap()
        .execute("DROP TABLE native_tournaments", [])
        .unwrap();
    assert!(handle_durable_tournament(&mut broker, &mut conn, create_message(), &db).is_err());
    assert!(broker.tournaments().is_empty());
    assert_eq!(conn, ConnState::default());
}

#[test]
fn native_tournament_corrupt_authority_is_not_replaced_with_empty_state() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let db =
        persistence::GameDb::open(file.path(), persistence::SessionRetention::Multiplayer).unwrap();
    rusqlite::Connection::open(file.path())
        .unwrap()
        .execute("INSERT INTO native_tournaments VALUES (1, 'corrupt')", [])
        .unwrap();
    assert!(db.load_tournaments().is_err());
}
