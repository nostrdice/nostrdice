use crate::db;
use crate::db::upsert_zap;
use crate::db::BetState;
use crate::db::Zap;
use crate::multiplier::MultiplierNote;
use crate::nonce::get_active_nonce;
use crate::nonce::nonce_commitment;
use crate::utils;
use crate::State;
use crate::MAIN_KEY_NAME;
use crate::NONCE_KEY_NAME;
use crate::SOCIAL_KEY_NAME;
use anyhow::bail;
use anyhow::Context;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use lightning_invoice::Bolt11Invoice;
use lnurl::pay::PayResponse;
use lnurl::Tag;
use nostr::bitcoin::hashes::sha256;
use nostr::Event;
use nostr::JsonUtil;
use nostr::ToBech32;
use nostr_sdk::hashes::Hash;
use nostr_sdk::EventId;
use nostr_sdk::PublicKey;
use serde::de;
use serde::Deserialize;
use serde::Deserializer;
use serde_json::json;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use time::OffsetDateTime;
use tonic_openssl_lnd::lnrpc;

/// Returns an invoice if a user wants to play a game
pub async fn get_invoice_for_game(
    Query(params): Query<HashMap<String, String>>,
    Extension(state): Extension<State>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (amount_msats, zap_request) = match params.get("amount").and_then(|a| a.parse::<u64>().ok())
    {
        None => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "ERROR",
                "reason": "Missing amount parameter",
            })),
        )),
        Some(amount_msats) => {
            let zap_request = params.get("nostr").map_or_else(
                || Ok(None),
                |event_str| {
                    Event::from_json(event_str)
                        .map_err(|_| {
                            (
                                StatusCode::BAD_REQUEST,
                                Json(json!({
                                    "status": "ERROR",
                                    "reason": "Invalid zap request",
                                })),
                            )
                        })
                        .map(Some)
                },
            )?;

            Ok((amount_msats, zap_request))
        }
    }?;

    match get_invoice_for_game_impl(state, amount_msats, zap_request).await {
        Ok(invoice) => Ok(Json(json!({
            "pr": invoice,
            "routers": []
        }))),
        Err(e) => {
            tracing::error!("Failed to get invoice for game zap: {e:#}");
            Err(handle_anyhow_error(e))
        }
    }
}

/// Returns an invoice if a user wants to zap us for donation reasons
pub async fn get_invoice_for_zap(
    Query(params): Query<HashMap<String, String>>,
    Extension(state): Extension<State>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (amount_msats, zap_request) = match params.get("amount").and_then(|a| a.parse::<u64>().ok())
    {
        None => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "ERROR",
                "reason": "Missing amount parameter",
            })),
        )),
        Some(amount_msats) => {
            let zap_request = params.get("nostr").map_or_else(
                || Ok(None),
                |event_str| {
                    Event::from_json(event_str)
                        .map_err(|_| {
                            (
                                StatusCode::BAD_REQUEST,
                                Json(json!({
                                    "status": "ERROR",
                                    "reason": "Invalid zap request",
                                })),
                            )
                        })
                        .map(Some)
                },
            )?;

            Ok((amount_msats, zap_request))
        }
    }?;

    match get_invoice_for_zap_impl(state, amount_msats, zap_request).await {
        Ok(invoice) => Ok(Json(json!({
            "pr": invoice,
            "routers": []
        }))),
        Err(e) => {
            tracing::error!("Failed to get invoice for normal zap: {e:#}");
            Err(handle_anyhow_error(e))
        }
    }
}

/// The roller's zap invoice memo specifies the terms of the bet.
///
/// The roller can verify the terms of the bet before sending the
/// payment. To do this, they must:
///
/// - Check that the `multiplier_note_id` corresponds to their chosen multiplier.
///
/// - Check that the `roller_npub` matches their own npub.
///
/// - Check that the `memo_hash` matches the hash of their zap memo.
fn zap_invoice_memo(
    nonce_commitment_note_id: EventId,
    nonce_commitment: sha256::Hash,
    multiplier_note: MultiplierNote,
    roller_npub: PublicKey,
    zap_memo: String,
    amount_msats: u64,
    index: usize,
) -> String {
    let nonce_commitment_note_id = nonce_commitment_note_id.to_bech32().expect("valid note");

    let multiplier_note_id = multiplier_note.note_id;

    let roller_npub = roller_npub.to_bech32().expect("valid npub");

    let memo_hash = sha256::Hash::hash(zap_memo.as_bytes());

    format!(
        "Bet {} sats that you will roll a number smaller than {}, \
         to multiply your wager by {}. nonce_commitment_note_id: {nonce_commitment_note_id}, \
         nonce_commitment: {nonce_commitment}, multiplier_note_id: {multiplier_note_id}, \
         roller_npub: {roller_npub}, memo_hash: {memo_hash}, index: {index}",
        amount_msats / 1_000,
        multiplier_note.multiplier.get_lower_than(),
        multiplier_note.multiplier.get_content(),
    )
}

pub(crate) async fn get_invoice_for_game_impl(
    state: State,
    amount_msats: u64,
    zap_request: Option<Event>,
) -> anyhow::Result<String> {
    let mut lnd = state.lightning_client.clone();
    let zap_request = match zap_request.as_ref() {
        // TODO: Maybe we should get rid of this branch altogether.
        None => bail!("Cannot play the game without a zap request"),
        Some(event) => match event.kind() {
            // TODO: Validate as valid zap request.
            nostr::Kind::ZapRequest => event,
            _ => bail!("Invalid Nostr event: not a zap request"),
        },
    };

    // TODO: Check if the user has a Lightning address configured.

    let zapped_note_id = utils::get_zapped_note_id(zap_request)?
        .to_bech32()
        .expect("valid note ID");

    let multiplier_note = match state.multipliers.get_multiplier_note(&zapped_note_id) {
        Some(multiplier_note) => multiplier_note,
        None => {
            bail!("Zapped note which wasn't a multiplier note");
        }
    };

    if amount_msats > multiplier_note.multiplier.get_max_amount_sat() * 1000 {
        bail!(
            "Zapped amount ({amount_msats} msat) is too high for the multiplier {}.",
            multiplier_note.multiplier.get_content()
        );
    }

    // Better check that we are taking bets before adding the zap invoice.
    let round = get_active_nonce(&state.db)
        .await?
        .context("Cannot accept zap without active nonce")?;

    // TODO: we could run into a race condition calculating the index, if the user would try to zap
    // very fast multiple times.
    let zaps = db::get_zaps_by_event_id(&state.db, round.event_id).await?;

    let index = zaps
        .iter()
        .filter(|z| z.roller == zap_request.pubkey)
        .collect::<Vec<_>>()
        .len();

    let memo = zap_invoice_memo(
        round.event_id,
        nonce_commitment(round.nonce),
        multiplier_note.clone(),
        zap_request.author(),
        zap_request.content.clone(),
        amount_msats,
        index,
    );
    let invoice = lnrpc::Invoice {
        value_msat: amount_msats as i64,
        // Once an active nonce has expired, this is how long it will take us to reveal it.
        expiry: state.reveal_nonce_after_secs as i64,
        memo,
        private: state.route_hints,
        ..Default::default()
    };

    let resp = lnd.add_invoice(invoice).await?.into_inner();

    let invoice = Bolt11Invoice::from_str(&resp.payment_request)?;

    let zap = Zap {
        roller: zap_request.pubkey,
        invoice,
        request: zap_request.clone(),
        multiplier_note_id: multiplier_note.note_id,
        nonce_commitment_note_id: round.event_id,
        bet_state: BetState::GameZapInvoiceRequested,
        zap_retries: 0,
        index,
        bet_timestamp: OffsetDateTime::now_utc(),
    };

    // At this stage, this `Zap` indicates the roller's _intention_ to bet. They have until the zap
    // invoice's expiry to complete the bet.
    upsert_zap(&state.db, hex::encode(resp.r_hash), zap, &state.multipliers).await?;

    Ok(resp.payment_request)
}

pub(crate) async fn get_invoice_for_zap_impl(
    state: State,
    amount_msats: u64,
    zap_request: Option<Event>,
) -> anyhow::Result<String> {
    let mut lnd = state.lightning_client.clone();
    let zap_request = match zap_request.as_ref() {
        None => {
            let request = lnrpc::Invoice {
                value_msat: amount_msats as i64,
                memo: "Donation to NostrDice".to_string(),
                private: state.route_hints,
                ..Default::default()
            };

            let resp = lnd.add_invoice(request).await?.into_inner();

            return Ok(resp.payment_request);
        }
        Some(event) => match event.kind() {
            nostr::Kind::ZapRequest => event,
            _ => bail!("Invalid Nostr event: not a zap request"),
        },
    };

    let invoice = lnrpc::Invoice {
        value_msat: amount_msats as i64,
        description_hash: sha256::Hash::hash(zap_request.as_json().as_bytes())
            .to_byte_array()
            .to_vec(),
        expiry: 60 * 5,
        memo: "Thank you for the donation".to_string(),
        private: state.route_hints,
        ..Default::default()
    };

    let resp = lnd.add_invoice(invoice).await?.into_inner();

    let invoice = Bolt11Invoice::from_str(&resp.payment_request)?;

    let zap = Zap {
        roller: zap_request.pubkey,
        invoice,
        request: zap_request.clone(),
        multiplier_note_id: String::new(),
        nonce_commitment_note_id: EventId::all_zeros(),
        bet_state: BetState::ZapInvoiceRequested,
        zap_retries: 0,
        index: 0,
        bet_timestamp: OffsetDateTime::now_utc(),
    };

    // invoice's expiry to complete the bet.
    upsert_zap(&state.db, hex::encode(resp.r_hash), zap, &state.multipliers).await?;

    Ok(resp.payment_request)
}

pub async fn get_lnurl_pay(
    Path(name): Path<String>,
    Extension(state): Extension<State>,
) -> Result<Json<PayResponse>, (StatusCode, Json<Value>)> {
    let metadata = format!(
        "[[\"text/identifier\",\"{name}@{}\"],[\"text/plain\",\"Sats for {name}\"]]",
        state.domain,
    );

    let hash = sha256::Hash::hash(metadata.as_bytes());

    tracing::debug!("Received request to zap for {name}");

    let (pk, callback_url_path) = match name.as_str() {
        MAIN_KEY_NAME => (state.main_keys.public_key(), "get-invoice-for-game"),
        NONCE_KEY_NAME => (state.nonce_keys.public_key(), "get-invoice-for-zap"),
        SOCIAL_KEY_NAME => (state.social_keys.public_key(), "get-invoice-for-zap"),
        _ => (state.social_keys.public_key(), "get-invoice-for-zap"),
    };

    let callback = format!(
        "https://{}/{}/{}",
        state.domain,
        callback_url_path,
        hex::encode(hash)
    );

    let pk = bitcoin::key::XOnlyPublicKey::from_slice(&pk.serialize()).expect("valid PK");

    let resp = PayResponse {
        callback,
        min_sendable: 1_000,
        max_sendable: 11_000_000_000,
        tag: Tag::PayRequest,
        metadata,
        comment_allowed: None,
        allows_nostr: Some(true),
        nostr_pubkey: Some(pk),
    };

    Ok(Json(resp))
}

pub(crate) fn handle_anyhow_error(err: anyhow::Error) -> (StatusCode, Json<Value>) {
    let err = json!({
        "status": "ERROR",
        "reason": format!("{err}"),
    });
    (StatusCode::BAD_REQUEST, Json(err))
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Nip05QueryParams {
    #[serde(default, deserialize_with = "empty_string_as_none")]
    name: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Nip05Response {
    /// a pair of nip05 username and their corresponding pubkeys in hex format
    pub names: HashMap<String, String>,
    pub relays: HashMap<String, Vec<String>>,
}

pub async fn get_nip05(
    params: Query<Nip05QueryParams>,
    Extension(state): Extension<State>,
) -> Result<Json<Nip05Response>, (StatusCode, Json<Value>)> {
    let all = Nip05Response {
        names: HashMap::from([
            (
                MAIN_KEY_NAME.to_string(),
                state.main_keys.public_key().to_hex(),
            ),
            (
                NONCE_KEY_NAME.to_string(),
                state.nonce_keys.public_key().to_hex(),
            ),
            (
                SOCIAL_KEY_NAME.to_string(),
                state.social_keys.public_key().to_hex(),
            ),
        ]),
        relays: HashMap::from([
            (state.main_keys.public_key().to_hex(), state.relays.clone()),
            (state.nonce_keys.public_key().to_hex(), state.relays.clone()),
            (
                state.social_keys.public_key().to_hex(),
                state.relays.clone(),
            ),
        ]),
    };
    if let Some(name) = &params.name {
        return match name.as_str() {
            MAIN_KEY_NAME => Ok(Json(Nip05Response {
                names: HashMap::from([(
                    MAIN_KEY_NAME.to_string(),
                    state.main_keys.public_key().to_hex(),
                )]),
                relays: HashMap::from([(
                    state.main_keys.public_key().to_hex(),
                    state.relays.clone(),
                )]),
            })),
            NONCE_KEY_NAME => Ok(Json(Nip05Response {
                names: HashMap::from([(
                    NONCE_KEY_NAME.to_string(),
                    state.nonce_keys.public_key().to_hex(),
                )]),
                relays: HashMap::from([(
                    state.nonce_keys.public_key().to_hex(),
                    state.relays.clone(),
                )]),
            })),
            SOCIAL_KEY_NAME => Ok(Json(Nip05Response {
                names: HashMap::from([(
                    SOCIAL_KEY_NAME.to_string(),
                    state.social_keys.public_key().to_hex(),
                )]),
                relays: HashMap::from([(
                    state.social_keys.public_key().to_hex(),
                    state.relays.clone(),
                )]),
            })),
            _ => Ok(Json(all)),
        };
    }
    Ok(Json(all))
}

fn empty_string_as_none<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: fmt::Display,
{
    let opt = Option::<String>::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => FromStr::from_str(s).map_err(de::Error::custom).map(Some),
    }
}
