//! - no instant payout
//!
//! - publish multiplier notes once
//! - announce commitment and nonce reveal on different account
//!
//! roll = first_2_bytes_in_decimal(sha256(nonce | npub | memo))
//!
//! ## zap invoice
//!
//! User claims they are owed zap_amount * multiplier
//!
//! - amount
//! - description/m: nonce commitment noteId, nonce commitment, multiplier note id, roller's npub, memo
//! - signature proves that we approved this zap request
//!
//! ## payout invoice
//!

use crate::db;
use crate::db::Round;
use crate::db::Zap;
use crate::multiplier::Multipliers;
use crate::zapper::PayInvoice;
use anyhow::Context;
use anyhow::Result;
use bitcoin::secp256k1::rand;
use nostr::bitcoin::hashes::sha256;
use nostr::bitcoin::hashes::HashEngine;
use nostr::prelude::ZapType;
use nostr::EventBuilder;
use nostr::Keys;
use nostr::Tag;
use nostr::ToBech32;
use nostr_sdk::client::ZapDetails;
use nostr_sdk::hashes::Hash;
use nostr_sdk::zapper::async_trait;
use nostr_sdk::Client;
use nostr_sdk::NostrZapper;
use nostr_sdk::PublicKey;
use nostr_sdk::TagStandard;
use nostr_sdk::ZapperBackend;
use nostr_sdk::ZapperError;
use rand::thread_rng;
use rand::Rng;
use rand::SeedableRng;
use sled::Db;
use std::fmt::Debug;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub struct LndZapper {
    pub sender: mpsc::Sender<PayInvoice>,
}

#[derive(Clone, Debug)]
pub struct RoundManager {
    client: Client,
    multipliers: Multipliers,
    nonce_keys: Keys,
}

impl RoundManager {
    pub fn new(client: Client, nonce_keys: Keys, multipliers: Multipliers) -> Self {
        Self {
            client,
            nonce_keys,
            multipliers,
        }
    }

    pub async fn commit_to_nonce(&self) -> Result<Round> {
        let mut rng = rand::rngs::StdRng::from_rng(thread_rng()).expect("rng");
        let nonce: [u8; 32] = rng.gen();

        let mut hasher = sha256::Hash::engine();
        hasher.input(&nonce);

        let commitment = sha256::Hash::from_engine(hasher);

        tracing::debug!("Generated commitment: {commitment}");

        let event = EventBuilder::text_note(
            format!("A new NostrDice round has started! Zap the note with your chosen multiplier.\nHere is the SHA256 commitment which makes the game fair: {}", commitment),
            [Tag::from_standardized(TagStandard::Sha256(commitment))],
        ).to_event(&self.nonce_keys)?;

        let event_id = self.client.send_event(event.clone()).await?;

        let round = Round { nonce, event_id };

        Ok(round)
    }

    pub async fn determine_winners(&self, round: Round, zaps: Vec<Zap>) -> anyhow::Result<()> {
        tracing::info!("Time to roll the dice");

        for zap in zaps {
            match zap {
                Zap {
                    roller,
                    invoice,
                    multiplier_note_id,
                    request,
                    receipt_id: Some(_),
                    ..
                } => {
                    let roll = generate_roll(round.nonce, roller, request.content.clone());

                    // TODO: This will change once we use static multiplier notes.
                    let multiplier = match self
                        .multipliers
                        .0
                        .iter()
                        .find(|note| note.note_id == multiplier_note_id)
                    {
                        Some(note) => &note.multiplier,
                        None => {
                            tracing::warn!("Zap does not correspond to this round");
                            continue;
                        }
                    };

                    let threshold = multiplier.get_lower_than();
                    if roll >= threshold {
                        tracing::debug!("{roller} did not win this time");
                        tracing::debug!(
                            "{roller} was aiming for <{threshold}, and they got {roll}"
                        );

                        continue;
                    }

                    tracing::info!("{roller} is a winner!");
                    tracing::debug!("{roller} was aiming for <{threshold}, and they got {roll}");

                    let zap_amount_msat = invoice
                        .amount_milli_satoshis()
                        .expect("amount to be present");
                    let amount_sat =
                        calculate_price_money(zap_amount_msat, multiplier.get_multiplier());

                    tracing::debug!(
                        "Sending {} * {} = {amount_sat} to {roller} for hitting a {} multiplier",
                        zap_amount_msat / 1_000,
                        multiplier.get_multiplier(),
                        multiplier.get_content()
                    );

                    let zap_details = ZapDetails::new(ZapType::Public).message(
                        format!("Won a {} bet on NostrDice!", multiplier.get_multiplier())
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
                Zap {
                    roller,
                    receipt_id: None,
                    ..
                } => {
                    tracing::debug!("Skipping {roller} because they did not pay zap invoice");
                }
            }
        }

        Ok(())
    }
}

pub fn calculate_price_money(amount_msat: u64, multiplier: f32) -> u64 {
    ((amount_msat as f32 / 1000.0) * multiplier).floor() as u64
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

pub async fn run_rounds(db: Db, manager: RoundManager, round_interval: Duration) -> Result<()> {
    loop {
        let round = match db::get_current_round(&db).context("Failed to get active dice roll")? {
            None => {
                let mut interval = tokio::time::interval(round_interval);

                interval.tick().await;

                let round = match manager.commit_to_nonce().await {
                    Ok(round) => {
                        let note_id = round.get_note_id();
                        tracing::info!("Started new round with note id: {}", note_id);
                        if let Err(e) = db::set_active_round(&db, round.event_id) {
                            tracing::error!(note_id, "Failed to set active round. Error: {e:#}");
                        }

                        if let Err(e) = db::upsert_round(&db, round.clone()) {
                            tracing::error!(note_id, "Failed to upsert round. Error: {e:#}");
                        }

                        round
                    }
                    Err(e) => {
                        tracing::error!("Failed to roll the dice. Error: {e:#}");
                        continue;
                    }
                };

                interval.tick().await;

                round
            }
            Some(round) => {
                let note_id = round.get_note_id();
                tracing::warn!(
                    note_id,
                    "Can't start a new dice roll round, as there is still an active round."
                );

                round
            }
        };

        // TODO: We are finishing the round early on a restart.

        tracing::info!(note_id = round.get_note_id(), "Closing the round.");

        // No more zaps allowed for the current round! Otherwise,
        // users will lose money.
        //
        // TODO: Remove concept of active round. Instead, we must
        // always have a nonce ready.
        db::remove_active_dice_roll(&db)?;

        let zaps = db::get_zaps_by_event_id(&db, round.event_id)?;

        let note_id = round.get_note_id();
        if let Err(e) = manager.determine_winners(round, zaps).await {
            tracing::error!(note_id, "Failed to end dice roll! Error: {e:#}")
        }
    }
}

fn generate_roll(nonce: [u8; 32], roller_npub: PublicKey, memo: String) -> u16 {
    let mut hasher = sha256::Hash::engine();

    let nonce = hex::encode(nonce);
    let nonce = nonce.as_bytes();

    let roller_npub = roller_npub.to_bech32().expect("valid npub");
    let roller_npub = roller_npub.as_bytes();

    let memo = memo.as_bytes();

    hasher.input(nonce);
    hasher.input(roller_npub);
    hasher.input(memo);

    let roll = sha256::Hash::from_engine(hasher);
    let roll = roll.to_byte_array();

    let roll = hex::encode(roll);

    dbg!(&roll);

    let roll = roll.get(0..4).expect("long enough");

    u16::from_str_radix(roll, 16).expect("valid hex")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dice::calculate_price_money;
    use crate::dice::generate_roll;
    use crate::multiplier::Multiplier;

    #[test]
    /// You can verify the outcome by visiting this URL:
    /// https://emn178.github.io/online-tools/sha256.html?input=0000000000000000000000000000000000000000000000000000000000000000npub130nwn4t5x8h0h6d983lfs2x44znvqezucklurjzwtn7cv0c73cxsjemx32Hello%2C%20world!%20%F0%9F%94%97&input_type=utf-8&output_type=hex&hmac_enabled=0&hmac_input_type=utf-8
    fn generate_roll_test() {
        let nonce = [0u8; 32];
        let roller_npub =
            PublicKey::parse("npub130nwn4t5x8h0h6d983lfs2x44znvqezucklurjzwtn7cv0c73cxsjemx32")
                .unwrap();
        let memo = "Hello, world! 🔗".to_string();

        let n = generate_roll(nonce, roller_npub, memo);

        println!("You rolled a {n}");

        assert_eq!(n, 19213);
    }

    #[test]
    pub fn test_multipliers_1_05() {
        let amount_msat = 1_000_000;

        let amount_sat = calculate_price_money(amount_msat, Multiplier::X1_05.get_multiplier());

        assert_eq!((1000.0 * 1.05) as u64, amount_sat)
    }

    #[test]
    pub fn test_multipliers_1_1() {
        let amount_msat = 1_000_000;

        let amount_sat = calculate_price_money(amount_msat, Multiplier::X1_1.get_multiplier());

        assert_eq!((1000.0 * 1.1) as u64, amount_sat)
    }

    #[test]
    pub fn test_multipliers_1_5() {
        let amount_msat = 1_000_000;

        let amount_sat = calculate_price_money(amount_msat, Multiplier::X1_5.get_multiplier());

        assert_eq!((1000.0 * 1.5) as u64, amount_sat)
    }

    #[test]
    pub fn test_multipliers_2() {
        let amount_msat = 1_000_000;

        let amount_sat = calculate_price_money(amount_msat, Multiplier::X2.get_multiplier());

        assert_eq!((1000.0 * 2.0) as u64, amount_sat)
    }
}
