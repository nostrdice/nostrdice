use lightning_invoice::Bolt11Invoice;
use nostr::Event;
use serde::Deserialize;
use serde::Serialize;
use sled::Db;
use strum_macros::EnumIter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zap {
    pub invoice: Bolt11Invoice,
    pub request: Event,
    // the note_id of the multiplier e.g. x1.1 that has been zapped
    pub note_id: String,
    // is some if the invoice was paid.
    pub receipt_id: Option<String>,
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

#[derive(Clone, Serialize, Deserialize, EnumIter)]
pub enum Multiplier {
    X1_05(String),
    X1_1(String),
    X1_33(String),
    X1_5(String),
    X2(String),
    X3(String),
    X10(String),
    X25(String),
    X50(String),
    X100(String),
    X1000(String),
}

impl Multiplier {
    pub fn get_content(&self) -> String {
        match self {
            Multiplier::X1_05(_) => "1.05x".to_string(),
            Multiplier::X1_1(_) => "1_1x".to_string(),
            Multiplier::X1_33(_) => "1_33x".to_string(),
            Multiplier::X1_5(_) => "1_5x".to_string(),
            Multiplier::X2(_) => "2x".to_string(),
            Multiplier::X3(_) => "3x".to_string(),
            Multiplier::X10(_) => "10x".to_string(),
            Multiplier::X25(_) => "25x".to_string(),
            Multiplier::X50(_) => "50x".to_string(),
            Multiplier::X100(_) => "100x".to_string(),
            Multiplier::X1000(_) => "1000x".to_string(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DiceRoll {
    pub roll: u16,
    pub nonce: u64,
    pub event_id: String,
    pub multipliers: Vec<Multiplier>,
}

pub fn upsert_dice_roll(db: &Db, dice_roll: DiceRoll) -> anyhow::Result<()> {
    let value = serde_json::to_vec(&dice_roll)?;
    db.insert(dice_roll.event_id.as_bytes(), value)?;

    Ok(())
}
