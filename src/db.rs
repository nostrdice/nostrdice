use lightning_invoice::Bolt11Invoice;
use nostr::Event;
use nostr::EventId;
use nostr::PublicKey;
use nostr::ToBech32;
use serde::Deserialize;
use serde::Serialize;
use sqlx::query;
use sqlx::query_as;
use sqlx::SqlitePool;
use time::OffsetDateTime;

/// The record of a roller's bet.
#[derive(Debug, Clone)]
pub struct Zap {
    pub roller: PublicKey,
    pub invoice: Bolt11Invoice,
    pub request: Event,
    // The ID of the chosen multiplier note e.g. 10x.
    pub multiplier_note_id: String,
    pub nonce_commitment_note_id: EventId,
    pub bet_state: BetState,
    pub index: usize,
    /// Timestamp when the user place his bet
    pub bet_timestamp: OffsetDateTime,
}

/// The state of a roller's bet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BetState {
    GameZapInvoiceRequested,
    ZapInvoiceRequested,
    ZapPaid,
    ZapFailed,
    PaidWinner,
    Loser,
}

struct ZapRow {
    roller: String,
    invoice: String,
    request_event: String,
    multiplier_note_id: String,
    nonce_commitment_note_id: String,
    bet_state: String,
    idx: i64,
    bet_timestamp: OffsetDateTime,
}

impl TryFrom<ZapRow> for Zap {
    type Error = sqlx::Error;

    fn try_from(row: ZapRow) -> Result<Self, Self::Error> {
        Ok(Zap {
            roller: row.roller.parse().map_err(|e| sqlx::Error::ColumnDecode {
                index: "roller".to_owned(),
                source: Box::new(e),
            })?,
            invoice: row.invoice.parse().map_err(|e| sqlx::Error::ColumnDecode {
                index: "invoice".to_owned(),
                source: Box::new(e),
            })?,
            request: serde_json::from_str(&row.request_event).map_err(|e| {
                sqlx::Error::ColumnDecode {
                    index: "request_event".to_owned(),
                    source: e.into(),
                }
            })?,
            multiplier_note_id: row.multiplier_note_id,
            nonce_commitment_note_id: row.nonce_commitment_note_id.parse().map_err(|e| {
                sqlx::Error::ColumnDecode {
                    index: "nonce_commitment_note_id".to_owned(),
                    source: Box::new(e),
                }
            })?,
            bet_state: serde_json::from_str(&row.bet_state).map_err(|e| {
                sqlx::Error::ColumnDecode {
                    index: "bet_state".to_owned(),
                    source: e.into(),
                }
            })?,
            index: row.idx as usize,
            bet_timestamp: row.bet_timestamp,
        })
    }
}

pub async fn upsert_zap(db: &SqlitePool, payment_hash: String, zap: Zap) -> anyhow::Result<()> {
    // TODO: This does not scale with lots of zaps.

    let roller = zap.roller.to_hex();
    let invoice = zap.invoice.to_string();
    let request = serde_json::to_string(&zap.request)?;
    let multiplier_id = zap.multiplier_note_id;
    let commitment_id = zap.nonce_commitment_note_id.to_hex();
    let bet_state = serde_json::to_string(&zap.bet_state)?;
    let idx = zap.index as i64;
    let ts = zap.bet_timestamp;

    query!(
        "INSERT INTO zaps
            (payment_hash, roller, invoice, request_event, multiplier_note_id,
             nonce_commitment_note_id, bet_state, idx, bet_timestamp)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT(payment_hash) DO UPDATE SET
            roller = excluded.roller,
            invoice = excluded.invoice,
            request_event = excluded.request_event,
            multiplier_note_id = excluded.multiplier_note_id,
            nonce_commitment_note_id = excluded.nonce_commitment_note_id,
            bet_state = excluded.bet_state,
            idx = excluded.idx,
            bet_timestamp = excluded.bet_timestamp;
        ;",
        payment_hash,
        roller,
        invoice,
        request,
        multiplier_id,
        commitment_id,
        bet_state,
        idx,
        ts,
    )
    .execute(db)
    .await
    .map(|_| ())
    .context("Failed to upsert zap")
}

pub async fn get_zaps_by_event_id(db: &SqlitePool, event_id: EventId) -> anyhow::Result<Vec<Zap>> {
    let event_id = event_id.to_hex();
    query_as!(
        ZapRow,
        "SELECT
            roller, invoice, request_event, multiplier_note_id,
            nonce_commitment_note_id, bet_state, idx, bet_timestamp
        FROM zaps WHERE nonce_commitment_note_id = ?1;",
        event_id,
    )
    .try_map(Zap::try_from)
    .fetch_all(db)
    .await
    .context("Failed to fetch zaps")
}

pub async fn get_zap(db: &SqlitePool, payment_hash: String) -> anyhow::Result<Option<Zap>> {
    query_as!(
        ZapRow,
        "SELECT
            roller, invoice, request_event, multiplier_note_id,
            nonce_commitment_note_id, bet_state, idx, bet_timestamp
        FROM zaps WHERE payment_hash = ?1;",
        payment_hash,
    )
    .try_map(Zap::try_from)
    .fetch_optional(db)
    .await
    .context("Failed to fetch zaps")
}

/// Returns the zaps within a timewindow
pub async fn get_zaps_in_time_window(
    db: &SqlitePool,
    start_time: OffsetDateTime,
    end_time: OffsetDateTime,
) -> anyhow::Result<Vec<Zap>> {
    query_as!(
        ZapRow,
        "SELECT
            roller, invoice, request_event, multiplier_note_id,
            nonce_commitment_note_id, bet_state, idx, bet_timestamp
        FROM zaps WHERE bet_timestamp > ?1 AND bet_timestamp < ?2;",
        start_time,
        end_time,
    )
    .try_map(Zap::try_from)
    .fetch_all(db)
    .await
    .context("Failed to fetch zaps")
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Round {
    pub nonce: [u8; 32],
    pub event_id: EventId,
}

impl Round {
    pub fn get_note_id(&self) -> String {
        self.event_id.to_bech32().expect("to fit")
    }
}

pub struct RoundRow {
    pub nonce: String,
    pub event_id: String,
}

impl TryFrom<RoundRow> for Round {
    type Error = sqlx::Error;

    fn try_from(row: RoundRow) -> Result<Self, Self::Error> {
        let mut nonce = [0; 32];
        hex::decode_to_slice(row.nonce, &mut nonce).map_err(|e| sqlx::Error::ColumnDecode {
            index: "nonce".to_owned(),
            source: Box::new(e),
        })?;

        Ok(Round {
            nonce,
            event_id: row
                .event_id
                .parse()
                .map_err(|e| sqlx::Error::ColumnDecode {
                    index: "event_id".to_owned(),
                    source: Box::new(e),
                })?,
        })
    }
}
