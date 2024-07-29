use crate::db;
use crate::db::upsert_zap;
use crate::db::Zap;
use crate::State;
use crate::MAIN_KEY_NAME;
use crate::NONCE_KEY_NAME;
use anyhow::bail;
use anyhow::Context;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use bitcoin::hashes::sha256;
use bitcoin::hashes::Hash;
use lightning_invoice::Bolt11Invoice;
use lnurl::pay::PayResponse;
use lnurl::Tag;
use nostr::event;
use nostr::Event;
use nostr::EventId;
use nostr::JsonUtil;
use nostr::ToBech32;
use serde::de;
use serde::Deserialize;
use serde::Deserializer;
use serde_json::json;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use tonic_openssl_lnd::lnrpc;

pub(crate) async fn get_invoice_impl(
    state: State,
    amount_msats: u64,
    zap_request: Option<Event>,
) -> anyhow::Result<String> {
    let mut lnd = state.lightning_client.clone();
    let zap_request = match zap_request.as_ref() {
        // TODO: Maybe we should get rid of this branch altogether.
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
            // TODO: Validate as valid zap request.
            nostr::Kind::ZapRequest => event,
            _ => bail!("Invalid Nostr event: not a zap request"),
        },
    };

    // TODO: Check if the user has a Lightning address configured.

    let zapped_note_id = get_zapped_note_id(zap_request)?
        .to_bech32()
        .expect("valid note ID");

    let multiplier_note = match state.multipliers.get_multiplier_note(&zapped_note_id) {
        Some(multiplier_note) => multiplier_note,
        None => {
            bail!("Zapped note which wasn't a multiplier note");
        }
    };

    if amount_msats > multiplier_note.multiplier.get_max_amount_sat() * 1000 {
        bail!("Zapped amount ({amount_msats} msat) is too high for the multiplier {}.", multiplier_note.multiplier.get_content());
    }

    // TODO: Must commit to a lot more things to avoid forged fraud proofs.
    let invoice = lnrpc::Invoice {
        value_msat: amount_msats as i64,
        description_hash: sha256::Hash::hash(zap_request.as_json().as_bytes())
            .to_byte_array()
            .to_vec(),
        // TODO: expire when the round ends.
        expiry: 60 * 5,
        memo: format!(
            "Bet {} sats that you will roll a number smaller than {}, \
                 to multiply your wager by {}. nostr:{}",
            amount_msats * 1_000,
            multiplier_note.multiplier.get_lower_than(),
            multiplier_note.multiplier.get_content(),
            zapped_note_id
        ),
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
        receipt_id: None,
        payout_id: None,
    };

    let round = db::get_current_round(&state.db)?.context("No active dice roll at the moment!")?;

    upsert_zap(&state.db, round.event_id, hex::encode(resp.r_hash), zap)?;

    Ok(resp.payment_request)
}

fn get_zapped_note_id(zap_request: &Event) -> anyhow::Result<EventId> {
    let tags = zap_request.tags();
    let tags = tags
        .iter()
        .filter_map(|tag| match tag.as_standardized() {
            Some(event::TagStandard::Event { event_id, .. }) => Some(*event_id),
            _ => None,
        })
        .collect::<Vec<_>>();

    let zapped_note = tags
        // first is ok here, because there should only be one event (if any)
        .first()
        .context("can only accept zaps on notes.")?;

    Ok(*zapped_note)
}

pub async fn get_invoice(
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

    match get_invoice_impl(state, amount_msats, zap_request).await {
        Ok(invoice) => Ok(Json(json!({
            "pr": invoice,
            "routers": []
        }))),
        Err(e) => {
            tracing::error!("Failed to get invoice: {e:#}");
            Err(handle_anyhow_error(e))
        }
    }
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
    let callback = format!("https://{}/get-invoice/{}", state.domain, hex::encode(hash));

    let pk = state.main_keys.public_key();
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
        ]),
        relays: HashMap::from([
            (state.main_keys.public_key().to_hex(), state.relays.clone()),
            (state.nonce_keys.public_key().to_hex(), state.relays.clone()),
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
