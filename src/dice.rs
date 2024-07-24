use crate::db;
use crate::db::DiceRoll;
use crate::db::Multiplier;
use crate::db::MultiplierNote;
use crate::db::Zap;
use crate::zapper::PayInvoice;
use anyhow::Result;
use bitcoin::hashes::sha256;
use bitcoin::secp256k1::rand;
use nostr::hashes::Hash;
use nostr::prelude::ZapType;
use nostr::EventBuilder;
use nostr::Keys;
use nostr::Marker;
use nostr::Tag;
use nostr::ToBech32;
use nostr_sdk::client::ZapDetails;
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

#[derive(Clone, Debug)]
pub struct DiceRoller {
    client: Client,
    keys: Keys,
}

impl DiceRoller {
    pub fn new(client: Client, keys: Keys) -> Self {
        Self { client, keys }
    }

    pub async fn start_roll(&self) -> Result<DiceRoll> {
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
            format!("We rolled the dice! Place your bet on the reply posts by zapping the amount. \nThis is the sha256 commitment: {}", commitment),
            [Tag::Sha256(commitment)],
        ).to_event(&self.keys)?;

        let event_id = self.client.send_event(event.clone()).await?;

        let dice_roll = DiceRoll {
            roll,
            nonce,
            event_id,
            multipliers: vec![],
        };

        Ok(dice_roll)
    }

    pub async fn add_multipliers(&self, db: &Db, mut dice_roll: DiceRoll) -> anyhow::Result<()> {
        let mention_event = Tag::Event {
            event_id: dice_roll.event_id,
            relay_url: None,
            marker: Some(Marker::Mention),
        };

        for multiplier in Multiplier::iter() {
            tokio::time::sleep(Duration::from_secs(20)).await;
            let event = EventBuilder::text_note(
                format!(
                    "Win {} the amount you zapped if the rolled number is lower than {}! nostr:{}",
                    multiplier.get_lower_than(),
                    multiplier.get_content(),

                    dice_roll.get_note_id()
                ),
                [mention_event.clone()],
            )
            .to_event(&self.keys)?;

            let event_id = self.client.send_event(event).await?;
            let note_id = event_id.to_bech32().expect("bech32");
            tracing::info!("Broadcasted multiplier note: {note_id}");

            dice_roll.multipliers.push(MultiplierNote {
                multiplier,
                note_id,
            });

            db::upsert_dice_roll(db, dice_roll.clone())?;
        }

        Ok(())
    }

    pub async fn end_roll(&self, dice_roll: DiceRoll, zaps: Vec<Zap>) -> anyhow::Result<()> {
        let winners = dice_roll
            .multipliers
            .into_iter()
            .filter(|m| m.multiplier.get_lower_than() > dice_roll.roll)
            .collect::<Vec<_>>();
        tracing::debug!("[{}], And the winners are {winners:?}.", dice_roll.roll);

        for zap in zaps {
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

                            let zap_details = ZapDetails::new(ZapType::Public).message(
                                format!(
                                    "Won a {} on nostr dice!",
                                    winner.multiplier.get_multiplier()
                                )
                                .to_string(),
                            );
                            if let Err(e) = self
                                .client
                                .zap(zap.roller, amount_sat, Some(zap_details))
                                .await
                            {
                                tracing::error!("Failed to zap {}. Error: {e:#}", zap.roller);

                                // TODO: Send a message to the user that we have not been able to
                                // payout.
                            }
                        }
                    }
                }
                None => {
                    tracing::debug!("Skipping {}. Reason: Did not pay the invoice.", zap.roller);
                }
            }
        }

        Ok(())
    }
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

pub async fn run_rounds(db: Db, dice_roller: DiceRoller) -> Result<()> {
    loop {
        match db::get_active_dice_roll(&db)? {
            None => {
                tokio::spawn({
                    let dice_roller = dice_roller.clone();
                    let db = db.clone();
                    async move {
                        match dice_roller.start_roll().await {
                            Ok(dice_roll) => {
                                let note_id = dice_roll.get_note_id();
                                tracing::info!("Started new round with note id: {}", note_id);
                                if let Err(e) = db::set_active_dice_roll(&db, dice_roll.event_id) {
                                    tracing::error!(
                                        note_id,
                                        "Failed to set dice roll active. Error: {e:#}"
                                    );
                                }

                                if let Err(e) = db::upsert_dice_roll(&db, dice_roll.clone()) {
                                    tracing::error!(
                                        note_id,
                                        "Failed to upsert dice roll. Error: {e:#}"
                                    );
                                }

                                // we already set the dice roll active since the multipliers will be
                                // published with delays. this way the user doesn't have to wait for
                                // all multiplier notes to be published before he can place a bet.
                                if let Err(e) = dice_roller.add_multipliers(&db, dice_roll).await {
                                    tracing::error!("Failed to add multiplier notes to roll event. Error: {e:#}");
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to roll the dice. Error: {e:#}");
                            }
                        }
                    }
                });

                sleep(ROUND_TIMEOUT).await;
            }
            Some(dice_roll) => {
                let note_id = dice_roll.get_note_id();
                tracing::warn!(
                    note_id,
                    "Can't start a new dice roll round, as there is still an active round."
                );
            }
        }

        let dice_roll = db::get_active_dice_roll(&db)?.expect("dice roll");
        tracing::info!(
            note_id = dice_roll.get_note_id(),
            "Closing the dice roll round."
        );

        let zaps = db::get_all_zaps_by_event_id(&db, dice_roll.event_id)?;

        let note_id = dice_roll.get_note_id();
        if let Err(e) = dice_roller.end_roll(dice_roll, zaps).await {
            tracing::error!(note_id, "Failed to end dice roll! Error: {e:#}")
        }

        db::remove_active_dice_roll(&db)?;
    }
}
