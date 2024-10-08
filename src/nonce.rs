use crate::db;
use crate::db::Round;
use crate::db::RoundRow;
use anyhow::Context;
use anyhow::Result;
use nostr::bitcoin::hashes::sha256;
use nostr::Tag;
use nostr_sdk::hashes::Hash;
use nostr_sdk::hashes::HashEngine;
use nostr_sdk::EventBuilder;
use nostr_sdk::EventId;
use nostr_sdk::TagStandard;
use nostr_sdk::ToBech32;
use rand::thread_rng;
use rand::Rng;
use rand::RngCore;
use rand::SeedableRng;
use sqlx::query;
use sqlx::query_as;
use sqlx::SqlitePool;
use std::ops::ControlFlow;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::broadcast;

/// The randomness generated by the server every round.
struct Nonce {
    /// The nonce.
    inner: [u8; 32],
    /// Hash of the nonce.
    commitment: sha256::Hash,
    /// Moment when the nonce was created. Ideally, the nonce
    /// commitment is published at this time.
    created_at: Instant,
    /// A nonce expires this long after creation.
    expire_after: Duration,
    /// A nonce is revealed this long after _expiration_.
    reveal_after: Duration,
}

/// Manage nonce generation, expiration and revelation.
///
/// Steps:
///
/// 1. Check if there was a previous active nonce i.e. a nonce that was not expired before the last
///    restart. If so, expire it and reveal it, triggering relevant payouts. It's hard to know if
///    there was any time left, so it's better to move on.
///
/// 2. Check if there was a previous expired nonce i.e. a nonce that was expired but not revealed
///    before the last restart. If so, reveal it, triggering relevant payouts.
///
/// 3. Generate a new nonce, mark it as the active nonce and publish its nonce commitment. Any new
///    zaps will be linked to this nonce.
///
/// 4. Wait until the active nonce expires.
///
/// 5. After the active nonce expires, spawn a task to reveal the nonce after the scheduled delay.
///    The delay allows for rollers who bet close to nonce expiry to have enough time to pay their
///    invoice.
///
/// 6. Go back to step 3.
///
/// The goal of this flow is to allow rollers to safely bet at any point. If they zap when there is
/// an active nonce, and complete the payment before the zap invoice expires, they will be
/// considered when the payouts are calculated.
pub async fn manage_nonces(
    client: nostr_sdk::Client,
    keys: nostr::Keys,
    db: SqlitePool,
    expire_after_secs: u64,
    reveal_after_secs: u64,
    mut ctrl_c: broadcast::Receiver<()>,
) -> Result<()> {
    // Immediately unset the nonce, so that we do not use a nonce that may have been revealed
    // already. This also ensures that we pay out any winners.
    if let Some(round) = unset_active_nonce(&db).await? {
        // We may have already revealed this nonce before the restart, but doing so again does not
        // hurt.
        if let Err(e) = reveal_nonce(&client, &keys, round.nonce, round.event_id).await {
            tracing::error!(
                nonce = hex::encode(round.nonce),
                "Failed to reveal nonce after restart: {e:#}. Must publish and handle payouts \
                 manually"
            );
        };
    }

    // Ensure that we reveal the latest expired nonce. This also ensures that we pay out any
    // winners.
    if let Some(round) = get_latest_expired_nonce(&db).await? {
        // We may have already revealed this nonce before the restart, but doing so again does not
        // hurt.
        if let Err(e) = reveal_nonce(&client, &keys, round.nonce, round.event_id).await {
            tracing::error!(
                nonce = hex::encode(round.nonce),
                "Failed to reveal expired nonce after restart: {e:#}. Must publish and handle \
                 payouts manually"
            );
        };
    }

    loop {
        let active_nonce = Nonce::new(thread_rng(), expire_after_secs, reveal_after_secs);

        let commitment_event_id =
            match publish_nonce_commitment(&client, &keys, active_nonce.commitment).await {
                Ok(event_id) => event_id,
                Err(e) => {
                    tracing::error!("Failed to publish nonce commitment: {e:#}. Trying again");
                    continue;
                }
            };

        if let Err(e) = set_active_nonce(
            &db,
            db::Round {
                nonce: active_nonce.inner,
                event_id: commitment_event_id,
            },
        )
        .await
        {
            tracing::error!("Failed to set active nonce: {e:#}");

            if let Err(e) = unset_active_nonce(&db).await {
                tracing::error!("Failed to unset active nonce. This is bad! Error: {e:#}");
            }

            continue;
        }

        tracing::debug!(commitment = %active_nonce.commitment, %commitment_event_id, "New active nonce");

        let expiry = tokio::time::Instant::from_std(active_nonce.expire_at());

        let exit = tokio::select! {
            _ = tokio::time::sleep_until(expiry) => ControlFlow::Continue(()),
            _ = ctrl_c.recv() => {
                tracing::warn!("Got Ctrl+C; shutting down...");
                ControlFlow::Break(())
            },
        };

        tracing::debug!(commitment = %active_nonce.commitment, "Nonce has expired");

        if let Err(e) = set_latest_expired_nonce(
            &db,
            db::Round {
                nonce: active_nonce.inner,
                event_id: commitment_event_id,
            },
        )
        .await
        {
            tracing::error!(
                nonce = hex::encode(active_nonce.inner),
                "Failed to set latest expired nonce: {e:#}. This could cause problems after an \
                 untimely restart"
            );
        }

        if exit.is_continue() {
            tokio::spawn(reveal_nonce_later(
                client.clone(),
                keys.clone(),
                active_nonce,
                commitment_event_id,
            ));
        } else {
            tracing::info!("Revealing nonce now due to Ctrl+C");
            if let Err(e) =
                reveal_nonce(&client, &keys, active_nonce.inner, commitment_event_id).await
            {
                tracing::error!(
                    nonce = hex::encode(active_nonce.inner),
                    "Failed to reveal nonce: {e:#}. Must publish manually"
                );
            }

            if let Err(e) = unset_active_nonce(&db).await {
                tracing::error!(
                    "Failed to unset active nonce during shutdown: {e:#}. This could be bad!"
                );
            }

            return Ok(());
        }
    }
}

impl Nonce {
    fn new<R: RngCore>(rng: R, expire_after_secs: u64, reveal_after_secs: u64) -> Self {
        let mut rng = rand::rngs::StdRng::from_rng(rng).expect("rng");
        let nonce: [u8; 32] = rng.gen();

        let commitment = nonce_commitment(nonce);

        Self {
            inner: nonce,
            commitment,
            created_at: Instant::now(),
            expire_after: Duration::from_secs(expire_after_secs),
            reveal_after: Duration::from_secs(reveal_after_secs),
        }
    }

    fn expire_at(&self) -> Instant {
        self.created_at + self.expire_after
    }

    fn reveal_at(&self) -> Instant {
        self.created_at + self.expire_after + self.reveal_after
    }
}

pub fn nonce_commitment(nonce: [u8; 32]) -> sha256::Hash {
    let mut hasher = sha256::Hash::engine();
    hasher.input(&nonce);

    sha256::Hash::from_engine(hasher)
}

async fn publish_nonce_commitment(
    client: &nostr_sdk::Client,
    keys: &nostr::Keys,
    commitment: sha256::Hash,
) -> Result<EventId> {
    let event = EventBuilder::text_note(
        format!(
            "A new NostrDice round has started! Zap the note with your chosen multiplier.\n\
             Here is the SHA256 commitment which makes the game fair: {commitment}"
        ),
        [Tag::from_standardized(TagStandard::Sha256(commitment))],
    )
    .to_event(keys)?;

    let event_id = client.send_event(event.clone()).await?;

    Ok(event_id)
}

async fn reveal_nonce_later(
    client: nostr_sdk::Client,
    keys: nostr::Keys,
    nonce: Nonce,
    commitment_event_id: EventId,
) {
    tracing::debug!(commitment = %nonce.commitment, "Waiting to reveal expired nonce");

    let reveal_at = tokio::time::Instant::from_std(nonce.reveal_at());
    tokio::time::sleep_until(reveal_at).await;

    if let Err(e) = reveal_nonce(&client, &keys, nonce.inner, commitment_event_id).await {
        tracing::error!(
            nonce = hex::encode(nonce.inner),
            "Failed to reveal nonce: {e:#}. Must publish manually"
        );
    };
}

async fn reveal_nonce(
    client: &nostr_sdk::Client,
    keys: &nostr_sdk::Keys,
    nonce: [u8; 32],
    commitment_event_id: EventId,
) -> Result<()> {
    let event = EventBuilder::text_note(
        format!(
            "Revealing nonce: {}. Matching commitment: nostr:{}",
            hex::encode(nonce),
            commitment_event_id.to_bech32().expect("valid note ID"),
        ),
        [],
    )
    .to_event(keys)?;

    client.send_event(event.clone()).await?;

    tracing::debug!(%commitment_event_id, "Expired nonce revealed");

    Ok(())
}

pub async fn get_active_nonce(db: &SqlitePool) -> Result<Option<Round>> {
    sqlx::query_as!(
        RoundRow,
        r#"SELECT nonces.event_id, nonces.nonce FROM active_nonce
            JOIN nonces ON nonces.event_id = active_nonce.nonce_event_id;"#
    )
    .try_map(Round::try_from)
    .fetch_optional(db)
    .await
    .context("Failed to get active nonce")
}

pub async fn set_active_nonce(db: &SqlitePool, round: Round) -> Result<()> {
    let event_id = round.event_id.to_hex();
    let nonce = hex::encode(round.nonce);

    query!(
        "INSERT INTO nonces (event_id, nonce) VALUES (?1, ?2);",
        event_id,
        nonce,
    )
    .execute(db)
    .await?;

    query!(
        "INSERT INTO active_nonce (id, nonce_event_id) VALUES (?1, ?2)
            ON CONFLICT(id) DO UPDATE SET nonce_event_id = excluded.nonce_event_id;",
        0,
        event_id,
    )
    .execute(db)
    .await?;

    Ok(())
}

pub async fn unset_active_nonce(db: &SqlitePool) -> Result<Option<db::Round>> {
    let id = query!("DELETE FROM active_nonce RETURNING nonce_event_id;")
        .fetch_optional(db)
        .await?
        .map(|r| r.nonce_event_id);

    match id {
        None => Ok(None),
        Some(id) => query_as!(
            RoundRow,
            "SELECT event_id, nonce FROM nonces WHERE event_id = ?1",
            id,
        )
        .try_map(Round::try_from)
        .fetch_optional(db)
        .await
        .context("Failed to get active nonce"),
    }
}

pub async fn set_latest_expired_nonce(db: &SqlitePool, round: db::Round) -> anyhow::Result<()> {
    let event_id = round.event_id.to_hex();

    query!(
        "INSERT INTO latest_expired_nonce (id, nonce_event_id) VALUES (?1, ?2)
            ON CONFLICT(id) DO UPDATE SET nonce_event_id = excluded.nonce_event_id;",
        0,
        event_id,
    )
    .execute(db)
    .await?;

    Ok(())
}

pub async fn get_latest_expired_nonce(db: &SqlitePool) -> anyhow::Result<Option<db::Round>> {
    sqlx::query_as!(
        RoundRow,
        r#"SELECT nonces.event_id, nonces.nonce FROM latest_expired_nonce
            JOIN nonces ON nonces.event_id = latest_expired_nonce.nonce_event_id;"#
    )
    .try_map(Round::try_from)
    .fetch_optional(db)
    .await
    .context("Failed to get active nonce")
}
