use anyhow::Context;
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
    // Is `Some` if the zap invoice was paid.
    pub receipt_id: Option<String>,
    // Is `Some` if the zapper chose a winning multiplier, _and_ if we
    // have sent the payout to the zapper.
    pub payout_id: Option<String>,
}

// The event id is the note id of the round announcing the roll.
pub fn upsert_zap(
    db: &Db,
    event_id: EventId,
    payment_hash: String,
    zap: Zap,
) -> anyhow::Result<()> {
    let value = serde_json::to_vec(&zap)?;

    let zap_tree = db.open_tree(event_id.to_hex())?;
    zap_tree.insert(payment_hash.as_bytes(), value)?;

    Ok(())
}

pub fn get_zaps_by_event_id(db: &Db, event_id: EventId) -> anyhow::Result<Vec<Zap>> {
    let zap_tree = db.open_tree(event_id.to_hex())?;

    let zaps = zap_tree
        .iter()
        .map(|e| {
            serde_json::from_slice::<Zap>(&e.expect("").1).context("failed to deserialize zap")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(zaps)
}

pub fn get_zap(db: &Db, event_id: EventId, payment_hash: String) -> anyhow::Result<Option<Zap>> {
    let zap_tree = db.open_tree(event_id.to_hex())?;
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

pub fn set_active_round(db: &Db, event_id: EventId) -> anyhow::Result<()> {
    let dice_roll_tree = db.open_tree("dice_roll")?;

    let value = serde_json::to_vec(&event_id)?;
    dice_roll_tree.insert("active".as_bytes(), value)?;

    Ok(())
}

pub fn remove_active_dice_roll(db: &Db) -> anyhow::Result<Option<Round>> {
    let dice_roll_tree = db.open_tree("dice_roll")?;
    let active_dice_roll = dice_roll_tree.remove("active".as_bytes())?;

    let event_id: EventId = match active_dice_roll {
        Some(event_id) => serde_json::from_slice(&event_id)?,
        None => return Ok(None),
    };

    let dice_roll = dice_roll_tree
        .get(event_id.as_bytes())?
        .context("missing dice roll")?;
    let dice_roll = serde_json::from_slice(&dice_roll)?;

    Ok(Some(dice_roll))
}

pub fn get_current_round(db: &Db) -> anyhow::Result<Option<Round>> {
    let dice_roll_tree = db.open_tree("dice_roll")?;
    let active_dice_roll = dice_roll_tree.get("active".as_bytes())?;

    let event_id: EventId = match active_dice_roll {
        Some(event_id) => serde_json::from_slice(&event_id)?,
        None => return Ok(None),
    };

    let dice_roll = dice_roll_tree
        .get(event_id.as_bytes())?
        .context("missing dice roll")?;
    let dice_roll = serde_json::from_slice(&dice_roll)?;

    Ok(Some(dice_roll))
}

pub fn upsert_round(db: &Db, dice_roll: Round) -> anyhow::Result<()> {
    let dice_roll_tree = db.open_tree("dice_roll")?;
    let value = serde_json::to_vec(&dice_roll)?;
    dice_roll_tree.insert(dice_roll.event_id.as_bytes(), value)?;

    Ok(())
}
