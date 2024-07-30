use lightning_invoice::Bolt11Invoice;
use nostr::Event;
use nostr::EventId;
use nostr::PublicKey;
use nostr::ToBech32;
use serde::Deserialize;
use serde::Serialize;
use sled::Db;

/// The record of a roller's bet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zap {
    pub roller: PublicKey,
    pub invoice: Bolt11Invoice,
    pub request: Event,
    // The ID of the chosen multiplier note e.g. 10x.
    pub multiplier_note_id: String,
    pub nonce_commitment_note_id: EventId,
    pub bet_state: BetState,
}

/// The state of a roller's bet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BetState {
    ZapInvoiceRequested,
    ZapPaid,
    ZapFailed,
    PaidWinner,
    Loser,
}

pub fn upsert_zap(db: &Db, payment_hash: String, zap: Zap) -> anyhow::Result<()> {
    let value = serde_json::to_vec(&zap)?;

    // TODO: This does not scale with lots of zaps.
    let zap_tree = db.open_tree("zaps")?;
    zap_tree.insert(payment_hash.as_bytes(), value)?;

    Ok(())
}

pub fn get_zaps_by_event_id(db: &Db, event_id: EventId) -> anyhow::Result<Vec<Zap>> {
    let zap_tree = db.open_tree("zaps")?;

    let zaps = zap_tree
        .iter()
        .filter_map(|e| match serde_json::from_slice::<Zap>(&e.expect("").1) {
            Ok(zap) => Some(zap),
            Err(e) => {
                tracing::error!("Failed to deserialize zap: {e}");
                None
            }
        })
        .filter(|zap| zap.nonce_commitment_note_id == event_id)
        .collect::<Vec<_>>();

    Ok(zaps)
}

pub fn get_zap(db: &Db, payment_hash: String) -> anyhow::Result<Option<Zap>> {
    let zap_tree = db.open_tree("zaps")?;
    let value = zap_tree.get(payment_hash.as_bytes())?;

    match value {
        Some(value) => {
            let zap = serde_json::from_slice(&value)?;
            Ok(Some(zap))
        }
        None => Ok(None),
    }
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
