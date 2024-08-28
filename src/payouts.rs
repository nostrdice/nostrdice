use crate::db::upsert_zap;
use crate::db::BetState;
use crate::db::Zap;
use crate::multiplier::Multipliers;
use anyhow::bail;
use nostr::bitcoin::hashes::sha256;
use nostr::bitcoin::hashes::HashEngine;
use nostr::prelude::ZapType;
use nostr::ToBech32;
use nostr_sdk::client::ZapDetails;
use nostr_sdk::hashes::Hash;
use nostr_sdk::Client;
use nostr_sdk::PublicKey;
use sqlx::SqlitePool;

pub async fn roll_the_die(
    db: &SqlitePool,
    zap: &Zap,
    client: Client,
    multipliers: Multipliers,
    nonce: [u8; 32],
    index: usize,
) -> anyhow::Result<()> {
    let Zap {
        roller,
        request,
        multiplier_note_id,
        invoice,
        ..
    } = zap;
    let roller_npub = roller.to_bech32().expect("npub");

    let roll = generate_roll(nonce, index, *roller, request.content.clone());

    let multiplier = match multipliers
        .0
        .iter()
        .find(|note| &note.note_id == multiplier_note_id)
    {
        Some(note) => &note.multiplier,
        None => {
            bail!("Zap for unknown multiplier note ID. roller_npub={roller_npub}, zap={zap:?}");
        }
    };

    let threshold = multiplier.get_lower_than();
    if roll >= threshold {
        tracing::debug!(
            %roller_npub,
            "Roller did not win this time. \
             Aimed for <{threshold}, got {roll}"
        );

        send_dm(
            &client,
            roller,
            format!("You lost. You rolled {roll}, which was bigger than {threshold}. Try again!"),
        )
        .await;

        let zap = Zap {
            bet_state: BetState::Loser,
            ..zap.clone()
        };
        upsert_zap(db, invoice.payment_hash().to_string(), zap, &multipliers).await?;

        return Ok(());
    }

    send_dm(
        &client,
        roller,
        format!("You won. You rolled {roll}, which was lower than {threshold}."),
    )
    .await;

    tracing::info!(
        %roller_npub,
        "Roller is a winner! Aimed for <{threshold}, got {roll}"
    );

    let zap_amount_msat = invoice
        .amount_milli_satoshis()
        .expect("amount to be present");
    let amount_sat = calculate_price_money(zap_amount_msat, multiplier.get_multiplier());

    tracing::debug!(
        %roller_npub,
        "Sending {} * {} = {amount_sat} to {roller_npub} for hitting a {} multiplier",
        zap_amount_msat / 1_000,
        multiplier.get_multiplier(),
        multiplier.get_content()
    );

    let zap_details = ZapDetails::new(ZapType::Public)
        .message(format!("Won a {}x bet on NostrDice!", multiplier.get_multiplier()).to_string());

    let zap = if let Err(e) = client.zap(zap.roller, amount_sat, Some(zap_details)).await {
        tracing::error!(%roller_npub, "Failed to zap. Error: {e:#}");

        send_dm(
            &client,
            roller,
            "Sorry, we failed to zap you your payout.".to_string(),
        )
        .await;

        Zap {
            bet_state: BetState::ZapFailed,
            ..zap.clone()
        }
    } else {
        Zap {
            bet_state: BetState::PaidWinner,
            ..zap.clone()
        }
    };

    upsert_zap(db, invoice.payment_hash().to_string(), zap, &multipliers).await?;
    Ok(())
}

async fn send_dm(client: &Client, to: &PublicKey, message: String) {
    let npub = to.to_bech32().expect("npub");

    // The `send_private_message` function (NIP17) seems to be not supported by major nostr clients.
    #[allow(deprecated)]
    if let Err(e) = client.send_direct_msg(*to, message, None).await {
        tracing::error!(
            %npub,
            "Failed to send DM: {e:#}"
        );
    }
}

pub fn calculate_price_money(amount_msat: u64, multiplier: f32) -> u64 {
    ((amount_msat as f32 / 1000.0) * multiplier).floor() as u64
}

fn generate_roll(nonce: [u8; 32], index: usize, roller_npub: PublicKey, memo: String) -> u16 {
    let mut hasher = sha256::Hash::engine();

    let nonce = hex::encode(nonce);
    let nonce = nonce.as_bytes();

    let roller_npub = roller_npub.to_bech32().expect("valid npub");
    let roller_npub = roller_npub.as_bytes();

    let memo = memo.as_bytes();

    let index = index.to_string();
    let index = index.as_bytes();

    hasher.input(nonce);
    hasher.input(roller_npub);
    hasher.input(memo);
    hasher.input(index);

    let roll = sha256::Hash::from_engine(hasher);
    let roll = roll.to_byte_array();

    let roll = hex::encode(roll);

    let roll = roll.get(0..4).expect("long enough");

    u16::from_str_radix(roll, 16).expect("valid hex")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplier::Multiplier;
    use crate::payouts::calculate_price_money;
    use crate::payouts::generate_roll;

    #[test]
    /// You can verify the outcome by visiting this URL:
    /// https://emn178.github.io/online-tools/sha256.html?input=0000000000000000000000000000000000000000000000000000000000000000npub130nwn4t5x8h0h6d983lfs2x44znvqezucklurjzwtn7cv0c73cxsjemx32Hello%2C%20world!%20%F0%9F%94%970&input_type=utf-8&output_type=hex&hmac_enabled=0&hmac_input_type=utf-8
    /// then take the first 4 digits of the hex and convert it to a decimal number.
    /// https://www.rapidtables.com/convert/number/hex-to-decimal.html?x=9d6b
    fn generate_roll_test() {
        let nonce = [0u8; 32];

        let roller_npub =
            PublicKey::parse("npub130nwn4t5x8h0h6d983lfs2x44znvqezucklurjzwtn7cv0c73cxsjemx32")
                .unwrap();
        let memo = "Hello, world! 🔗".to_string();

        let n = generate_roll(nonce, 0, roller_npub, memo);

        println!("You rolled a {n}");

        assert_eq!(n, 40299);
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
