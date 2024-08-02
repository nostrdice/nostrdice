use crate::db;
use crate::db::BetState;
use crate::db::Zap;
use crate::multiplier::Multiplier;
use crate::multiplier::Multipliers;
use anyhow::Result;
use nostr::EventBuilder;
use nostr::EventId;
use nostr::PublicKey;
use sled::Db;
use time::Duration;
use time::OffsetDateTime;
use tokio::time::sleep;

/// Time window of positing updates on social
const TIME_WINDOW: u64 = 60;

/// Posts updates on nostr every {TIME_WINDOW}minutes.
pub async fn post_social_updates(
    client: nostr_sdk::Client,
    keys: nostr::Keys,
    db: Db,
    multipliers: Multipliers,
    game: PublicKey,
    nonce: PublicKey,
) {
    loop {
        if let Err(err) = post_social_inner(
            client.clone(),
            keys.clone(),
            db.clone(),
            multipliers.clone(),
            game,
            nonce,
        )
        .await
        {
            tracing::error!("Could not post social update {err:#}");
        }
        sleep(tokio::time::Duration::from_secs(TIME_WINDOW * 60)).await;
    }
}

async fn post_social_inner(
    client: nostr_sdk::Client,
    keys: nostr::Keys,
    db: Db,
    multipliers: Multipliers,
    game: PublicKey,
    nonce: PublicKey,
) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let last_announcement_cut_off = now - Duration::minutes(TIME_WINDOW);
    let zaps = db::get_zaps_in_time_window(&db, last_announcement_cut_off, now)?;

    let winners = filter_zaps(&multipliers, &zaps, BetState::PaidWinner);

    if winners.is_empty() {
        tracing::debug!("No winners in this round, not positing anything");
        return Ok(());
    }

    let losers = filter_zaps(&multipliers, &zaps, BetState::Loser);

    let msg = format!("Winner winner, chicken dinner. Thank you for all the participants in the last {} minutes. We had {} participants of which {} won somethings.", TIME_WINDOW, winners.len() + losers.len(), winners.len());
    let closing_message = format!(
        "Follow nostr:{} for another round and nostr:{} for the published nonces",
        game, nonce
    );
    let winners = format_winners(winners);
    let losers = format_losers(losers);

    let msg = format!("{} \n {}\n{}\n{}", msg, winners, losers, closing_message);
    let note_id = publish_note(&client, &keys, msg).await?;
    tracing::debug!("Published game summary: {note_id}",);
    Ok(())
}

fn filter_zaps(
    multipliers: &Multipliers,
    zaps: &[Zap],
    state: BetState,
) -> Vec<(PublicKey, Multiplier, u64)> {
    zaps.iter()
        .filter_map(|zap| {
            if zap.bet_state != state {
                return None;
            }

            let multiplier_note = match multipliers.get_multiplier_note(&zap.multiplier_note_id) {
                Some(multiplier_note) => multiplier_note,
                None => {
                    return None;
                }
            };

            Some((
                zap.roller,
                multiplier_note.multiplier,
                zap.invoice.amount_milli_satoshis().unwrap_or_default(),
            ))
        })
        .collect::<Vec<_>>()
}

fn format_winners(winners: Vec<(PublicKey, Multiplier, u64)>) -> String {
    if winners.is_empty() {
        return String::new();
    }
    let mut message = String::from("Winners:\n");
    for (pubkey, multiplier, amount) in winners {
        message.push_str(&format!(
            "- nostr:{}: won {} x {}sats \n",
            pubkey,
            multiplier.get_multiplier(),
            amount / 1000
        ));
    }
    message
}
fn format_losers(players: Vec<(PublicKey, Multiplier, u64)>) -> String {
    if players.is_empty() {
        return String::new();
    }
    let mut message = String::from("Losers - please try again:\n");
    for (pubkey, _, _) in players {
        message.push_str(&format!("- nostr:{}\n", pubkey,));
    }
    message
}

async fn publish_note(
    client: &nostr_sdk::Client,
    keys: &nostr::Keys,
    msg: String,
) -> Result<EventId> {
    let event = EventBuilder::text_note(msg, []).to_event(keys)?;

    let event_id = client.send_event(event.clone()).await?;

    Ok(event_id)
}
