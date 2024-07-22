use crate::db;
use crate::db::DiceRoll;
use anyhow::Result;
use bitcoin::hashes::sha256;
use bitcoin::secp256k1::rand;
use nostr::hashes::Hash;
use nostr::EventBuilder;
use nostr::Keys;
use nostr::Marker;
use nostr::Tag;
use nostr::ToBech32;
use nostr_sdk::Client;
use rand::Rng;
use sha2::Digest;
use sha2::Sha256;
use sled::Db;
use std::time::Duration;
use tokio::time::sleep;

// a new round every five minutes
const ROUND_TIMEOUT: Duration = Duration::from_secs(60 * 5);

const MULTIPLIERS: [&str; 11] = [
    "1.05x", "1.1x", "1.33x", "1.5x", "2x", "3x", "10x", "25x", "50x", "100x", "1000x",
];

pub async fn start_rounds(db: Db, keys: Keys, relays: Vec<String>) -> Result<()> {
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

        let event = EventBuilder::text_note(
            format!("What is it gonna be? {}", commitment),
            [Tag::Sha256(commitment)],
        )
        .to_event(&keys)?;

        // Create new client
        let client = Client::new(&keys);
        client.add_relays(relays.clone()).await?;
        client.connect().await;

        let event_id = client.send_event(event.clone()).await?;
        let note_id = event.id.to_bech32().expect("bech32");
        println!("Broadcasted event id: {note_id}!",);

        let dice_roll = DiceRoll {
            roll,
            nonce,
            event_id: note_id.clone(),
        };

        db::upsert_dice_roll(&db, dice_roll)?;

        let mention_event = Tag::Event {
            event_id,
            relay_url: None,
            marker: Some(Marker::Mention),
        };

        for multiplier in MULTIPLIERS {
            let event = EventBuilder::text_note(
                format!("{multiplier} nostr:{note_id}"),
                [mention_event.clone()],
            )
            .to_event(&keys)?;
            let event_id = client.send_event(event).await?;
            tracing::info!(
                "Broadcasted event id: {}!",
                event_id.to_bech32().expect("bech32")
            );
        }

        let _ = client.disconnect().await;

        sleep(ROUND_TIMEOUT).await;
    }
}
