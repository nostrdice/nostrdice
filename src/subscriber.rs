use crate::db::get_zap;
use crate::db::upsert_zap;
use crate::db::BetState;
use crate::db::Zap;
use crate::multiplier::Multipliers;
use crate::nonce;
use crate::payouts;
use crate::utils;
use anyhow::Result;
use bitcoin::hashes::Hash;
use bitcoin::key::Secp256k1;
use bitcoin::secp256k1::SecretKey;
use lightning_invoice::Currency;
use lightning_invoice::InvoiceBuilder;
use lightning_invoice::PaymentSecret;
use nostr::prelude::ToBech32;
use nostr::EventBuilder;
use nostr::EventId;
use nostr::Keys;
use nostr_sdk::Client;
use nostr_sdk::Options;
use sled::Db;
use std::time::Duration;
use tonic_openssl_lnd::lnrpc;
use tonic_openssl_lnd::lnrpc::invoice::InvoiceState;
use tonic_openssl_lnd::LndLightningClient;

pub async fn start_invoice_subscription(
    db: Db,
    mut lnd: LndLightningClient,
    key: Keys,
    client: Client,
    multipliers: Multipliers,
) {
    loop {
        tracing::info!("Starting invoice subscription");

        let sub = lnrpc::InvoiceSubscription::default();
        let mut invoice_stream = lnd
            .subscribe_invoices(sub)
            .await
            .expect("Failed to start invoice subscription")
            .into_inner();

        while let Some(ln_invoice) = invoice_stream
            .message()
            .await
            .expect("Failed to receive invoices")
        {
            match InvoiceState::from_i32(ln_invoice.state) {
                Some(InvoiceState::Settled) => {
                    let db = db.clone();
                    let key = key.clone();
                    tokio::spawn({
                        let client = client.clone();
                        let multipliers = multipliers.clone();
                        async move {
                            let fut = handle_paid_invoice(
                                &db,
                                hex::encode(ln_invoice.r_hash),
                                key.clone(),
                                client,
                                multipliers.clone(),
                            );

                            match tokio::time::timeout(Duration::from_secs(30), fut).await {
                                Ok(Ok(_)) => {
                                    tracing::info!("Handled paid invoice!");
                                }
                                Ok(Err(e)) => {
                                    tracing::error!("Failed to handle paid invoice: {}", e);
                                }
                                Err(_) => {
                                    tracing::error!("Timeout");
                                }
                            }
                        }
                    });
                }
                None
                | Some(InvoiceState::Canceled)
                | Some(InvoiceState::Open)
                | Some(InvoiceState::Accepted) => {}
            }
        }
    }
}

async fn handle_paid_invoice(
    db: &Db,
    payment_hash: String,
    keys: Keys,
    client: Client,
    multipliers: Multipliers,
) -> anyhow::Result<()> {
    match get_zap(db, payment_hash.clone())? {
        None => {
            tracing::warn!("Received a payment without bet.");
            Ok(())
        }
        Some(
            mut zap @ Zap {
                bet_state: BetState::ZapInvoiceRequested,
                ..
            },
        ) => {
            let note_id = zap.request.id().to_hex();
            let amount_msat = zap.invoice.amount_milli_satoshis().unwrap_or_default();
            tracing::info!(note_id, amount_msat, "Received a zap for non game note");

            let client = ephermal_client(client, &mut zap).await?;

            let event_id = publish_zap_receipt(&keys, &mut zap, client).await?;

            tracing::info!(
                event_id = event_id.to_bech32().expect("bech32"),
                "Broadcasted zap receipt for normal zap",
            );

            Ok(())
        }
        Some(
            mut zap @ Zap {
                bet_state: BetState::GameZapInvoiceRequested,
                ..
            },
        ) => {
            let note_id = zap.request.id().to_hex();
            let amount_msat = zap.invoice.amount_milli_satoshis().unwrap_or_default();
            tracing::info!(note_id, amount_msat, "Received a zap for game note");
            // At this stage, this `Zap` indicates that the roller has placed their bet. We will
            // determine their outcome as soon as their nonce is revealed.
            zap.bet_state = BetState::ZapPaid;
            upsert_zap(db, payment_hash, zap.clone())?;

            let client = ephermal_client(client, &mut zap).await?;

            tokio::spawn({
                let db = db.clone();
                let client = client.clone();
                let zap = zap.clone();
                async move {
                    match nonce::get_active_nonce(&db) {
                        Ok(Some(round)) => {
                            tracing::info!(
                                nonce_commitment_note_id = round.get_note_id(),
                                "Time to roll the dice"
                            );
                            if let Err(e) = payouts::roll_the_die(
                                &db,
                                &zap,
                                client,
                                multipliers,
                                round.nonce,
                                zap.index,
                            )
                            .await
                            {
                                tracing::error!("Failed to roll the die. Error: {e:#}");
                            }
                        }
                        Ok(None) => tracing::error!("Failed to payout winner: No active round."),
                        Err(e) => tracing::error!("Failed to get active nonce round. Error: {e:#}"),
                    }
                }
            });

            let event_id = publish_zap_receipt(&keys, &mut zap, client).await?;

            tracing::info!(
                event_id = event_id.to_bech32().expect("bech32"),
                "Broadcasted zap receipt for game zap",
            );

            Ok(())
        }
        Some(_) => {
            tracing::warn!("Ignoring other zaps which might have been donations.");
            Ok(())
        }
    }
}

async fn publish_zap_receipt(keys: &Keys, zap: &mut Zap, client: Client) -> Result<EventId> {
    let preimage = zap.request.id.to_bytes();
    let invoice_hash = bitcoin::hashes::sha256::Hash::hash(&preimage);

    let payment_secret = zap.request.id.to_bytes();

    let private_key =
        SecretKey::from_hashed_data::<bitcoin::hashes::sha256::Hash>(zap.request.id.as_bytes());

    let amt_msats = zap
        .invoice
        .amount_milli_satoshis()
        .expect("Invoice must have an amount");

    let fake_invoice = InvoiceBuilder::new(Currency::Bitcoin)
        .amount_milli_satoshis(amt_msats)
        .invoice_description(zap.invoice.description())
        .current_timestamp()
        .payment_hash(invoice_hash)
        .payment_secret(PaymentSecret(payment_secret))
        .min_final_cltv_expiry_delta(144)
        .basic_mpp()
        .build_signed(|hash| {
            Secp256k1::signing_only().sign_ecdsa_recoverable(hash, &private_key)
        })?;

    let event = EventBuilder::zap_receipt(
        fake_invoice.to_string(),
        Some(hex::encode(preimage)),
        &zap.request.clone(),
    )
    .to_event(keys)?;

    let event_id = client.send_event(event.clone()).await?;
    Ok(event_id)
}

async fn ephermal_client(client: Client, zap: &mut Zap) -> anyhow::Result<Client> {
    let og_client = client.clone();
    let options = Options::default();
    let client = Client::with_opts(
        og_client.signer().await?,
        options
            .wait_for_send(true)
            .send_timeout(Some(Duration::from_secs(20))),
    );
    let relays = og_client.relays().await;
    let relays = relays.keys();
    client.add_relays(relays).await?;
    client.add_relays(utils::get_relays(&zap.request)?).await?;
    client.connect().await;
    client.set_zapper(og_client.zapper().await?).await;
    Ok(client)
}
