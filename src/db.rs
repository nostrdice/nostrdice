use lightning_invoice::Bolt11Invoice;
use nostr::Event;
use serde::{Deserialize, Serialize};
use sled::Db;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zap {
    pub invoice: Bolt11Invoice,
    pub request: Event,
    // is some if the invoice was paid.
    pub note_id: Option<String>,
}

pub fn upsert_zap(db: &Db, payment_hash: String, zap: Zap) -> anyhow::Result<()> {
    let value = serde_json::to_vec(&zap)?;
    db.insert(payment_hash.as_bytes(), value)?;

    Ok(())
}

pub fn get_zap(db: &Db, payment_hash: String) -> anyhow::Result<Option<Zap>> {
    let value = db.get(payment_hash.as_bytes())?;

    match value {
        Some(value) => {
            let zap = serde_json::from_slice(&value)?;
            Ok(Some(zap))
        }
        None => Ok(None),
    }
}


#[derive(Clone, Serialize, Deserialize)]
pub struct DiceRoll {
    pub roll: u16,
    pub nonce: u64,
    pub event_id: String,
}

pub fn upsert_dice_roll(db: &Db, dice_roll: DiceRoll) -> anyhow::Result<()> {
    let value = serde_json::to_vec(&dice_roll)?;
    db.insert(dice_roll.event_id.as_bytes(), value)?;

    Ok(())
}
