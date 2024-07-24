use crate::db;
use crate::db::upsert_zap;
use crate::db::Zap;
use crate::State;
use anyhow::anyhow;
use anyhow::bail;
use anyhow::Context;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use bitcoin::hashes::sha256;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::ThirtyTwoByteHash;
use lightning_invoice::Bolt11Invoice;
use lnurl::pay::PayResponse;
use lnurl::Tag;
use nostr::event;
use nostr::Event;
use nostr::EventId;
use nostr::JsonUtil;
use nostr::ToBech32;
use serde_json::json;
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use tonic_openssl_lnd::lnrpc;

pub(crate) async fn get_invoice_impl(
    state: State,
    hash: String,
    amount_msats: u64,
    zap_request: Option<Event>,
) -> anyhow::Result<String> {
    let mut lnd = state.lightning_client.clone();
    let (desc_hash, memo) = match zap_request.as_ref() {
        None => (
            sha256::Hash::from_str(&hash)?,
            "Donation to nostr-dice".to_string(),
        ),
        Some(event) => {
            // todo validate as valid zap request
            if event.kind != nostr::Kind::ZapRequest {
                return Err(anyhow!("Invalid zap request"));
            }

            let dice_roll = db::get_active_dice_roll(&state.db)?
                .context("No active dice roll at the moment!")?;

            // TODO: check if the user has a lightning address configured

            let zapped_note_id = get_note_id(event)?.to_bech32().expect("to fit");

            match dice_roll.get_multiplier_note(zapped_note_id.clone()) {
                Some(multiplier_note) => (
                    sha256::Hash::hash(event.as_json().as_bytes()),
                    format!(
                        "You bet {} your amount on Nostr Dice that the roll is lower than {}, nostr:{}",
                        multiplier_note.multiplier.get_content(),
                        multiplier_note.multiplier.get_lower_than(),
                        zapped_note_id
                    ),
                ),
                None => {
                    bail!("Zapped note which wasn't a multiplier note")
                }
            }
        }
    };

    let request = lnrpc::Invoice {
        value_msat: amount_msats as i64,
        description_hash: desc_hash.into_32().to_vec(),
        // TODO: expire when the round ends.
        expiry: 60 * 5,
        memo,
        private: state.route_hints,
        ..Default::default()
    };

    let resp = lnd.add_invoice(request).await?.into_inner();

    if let Some(zap_request) = zap_request {
        let invoice = Bolt11Invoice::from_str(&resp.payment_request)?;
        let tags = zap_request.tags();
        let tags = tags
            .iter()
            .filter_map(|tag| match tag {
                event::Tag::Event { event_id, .. } => Some(*event_id),
                _ => None,
            })
            .collect::<Vec<_>>();

        let zapped_note = tags
            // first is ok here, because there should only be one event (if any)
            .first()
            .context("can only accept zaps on notes.")?;

        let zap = Zap {
            roller: zap_request.pubkey,
            invoice,
            request: zap_request,
            note_id: zapped_note.to_bech32()?,
            receipt_id: None,
            payout_id: None,
        };

        let dice_roll =
            db::get_active_dice_roll(&state.db)?.context("No active dice roll at the moment!")?;

        upsert_zap(&state.db, dice_roll.event_id, hex::encode(resp.r_hash), zap)?;
    }

    Ok(resp.payment_request)
}

fn get_note_id(zap_request: &Event) -> anyhow::Result<EventId> {
    let tags = zap_request.tags();
    let tags = tags
        .iter()
        .filter_map(|tag| match tag {
            event::Tag::Event { event_id, .. } => Some(*event_id),
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
    Path(hash): Path<String>,
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

    match get_invoice_impl(state, hash, amount_msats, zap_request).await {
        Ok(invoice) => Ok(Json(json!({
            "pr": invoice,
            "routers": []
        }))),
        Err(e) => {
            tracing::error!("Failed to get invoice: {e:#}");
            Err(handle_anyhow_error(e))
        },
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

    let resp = PayResponse {
        callback,
        min_sendable: 1_000,
        max_sendable: 11_000_000_000,
        tag: Tag::PayRequest,
        metadata,
        comment_allowed: None,
        allows_nostr: Some(true),
        nostr_pubkey: Some(*state.keys.public_key()),
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
