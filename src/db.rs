use lightning_invoice::Bolt11Invoice;
use nostr::Event;
use serde::{Deserialize, Serialize};
use sled::Db;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zap {
    pub invoice: Bolt11Invoice,
    pub request: Event,
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
