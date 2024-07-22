use anyhow::Context;
use lightning_invoice::Bolt11Invoice;
use nostr::Event;
use nostr::PublicKey;
use serde::Deserialize;
use serde::Serialize;
use sled::Db;
use std::fmt;
use std::fmt::Formatter;
use strum_macros::EnumIter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zap {
    pub roller: PublicKey,
    pub invoice: Bolt11Invoice,
    pub request: Event,
    // the note_id of the multiplier e.g. x1.1 that has been zapped
    pub note_id: String,
    // is some if the invoice was paid.
    pub receipt_id: Option<String>,
}

pub fn upsert_zap(db: &Db, payment_hash: String, zap: Zap) -> anyhow::Result<()> {
    let value = serde_json::to_vec(&zap)?;

    let zap_tree = db.open_tree("zap")?;
    zap_tree.insert(payment_hash.as_bytes(), value)?;

    Ok(())
}

pub fn get_all_zaps(db: &Db) -> anyhow::Result<Vec<Zap>> {
    let zap_tree = db.open_tree("zap")?;

    let zaps = zap_tree
        .iter()
        .map(|e| {
            serde_json::from_slice::<Zap>(&e.expect("").1).context("failed to deserialize zap")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(zaps)
}

pub fn get_zap(db: &Db, payment_hash: String) -> anyhow::Result<Option<Zap>> {
    let zap_tree = db.open_tree("zap")?;
    let value = zap_tree.get(payment_hash.as_bytes())?;

    match value {
        Some(value) => {
            let zap = serde_json::from_slice(&value)?;
            Ok(Some(zap))
        }
        None => Ok(None),
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct MultiplierNote {
    pub multiplier: Multiplier,
    pub note_id: String,
}

impl fmt::Display for MultiplierNote {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        format!("{}, {}", self.note_id, self.multiplier.get_content()).fmt(f)
    }
}

#[derive(Clone, Serialize, Deserialize, EnumIter, Debug)]
pub enum Multiplier {
    X1_05,
    X1_1,
    X1_33,
    X1_5,
    X2,
    X3,
    X10,
    X25,
    X50,
    X100,
    X1000,
}

impl Multiplier {
    pub const fn get_multiplier(&self) -> f32 {
        match self {
            Multiplier::X1_05 => 1.05,
            Multiplier::X1_1 => 1.10,
            Multiplier::X1_33 => 1.33,
            Multiplier::X1_5 => 1.5,
            Multiplier::X2 => 2.0,
            Multiplier::X3 => 3.0,
            Multiplier::X10 => 10.0,
            Multiplier::X25 => 25.0,
            Multiplier::X50 => 50.0,
            Multiplier::X100 => 100.0,
            Multiplier::X1000 => 1000.0,
        }
    }

    pub const fn get_lower_than(&self) -> u16 {
        match self {
            Multiplier::X1_05 => 60_541,
            Multiplier::X1_1 => 57_789,
            Multiplier::X1_33 => 47_796,
            Multiplier::X1_5 => 42_379,
            Multiplier::X2 => 31_784,
            Multiplier::X3 => 21_189,
            Multiplier::X10 => 6_356,
            Multiplier::X25 => 2_542,
            Multiplier::X50 => 1_271,
            Multiplier::X100 => 635,
            Multiplier::X1000 => 64,
        }
    }

    pub fn get_content(&self) -> String {
        match self {
            Multiplier::X1_05 => "1.05x".to_string(),
            Multiplier::X1_1 => "1_1x".to_string(),
            Multiplier::X1_33 => "1_33x".to_string(),
            Multiplier::X1_5 => "1_5x".to_string(),
            Multiplier::X2 => "2x".to_string(),
            Multiplier::X3 => "3x".to_string(),
            Multiplier::X10 => "10x".to_string(),
            Multiplier::X25 => "25x".to_string(),
            Multiplier::X50 => "50x".to_string(),
            Multiplier::X100 => "100x".to_string(),
            Multiplier::X1000 => "1000x".to_string(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DiceRoll {
    pub roll: u16,
    pub nonce: u64,
    pub event_id: String,
    pub multipliers: Vec<MultiplierNote>,
}

pub fn upsert_dice_roll(db: &Db, dice_roll: DiceRoll) -> anyhow::Result<()> {
    let dice_roll_tree = db.open_tree("dice_roll")?;
    let value = serde_json::to_vec(&dice_roll)?;
    dice_roll_tree.insert(dice_roll.event_id.as_bytes(), value)?;

    Ok(())
}
