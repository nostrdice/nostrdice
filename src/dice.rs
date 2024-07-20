use std::time::Duration;
use anyhow::Result;
use bitcoin::hashes::sha256;
use bitcoin::secp256k1::rand;
use nostr::{EventBuilder, Keys, Tag, ToBech32};
use nostr::hashes::Hash;
use nostr_sdk::Client;
use rand::Rng;
use sled::Db;
use tokio::time::sleep;
use sha2::Sha256;
use sha2::Digest;
use crate::db;
use crate::db::DiceRoll;

// a new round every five minutes
const ROUND_TIMEOUT: Duration = Duration::from_secs(60*5);

pub async fn start_rounds(db: Db, keys: Keys) -> Result<()> {
    loop {
        let (roll, nonce) = {
            let mut rng = rand::thread_rng();
            let roll = rng.gen_range(u16::MIN..=u16::MAX);
            let nonce = rng.gen_range(u64::MIN..=u64::MAX);

            (roll, nonce)
        };

        let mut hasher = Sha256::new();
        hasher.update(roll.to_le_bytes());
        hasher.update(nonce.to_le_bytes());
        let commitment = hasher.finalize();

        let commitment = sha256::Hash::hash(&commitment);

        let event = EventBuilder::text_note(format!("What is it gonna be? {}", commitment), [Tag::Sha256(commitment)]).to_event(&keys)?;

        // Create new client
        let client = Client::new(&keys);
        client.add_relays(crate::subscriber::RELAYS).await?;
        client.connect().await;

        let event_id = client.send_event(event).await?;
        let _ = client.disconnect().await;

        let dice_roll = DiceRoll {
            roll,
            nonce,
            event_id: event_id.to_bech32().expect("bech 32"),
        };

        db::upsert_dice_roll(&db, dice_roll)?;

        println!(
            "Broadcasted event id: {}!",
            event_id.to_bech32().expect("bech32")
        );

        sleep(ROUND_TIMEOUT).await;
    }
}