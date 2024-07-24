use crate::db;
use crate::db::get_zap;
use crate::db::upsert_zap;
use anyhow::Context;
use bitcoin::hashes::sha256::Hash as Sha256;
use bitcoin::hashes::Hash;
use bitcoin::key::Secp256k1;
use bitcoin::secp256k1::SecretKey;
use lightning_invoice::Currency;
use lightning_invoice::InvoiceBuilder;
use lightning_invoice::PaymentSecret;
use nostr::prelude::ToBech32;
use nostr::EventBuilder;
use nostr::Keys;
use nostr_sdk::Client;
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
                        async move {
                            let fut = handle_paid_invoice(
                                &db,
                                hex::encode(ln_invoice.r_hash),
                                key.clone(),
                                client,
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
) -> anyhow::Result<()> {
    let dice_roll = db::get_active_dice_roll(db)?.context("No active dice roll.")?;
    let roll_event_id = dice_roll.event_id;

    match get_zap(db, roll_event_id, payment_hash.clone())? {
        None => {
            tracing::warn!("Received a payment without bet.");
            Ok(())
        }
        Some(mut zap) => {
            if zap.receipt_id.is_some() {
                return Ok(());
            }

            let preimage = zap.request.id.to_bytes();
            let invoice_hash = Sha256::hash(&preimage);

            let payment_secret = zap.request.id.to_bytes();

            let private_key = SecretKey::from_hashed_data::<Sha256>(zap.request.id.as_bytes());

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
                zap.request.clone(),
            )
            .to_event(&keys)?;

            let event_id = client.send_event(event).await?;

            tracing::info!(
                "Broadcasted event id: {}!",
                event_id.to_bech32().expect("bech32")
            );

            zap.receipt_id = Some(event_id.to_bech32().expect("bech32"));
            upsert_zap(db, roll_event_id, payment_hash, zap)?;

            Ok(())
        }
    }
}
