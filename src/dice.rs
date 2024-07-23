use crate::db;
use crate::db::DiceRoll;
use crate::db::Multiplier;
use crate::db::MultiplierNote;
use crate::zapper::PayInvoice;
use anyhow::Result;
use bitcoin::hashes::sha256;
use bitcoin::secp256k1::rand;
use nostr::hashes::Hash;
use nostr::EventBuilder;
use nostr::Keys;
use nostr::Marker;
use nostr::Tag;
use nostr::ToBech32;
use nostr_sdk::zapper::async_trait;
use nostr_sdk::Client;
use nostr_sdk::NostrZapper;
use nostr_sdk::ZapperBackend;
use nostr_sdk::ZapperError;
use rand::Rng;
use sha2::Digest;
use sha2::Sha256;
use sled::Db;
use std::fmt::Debug;
use std::time::Duration;
use strum::IntoEnumIterator;
use tokio::sync::mpsc;
use tokio::time::sleep;

// a new round every five minutes
const ROUND_TIMEOUT: Duration = Duration::from_secs(60 * 5);

#[derive(Clone, Debug)]
pub struct LndZapper {
    pub sender: mpsc::Sender<PayInvoice>,
}

#[async_trait]
impl NostrZapper for LndZapper {
    type Err = ZapperError;

    fn backend(&self) -> ZapperBackend {
        ZapperBackend::Custom("lnd".to_string())
    }

    async fn pay(&self, invoice: String) -> nostr::Result<(), Self::Err> {
        self.sender
            .send(PayInvoice(invoice))
            .await
            .map_err(ZapperError::backend)
    }
}

pub async fn start_rounds(
    db: Db,
    keys: Keys,
    relays: Vec<String>,
    lnd_zapper: LndZapper,
) -> Result<()> {
    // Create new client
    let client = Client::new(&keys);
    client.add_relays(relays).await?;

    client.set_zapper(lnd_zapper).await;
    client.connect().await;

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

        let event_id = client.send_event(event.clone()).await?;
        let note_id = event.id.to_bech32().expect("bech32");
        println!("Broadcasted event id: {note_id}!",);

        let mut dice_roll = DiceRoll {
            roll,
            nonce,
            event_id: note_id.clone(),
            multipliers: vec![],
        };

        let mention_event = Tag::Event {
            event_id,
            relay_url: None,
            marker: Some(Marker::Mention),
        };

        for multiplier in Multiplier::iter() {
            let event = EventBuilder::text_note(
                format!("{} nostr:{note_id}", multiplier.get_content()),
                [mention_event.clone()],
            )
            .to_event(&keys)?;
            let event_id = client.send_event(event).await?;
            let note_id = event_id.to_bech32().expect("bech32");
            tracing::info!("Broadcasted multiplier note: {note_id}");

            dice_roll.multipliers.push(MultiplierNote {
                multiplier,
                note_id,
            })
        }

        db::upsert_dice_roll(&db, dice_roll.clone())?;

        sleep(ROUND_TIMEOUT).await;

        // TODO: separate this into a dedicated tokio task that will load active dice rolls from the
        // database.

        let winners = dice_roll
            .multipliers
            .into_iter()
            .filter(|m| m.multiplier.get_lower_than() > dice_roll.roll)
            .collect::<Vec<_>>();
        tracing::debug!("And the winners are {winners:?}.");

        for zap in db::get_all_zaps(&db)? {
            match zap.receipt_id {
                Some(_) => {
                    for winner in winners.iter() {
                        if winner.note_id == zap.note_id {
                            tracing::debug!("{} is a winner!", zap.roller);
                            let amount_sat =
                                ((zap.invoice.amount_milli_satoshis().expect("missing amount")
                                    as f32
                                    / 1000.0)
                                    * winner.multiplier.get_multiplier())
                                .floor() as u64;

                            if let Err(e) = client.zap(zap.roller, amount_sat, None).await {
                                tracing::error!("Failed to zap {}. Error: {e:#}", zap.roller);
                            }
                        }
                    }
                }
                None => {
                    tracing::debug!("Skipping {}. Reason: Did not pay the invoice.", zap.roller);
                }
            }
        }
    }
}
